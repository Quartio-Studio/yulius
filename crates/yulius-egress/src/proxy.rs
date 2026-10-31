//! The proxy itself: authentication of agent runs, tunnels, interception of bound hosts,
//! forwarding over verified HTTPS and redaction of responses.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::{Stream, StreamExt, TryStreamExt};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, BodyStream, Empty, Full, StreamBody};
use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::http::request::Parts;
use hyper::http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode, header};
use hyper::upgrade::Upgraded;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use yulius_app::ports::egress::{EgressClient, EgressDecision, EgressPolicy, EgressRequest};
use yulius_domain::vault::egress_destination_allowed;
use yulius_domain::vault::redact::{Matcher, PlainMatcher, StreamRedactor, redact_all};

use crate::ca::CertificateAuthority;

/// Boxed error of bodies and resolvers.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// Response body of the proxy.
pub type EgressBody = UnsyncBoxBody<Bytes, std::io::Error>;

/// Least time between two warnings about the same limit (SPEC-353 LAP-002).
const WARN_EVERY: Duration = Duration::from_secs(60);

/// Connect timeout toward a destination.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// TLS handshake with the agent's client.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Request head read timeout inside an intercepted connection.
const HEADER_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest life of one tunnel or intercepted connection.
const CONNECTION_MAX: Duration = Duration::from_secs(24 * 3600);
/// DNS resolution timeout.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Headers that belong to one hop (RFC 9110 §7.6.1) or to the proxy.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

/// Proxy configuration.
#[derive(Debug, Clone)]
pub struct EgressConfig {
    /// Fixed addresses for host names, port included (tests); they bypass the destination
    /// policy.
    pub resolve: HashMap<String, SocketAddr>,
    /// Extra PEM roots trusted upstream (tests).
    pub extra_roots_pem: Vec<String>,
    /// Simultaneous tunnels and intercepted connections.
    pub max_connections: usize,
    /// Simultaneous tunnels and intercepted connections of one sandbox (SPEC-353 LAP-002): half
    /// of the proxy, so one sandbox can neither take every slot nor all of its own relay's
    /// connections to Yulius.
    pub max_per_sandbox: usize,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            resolve: HashMap::new(),
            extra_roots_pem: Vec::new(),
            max_connections: 256,
            max_per_sandbox: 128,
        }
    }
}

/// The egress proxy.
pub struct EgressProxy {
    policy: Arc<dyn EgressPolicy>,
    ca: CertificateAuthority,
    http: reqwest::Client,
    resolve: HashMap<String, SocketAddr>,
    slots: Arc<Semaphore>,
    /// Each sandbox's share of [`Self::slots`] (LAP-002), one entry per sandbox that asked.
    shares: Mutex<HashMap<String, Share>>,
    max_per_sandbox: usize,
    /// When the proxy last warned that every slot was taken.
    full_warned: Mutex<Option<Instant>>,
}

/// One sandbox's connections and when the proxy last warned that they were all taken.
struct Share {
    slots: Arc<Semaphore>,
    warned: Option<Instant>,
}

/// `sandbox`'s share in `shares` (`max` connections). A sandbox seen for the first time drops the
/// entries of sandboxes that hold no connection, so the map holds the busy sandboxes and one more.
fn share_for<'a>(
    shares: &'a mut HashMap<String, Share>,
    sandbox: &str,
    max: usize,
) -> &'a mut Share {
    if !shares.contains_key(sandbox) {
        shares.retain(|_, share| share.slots.available_permits() < max);
    }
    shares.entry(sandbox.to_owned()).or_insert_with(|| Share {
        slots: Arc::new(Semaphore::new(max)),
        warned: None,
    })
}

/// A connection's places: one in the proxy, one in its sandbox's share. Dropped when it ends.
struct Slot {
    _proxy: OwnedSemaphorePermit,
    _sandbox: OwnedSemaphorePermit,
}

/// Whether a warning last given at `last` may be given again now (and records it).
fn warn_due(last: &mut Option<Instant>) -> bool {
    let due = last.is_none_or(|at| at.elapsed() >= WARN_EVERY);
    if due {
        *last = Some(Instant::now());
    }
    due
}

impl std::fmt::Debug for EgressProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressProxy").finish_non_exhaustive()
    }
}

/// Resolves a destination and keeps only addresses the proxy may reach.
async fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if egress_destination_allowed(ip) {
            Ok(vec![SocketAddr::new(ip, port)])
        } else {
            Err(format!(
                "Yulius proxy: {host} is a local address; it is not reachable through the proxy"
            ))
        };
    }
    let found = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| format!("Yulius proxy: resolving {host} timed out"))?
        .map_err(|_| format!("Yulius proxy: {host} could not be resolved"))?;
    let allowed: Vec<SocketAddr> = found
        .filter(|a| egress_destination_allowed(a.ip()))
        .collect();
    if allowed.is_empty() {
        return Err(format!(
            "Yulius proxy: {host} resolves only to local addresses; it is not reachable through the proxy"
        ));
    }
    Ok(allowed)
}

/// reqwest's resolver: the same destination policy (no DNS rebinding to local addresses).
struct PolicyResolver;

impl reqwest::dns::Resolve for PolicyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = lookup(&host, 0).await.map_err(BoxError::from)?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn text(status: u16, message: &str) -> Response<EgressBody> {
    let mut response = Response::new(
        Full::new(Bytes::from(format!("{message}\n")))
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn auth_required() -> Response<EgressBody> {
    let mut response = text(
        407,
        "Yulius proxy: proxy credentials (the run's task token) are required",
    );
    response.headers_mut().insert(
        header::PROXY_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"yulius\""),
    );
    response
}

fn empty(status: StatusCode) -> Response<EgressBody> {
    let mut response = Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync());
    *response.status_mut() = status;
    response
}

/// The task token from `Proxy-Authorization: Basic base64(user:token)`.
fn proxy_token(headers: &hyper::http::HeaderMap) -> Option<String> {
    let value = headers.get(header::PROXY_AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (_, token) = decoded.split_once(':')?;
    (!token.is_empty()).then(|| token.to_owned())
}

fn hop(name: &str) -> bool {
    HOP_BY_HOP.contains(&name)
}

/// A host for a URL (IPv6 literals in brackets).
fn url_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

/// `host:port` of a CONNECT request.
fn connect_target(uri: &hyper::http::Uri) -> Option<(String, u16)> {
    let authority = uri.authority()?;
    let host = authority
        .host()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    let port = authority.port_u16()?;
    (!host.is_empty() && port != 0).then_some((host, port))
}

/// Where a request goes.
struct Upstream {
    scheme: &'static str,
    host: String,
    port: u16,
    /// The host carries agent secrets: the request is rewritten by the policy.
    bound: bool,
}

impl Upstream {
    fn url(&self, path: &str) -> String {
        let default = if self.scheme == "https" { 443 } else { 80 };
        let port = if self.port == default {
            String::new()
        } else {
            format!(":{}", self.port)
        };
        format!("{}://{}{port}{path}", self.scheme, url_host(&self.host))
    }
}

/// A response body redacted as it streams.
fn redacting_body(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    matcher: Arc<PlainMatcher>,
) -> EgressBody {
    let state = (Box::pin(stream), Some(StreamRedactor::new(matcher, 1)));
    let frames = futures::stream::unfold(state, |(mut stream, redactor)| async move {
        let mut redactor = redactor?;
        match stream.next().await {
            Some(Ok(chunk)) => {
                let out = redactor.push(&chunk);
                Some((Ok(Frame::data(Bytes::from(out))), (stream, Some(redactor))))
            }
            Some(Err(error)) => Some((Err(std::io::Error::other(error)), (stream, None))),
            None => {
                let out = redactor.flush();
                Some((Ok(Frame::data(Bytes::from(out))), (stream, None)))
            }
        }
    });
    StreamBody::new(frames).boxed_unsync()
}

impl EgressProxy {
    /// A proxy asking `policy`, signing with `ca`.
    ///
    /// # Errors
    /// The upstream HTTP client cannot be built (TLS roots).
    pub fn new(
        policy: Arc<dyn EgressPolicy>,
        ca: CertificateAuthority,
        config: EgressConfig,
    ) -> Result<Self, BoxError> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .dns_resolver(Arc::new(PolicyResolver));
        for (host, addr) in &config.resolve {
            builder = builder.resolve(host, *addr);
        }
        let roots = config
            .extra_roots_pem
            .iter()
            .map(|pem| reqwest::Certificate::from_pem(pem.as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        if !roots.is_empty() {
            builder = builder.tls_certs_merge(roots);
        }
        Ok(Self {
            policy,
            ca,
            http: builder.build()?,
            resolve: config.resolve,
            slots: Arc::new(Semaphore::new(config.max_connections.max(1))),
            shares: Mutex::new(HashMap::new()),
            max_per_sandbox: config.max_per_sandbox.max(1),
            full_warned: Mutex::new(None),
        })
    }

    /// A place for one connection of `client`'s sandbox, or the text of the `503` refusing it:
    /// past the sandbox's share, or with every slot of the proxy taken (LAP-002). Each refusal is logged
    /// as a warning, at most once a minute per limit and sandbox.
    fn slot(&self, client: &EgressClient) -> Result<Slot, String> {
        let sandbox = client.sandbox_id.as_str();
        let share = {
            let mut shares = self
                .shares
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let share = share_for(&mut shares, sandbox, self.max_per_sandbox);
            match Arc::clone(&share.slots).try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    if warn_due(&mut share.warned) {
                        tracing::warn!(
                            sandbox,
                            share = self.max_per_sandbox,
                            "egress proxy: a sandbox holds its share of the connections; \
                             leftover processes may hold them (docs/operations.md)"
                        );
                    }
                    return Err(format!(
                        "Yulius proxy: this sandbox's processes hold all of its {} connections; \
                         end leftover processes or try again",
                        self.max_per_sandbox
                    ));
                }
            }
        };
        let Ok(proxy) = Arc::clone(&self.slots).try_acquire_owned() else {
            let mut warned = self
                .full_warned
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if warn_due(&mut warned) {
                tracing::warn!(sandbox, "egress proxy: every connection is taken");
            }
            return Err("Yulius proxy: too many connections; try again".to_owned());
        };
        Ok(Slot {
            _proxy: proxy,
            _sandbox: share,
        })
    }

    /// The CA certificate agent runs trust.
    #[must_use]
    pub fn ca_pem(&self) -> &str {
        self.ca.cert_pem()
    }

    async fn destination(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        if let Some(addr) = self.resolve.get(host) {
            return Ok(vec![*addr]);
        }
        lookup(host, port).await
    }

    /// Handles one proxy request: `CONNECT host:port` (tunnel, or interception of a host bound
    /// to an agent secret) or an absolute `http://` request.
    pub async fn handle<B>(self: &Arc<Self>, mut request: Request<B>) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let Some(token) = proxy_token(request.headers()) else {
            return auth_required();
        };
        let Some(client) = self.policy.egress_client(&token).await else {
            return auth_required();
        };
        let permit = match self.slot(&client) {
            Ok(slot) => slot,
            Err(refused) => return text(503, &refused),
        };
        if request.method() == Method::CONNECT {
            let Some((host, port)) = connect_target(request.uri()) else {
                return text(400, "Yulius proxy: CONNECT needs host:port");
            };
            let intercept = self.policy.egress_intercepts(&client, &host, port).await;
            let addrs = match self.destination(&host, port).await {
                Ok(addrs) => addrs,
                Err(message) => return text(403, &message),
            };
            let on_upgrade = hyper::upgrade::on(&mut request);
            let proxy = Arc::clone(self);
            tokio::spawn(async move {
                match on_upgrade.await {
                    Ok(upgraded) if intercept => {
                        proxy.intercept(client, host, port, upgraded, permit).await;
                    }
                    Ok(upgraded) => tunnel(addrs, upgraded, permit).await,
                    Err(error) => tracing::debug!(%error, "proxy tunnel was not opened"),
                }
            });
            return empty(StatusCode::OK);
        }
        let absolute = request.uri().scheme_str() == Some("http");
        let Some(host) = request
            .uri()
            .host()
            .map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase())
            .filter(|_| absolute)
        else {
            return text(
                400,
                "Yulius proxy: expected CONNECT or an absolute http:// URL",
            );
        };
        let port = request.uri().port_u16().unwrap_or(80);
        // A plain-HTTP request to a bound host is sent over HTTPS (a secret never travels in
        // clear): port 80 means the host's HTTPS port.
        let https_port = if port == 80 { 443 } else { port };
        let bound = self
            .policy
            .egress_intercepts(&client, &host, https_port)
            .await;
        if let Err(message) = self.destination(&host, port).await {
            return text(403, &message);
        }
        let upstream = if bound {
            Upstream {
                scheme: "https",
                host,
                port: https_port,
                bound: true,
            }
        } else {
            Upstream {
                scheme: "http",
                host,
                port,
                bound: false,
            }
        };
        let (parts, body) = request.into_parts();
        let response = self.forward(&client, &upstream, parts, body).await;
        drop(permit);
        response
    }

    async fn intercept(
        self: Arc<Self>,
        client: EgressClient,
        host: String,
        port: u16,
        upgraded: Upgraded,
        permit: Slot,
    ) {
        let config = match self.ca.server_config(&host) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(%error, "no certificate for an intercepted host");
                return;
            }
        };
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        let tls =
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(TokioIo::new(upgraded)))
                .await
            {
                Ok(Ok(tls)) => tls,
                Ok(Err(error)) => {
                    tracing::debug!(%error, "client refused the egress certificate");
                    return;
                }
                Err(_) => return,
            };
        let proxy = Arc::clone(&self);
        let service = hyper::service::service_fn(move |request: Request<Incoming>| {
            let proxy = Arc::clone(&proxy);
            let client = client.clone();
            let upstream = Upstream {
                scheme: "https",
                host: host.clone(),
                port,
                bound: true,
            };
            async move {
                let (parts, body) = request.into_parts();
                Ok::<_, Infallible>(proxy.forward(&client, &upstream, parts, body).await)
            }
        });
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .keep_alive(true);
        let serving = builder.serve_connection(TokioIo::new(tls), service);
        if let Ok(Err(error)) = tokio::time::timeout(CONNECTION_MAX, serving).await {
            tracing::debug!(%error, "intercepted connection ended");
        }
        drop(permit);
    }

    /// Sends one request upstream (rewritten by the policy for a bound host) and streams the
    /// redacted response back.
    async fn forward<B>(
        &self,
        client: &EgressClient,
        upstream: &Upstream,
        parts: Parts,
        body: B,
    ) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        if parts.headers.contains_key(header::UPGRADE) {
            return text(
                501,
                "Yulius proxy: protocol upgrades (WebSockets) are not proxied for hosts with agent secrets",
            );
        }
        let listed: Vec<String> = parts
            .headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|v| v.trim().to_ascii_lowercase())
            .collect();
        let path = parts
            .uri
            .path_and_query()
            .map_or("/", |p| p.as_str())
            .to_owned();
        let mut headers: Vec<(String, Vec<u8>)> = parts
            .headers
            .iter()
            .filter(|(name, _)| !hop(name.as_str()) && !listed.iter().any(|l| l == name.as_str()))
            .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
            .collect();
        let (path, headers) = if upstream.bound {
            let request = EgressRequest {
                host: upstream.host.clone(),
                port: upstream.port,
                path_and_query: path,
                headers,
            };
            match self.policy.egress_apply(client, request).await {
                EgressDecision::Forward {
                    path_and_query,
                    headers,
                    ..
                } => (path_and_query, headers),
                EgressDecision::Refuse { status, message } => return text(status, &message),
            }
        } else {
            headers.retain(|(name, _)| name != "x-yulius-secret");
            (path, headers)
        };
        let mut builder = self.http.request(parts.method.clone(), upstream.url(&path));
        for (name, value) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_bytes(&value),
            ) {
                builder = builder.header(name, value);
            }
        }
        if !(body.is_end_stream() || body.size_hint().exact() == Some(0)) {
            let stream = TryStreamExt::map_err(BodyStream::new(body), Into::<BoxError>::into)
                .try_filter_map(|frame| async move { Ok(frame.into_data().ok()) });
            builder = builder.body(reqwest::Body::wrap_stream(stream));
        }
        let response = match builder.send().await {
            Ok(response) => response,
            Err(error) => {
                let why = if error.is_timeout() {
                    "timed out"
                } else if error.is_connect() {
                    "connection failed"
                } else {
                    "request failed"
                };
                tracing::debug!(host = %upstream.host, error = %error, "upstream request failed");
                return text(
                    502,
                    &format!(
                        "Yulius proxy: {} could not be reached ({why})",
                        upstream.host
                    ),
                );
            }
        };
        self.respond(client, response).await
    }

    async fn respond(
        &self,
        client: &EgressClient,
        response: reqwest::Response,
    ) -> Response<EgressBody> {
        let matcher = self.policy.egress_redactor(client).await;
        if !matcher.is_empty()
            && let Some(encoding) = response
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_ascii_lowercase())
            && encoding != "identity"
        {
            return text(
                502,
                &format!(
                    "Yulius proxy: the response is compressed ({encoding}) and cannot be checked for agent secrets; request it without Accept-Encoding"
                ),
            );
        }
        let mut builder = Response::builder().status(response.status().as_u16());
        for (name, value) in response.headers() {
            if hop(name.as_str()) || name == header::CONTENT_LENGTH {
                continue;
            }
            let (redacted, _) = redact_all(matcher.as_ref(), value.as_bytes());
            if let Ok(value) = HeaderValue::from_bytes(&redacted) {
                builder = builder.header(name, value);
            }
        }
        let body = redacting_body(response.bytes_stream(), matcher);
        builder
            .body(body)
            .unwrap_or_else(|_| text(502, "Yulius proxy: invalid upstream response"))
    }
}

/// An opaque tunnel to a host without agent secrets.
async fn tunnel(addrs: Vec<SocketAddr>, upgraded: Upgraded, permit: Slot) {
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addrs[..])).await {
        Ok(Ok(stream)) => stream,
        _ => {
            tracing::debug!("tunnel destination unreachable");
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let mut client = TokioIo::new(upgraded);
    let mut server = stream;
    let _ = tokio::time::timeout(
        CONNECTION_MAX,
        tokio::io::copy_bidirectional(&mut client, &mut server),
    )
    .await;
    drop(permit);
}

/// Serves the proxy on its own listener (tests and standalone use); Yulius serves it on the
/// internal listener instead.
pub async fn serve(listener: TcpListener, proxy: Arc<EgressProxy>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let proxy = Arc::clone(&proxy);
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let proxy = Arc::clone(&proxy);
                async move { Ok::<_, Infallible>(proxy.handle(request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LAP-002 (review round 1): the shares stay bounded. A new sandbox's entry drops those of
    /// sandboxes that hold no connection; one that holds connections keeps its entry.
    #[test]
    fn idle_sandboxes_lose_their_share_entry() {
        let mut shares = HashMap::new();
        let busy = Arc::clone(&share_for(&mut shares, "busy", 2).slots).try_acquire_owned();
        assert!(busy.is_ok());
        for n in 0..100 {
            let _ = share_for(&mut shares, &format!("idle-{n}"), 2);
        }
        let mut kept: Vec<&String> = shares.keys().collect();
        kept.sort();
        assert_eq!(kept, ["busy", "idle-99"]);
    }

    #[test]
    fn proxy_credentials_and_targets() {
        let mut headers = hyper::http::HeaderMap::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode("yulius:tok123");
        headers.insert(
            header::PROXY_AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {encoded}"))
                .unwrap_or(HeaderValue::from_static("")),
        );
        assert_eq!(proxy_token(&headers), Some("tok123".into()));
        assert_eq!(proxy_token(&hyper::http::HeaderMap::new()), None);
        let uri: hyper::http::Uri = "api.example.com:443".parse().unwrap_or_default();
        assert_eq!(connect_target(&uri), Some(("api.example.com".into(), 443)));
        let upstream = Upstream {
            scheme: "https",
            host: "::1".into(),
            port: 8443,
            bound: true,
        };
        assert_eq!(upstream.url("/x"), "https://[::1]:8443/x");
        let default = Upstream {
            scheme: "https",
            host: "a.com".into(),
            port: 443,
            bound: true,
        };
        assert_eq!(default.url("/"), "https://a.com/");
    }

    #[tokio::test]
    async fn local_destinations_are_refused() {
        assert!(lookup("127.0.0.1", 80).await.is_err());
        assert!(lookup("169.254.169.254", 80).await.is_err());
        assert!(lookup("localhost", 80).await.is_err());
        assert!(lookup("1.1.1.1", 443).await.is_ok());
    }
}
