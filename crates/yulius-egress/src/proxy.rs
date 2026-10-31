//! The proxy itself: authentication of agent runs, tunnels, interception of bound hosts,
//! forwarding over verified HTTPS and redaction of responses.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
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
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use yulius_app::ports::egress::{EgressClient, EgressDecision, EgressPolicy, EgressRequest};
use yulius_domain::connections::{self, Activity, Busier, Connections};
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
/// Least idle time before a connection gives way to a new one at a limit (SPEC-355 SLR-002,
/// SLR-003): the relays' (`yulius_sandbox::relay::IDLE_FLOOR`), longer than any call to Yulius
/// waits without a byte.
pub const IDLE_FLOOR: Duration = Duration::from_secs(300);
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
    /// How long a connection is idle (no byte either way) before it may give way to a new one at
    /// a sandbox's share or the proxy's limit (SPEC-355 SLR-002).
    pub idle_floor: Duration,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            resolve: HashMap::new(),
            extra_roots_pem: Vec::new(),
            max_connections: 256,
            max_per_sandbox: 128,
            idle_floor: IDLE_FLOOR,
        }
    }
}

/// The egress proxy.
pub struct EgressProxy {
    policy: Arc<dyn EgressPolicy>,
    ca: CertificateAuthority,
    http: reqwest::Client,
    resolve: HashMap<String, SocketAddr>,
    /// The open tunnels and intercepted connections by sandbox (LAP-002, SPEC-355 SLR-002).
    slots: Arc<Slots>,
    max_per_sandbox: usize,
    /// When the proxy last gave each warning (a kind, and a sandbox or `None` for the whole
    /// proxy). Holds the warnings of the last minute only.
    warned: Mutex<Warned>,
}

/// When each warning was last given, by kind and sandbox.
type Warned = HashMap<(&'static str, Option<String>), Instant>;

/// The proxy's connections: at most `max_connections`, `max_per_sandbox` per sandbox; at a limit
/// the longest-idle one past the floor gives way (SLR-002), else the oldest of a busier run or
/// sandbox (SPEC-357). Each holds the sender that ends it.
struct Slots {
    /// By sandbox and run (`EgressClient::use_key`), SPEC-357.
    table: Mutex<Connections<String, oneshot::Sender<()>, String>>,
    epoch: Instant,
}

impl Slots {
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// A connection's place in its sandbox's share and the proxy. Freed when it ends.
struct Slot {
    slots: Arc<Slots>,
    id: u64,
    activity: Activity,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.slots
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(self.id);
    }
}

/// A connection's stream whose every byte either way marks it active (SLR-002).
struct Tracked<S> {
    inner: S,
    slots: Arc<Slots>,
    activity: Activity,
}

impl<S> Tracked<S> {
    fn new(inner: S, slot: &Slot) -> Self {
        Self {
            inner,
            slots: Arc::clone(&slot.slots),
            activity: slot.activity.clone(),
        }
    }

    fn touch(&self) {
        self.activity.touch(self.slots.now());
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Tracked<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.touch();
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tracked<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if matches!(poll, Poll::Ready(Ok(n)) if n > 0) {
            self.touch();
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Whether the warning `kind` about `sandbox` (`None`: the whole proxy) may be given again now
/// (and records it); forgets the warnings older than [`WARN_EVERY`], so the map stays as small as
/// the minute's warnings.
fn warn_due(warned: &mut Warned, kind: &'static str, sandbox: Option<&str>) -> bool {
    warned.retain(|_, at| at.elapsed() < WARN_EVERY);
    let key = (kind, sandbox.map(str::to_owned));
    if warned.contains_key(&key) {
        return false;
    }
    warned.insert(key, Instant::now());
    true
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

/// The warning kind of a connection taken from a busier run or sandbox: each warns once a minute
/// per sandbox, apart (SPEC-357 ESR-005).
fn busier_kind(busier: Busier) -> &'static str {
    match busier {
        Busier::Owner(_) => "busiest run",
        Busier::Key(_) => "busiest sandbox",
    }
}

/// The `503` at a full sandbox share (SPEC-357 ESR-004): `mine` of `share` are the asking run's,
/// another run holds at most `most_held` (held in flight included). "The most" only when no
/// other run holds as many; else the table found no run with `MARGIN` more to give up.
fn share_refusal(share: usize, mine: usize, most_held: usize) -> String {
    let whose = if mine > most_held {
        "the most of the sandbox's runs: end this run's leftover processes".to_owned()
    } else {
        format!(
            "and no other run has {} more to give up: end leftover processes",
            connections::MARGIN
        )
    };
    format!(
        "Yulius proxy: this sandbox's processes hold all of its {share} connections, each active \
         lately; this run holds {mine} of them, {whose} or try again"
    )
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

/// A request as it goes upstream, once the policy allowed it.
struct Outgoing {
    method: Method,
    path: String,
    headers: Vec<(String, Vec<u8>)>,
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
            slots: Arc::new(Slots {
                table: Mutex::new(Connections::with_owners(
                    config.max_connections,
                    config.max_per_sandbox,
                    u64::try_from(config.idle_floor.as_millis()).unwrap_or(u64::MAX),
                )),
                epoch: Instant::now(),
            }),
            max_per_sandbox: config
                .max_per_sandbox
                .clamp(1, config.max_connections.max(1)),
            warned: Mutex::new(HashMap::new()),
        })
    }

    /// A place for one connection of `client`'s run in its sandbox, with what resolves once it
    /// gave way, or the text of the `503` refusing it. Past the sandbox's share its longest-idle
    /// connection gives way, past the proxy's limit the longest-idle of all (SPEC-355 SLR-002);
    /// failing that, the oldest of the busiest other run (or sandbox) holding 2 more (SPEC-357
    /// ESR-002, ESR-003); else the connection is refused (LAP-002) with how many the asking run
    /// holds. Each refusal and each connection that gave way is logged as a warning, at most once
    /// a minute per limit and sandbox.
    ///
    /// A `held` connection (a plain request) is held in flight before the table is released, so
    /// nothing takes it meanwhile (review round 4).
    fn slot(
        &self,
        client: &EgressClient,
        held: bool,
    ) -> Result<(Slot, oneshot::Receiver<()>), String> {
        let sandbox = client.sandbox_id.as_str();
        let run = client.use_key();
        let (end, ended) = oneshot::channel();
        let (admitted, counts) = {
            let mut table = self
                .slots
                .table
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (key, owner, now) = (sandbox.to_owned(), run.to_owned(), self.slots.now());
            let admitted = if held {
                table.admit_held_by(key, owner, end, now)
            } else {
                table.admit_by(key, owner, end, now)
            };
            // Only a refusal at the share tells how many (SPEC-357 ESR-004).
            let counts = matches!(admitted, Err(connections::Full::Key)).then(|| {
                let (sandbox, run) = (sandbox.to_owned(), run.to_owned());
                (
                    table.count_of(&sandbox, &run),
                    table.most_of_others(&sandbox, &run),
                )
            });
            (admitted, counts)
        };
        let mut warned = self.warned.lock().unwrap_or_else(PoisonError::into_inner);
        match admitted {
            Ok(admitted) => {
                if let Some(evicted) = &admitted.evicted
                    && let Some(busier) = evicted.busier
                {
                    if warn_due(&mut warned, busier_kind(busier), Some(&evicted.key)) {
                        match busier {
                            Busier::Owner(held) => tracing::warn!(
                                sandbox = %evicted.key,
                                run = %evicted.owner,
                                held,
                                "egress proxy: a sandbox's share is full of active connections; \
                                 the busiest run's oldest connection was closed for another run \
                                 of the sandbox; its leftover processes may hold them \
                                 (docs/operations.md)"
                            ),
                            Busier::Key(held) => tracing::warn!(
                                sandbox = %evicted.key,
                                run = %evicted.owner,
                                sandbox_held = held,
                                for_sandbox = sandbox,
                                "egress proxy: every connection is taken and active; the busiest \
                                 sandbox's oldest connection was closed for another sandbox"
                            ),
                        }
                    }
                } else if let Some(evicted) = admitted.evicted
                    && warn_due(&mut warned, "gave way", Some(&evicted.key))
                {
                    tracing::warn!(
                        sandbox = %evicted.key,
                        idle_s = evicted.idle_ms / 1000,
                        "egress proxy: at a limit, the longest-idle connection was closed for a \
                         new one; leftover processes may hold them (docs/operations.md)"
                    );
                }
                let slot = Slot {
                    slots: Arc::clone(&self.slots),
                    id: admitted.id,
                    activity: admitted.activity,
                };
                Ok((slot, ended))
            }
            Err(connections::Full::Key) => {
                let (mine, most_held) = counts.unwrap_or_default();
                if warn_due(&mut warned, "share", Some(sandbox)) {
                    tracing::warn!(
                        sandbox,
                        run,
                        held = mine,
                        share = self.max_per_sandbox,
                        "egress proxy: a sandbox holds its share of the connections; \
                         leftover processes may hold them (docs/operations.md)"
                    );
                }
                Err(share_refusal(self.max_per_sandbox, mine, most_held))
            }
            Err(connections::Full::Total) => {
                if warn_due(&mut warned, "full", None) {
                    tracing::warn!(sandbox, "egress proxy: every connection is taken");
                }
                Err("Yulius proxy: too many connections; try again".to_owned())
            }
        }
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
        // A place is taken only once the request is known to be served, so that a refused one
        // never closes another connection for it (SPEC-357, review round 3).
        if request.method() == Method::CONNECT {
            let Some((host, port)) = connect_target(request.uri()) else {
                return text(400, "Yulius proxy: CONNECT needs host:port");
            };
            let intercept = self.policy.egress_intercepts(&client, &host, port).await;
            let addrs = match self.destination(&host, port).await {
                Ok(addrs) => addrs,
                Err(message) => return text(403, &message),
            };
            let (permit, ended) = match self.slot(&client, false) {
                Ok(slot) => slot,
                Err(refused) => return text(503, &refused),
            };
            let on_upgrade = hyper::upgrade::on(&mut request);
            let proxy = Arc::clone(self);
            tokio::spawn(async move {
                let serving = async move {
                    match on_upgrade.await {
                        Ok(upgraded) if intercept => {
                            proxy.intercept(client, host, port, upgraded, permit).await;
                        }
                        Ok(upgraded) => tunnel(addrs, upgraded, permit).await,
                        Err(error) => tracing::debug!(%error, "proxy tunnel was not opened"),
                    }
                };
                // A connection that gave way to a new one ends here (SLR-002).
                tokio::select! {
                    () = serving => {}
                    _ = ended => {}
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
        // The policy decides before a place is taken: a refused request takes none (review
        // round 4).
        let outgoing = match self.prepare(&client, &upstream, &parts).await {
            Ok(outgoing) => outgoing,
            Err((status, message)) => return text(status, &message),
        };
        // In flight until its answer's head arrives: it never gives way meanwhile (its bytes pass
        // through no tracked stream).
        let (permit, ended) = match self.slot(&client, true) {
            Ok(slot) => slot,
            Err(refused) => return text(503, &refused),
        };
        drop(ended);
        let response = self.send(&client, &upstream, outgoing, body).await;
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
        let client_io = Tracked::new(TokioIo::new(upgraded), &permit);
        let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(client_io)).await {
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
        match self.prepare(client, upstream, &parts).await {
            Ok(outgoing) => self.send(client, upstream, outgoing, body).await,
            Err((status, message)) => text(status, &message),
        }
    }

    /// What `parts` becomes upstream (rewritten by the policy for a bound host), or the status and
    /// text refusing it.
    async fn prepare(
        &self,
        client: &EgressClient,
        upstream: &Upstream,
        parts: &Parts,
    ) -> Result<Outgoing, (u16, String)> {
        if parts.headers.contains_key(header::UPGRADE) {
            return Err((
                501,
                "Yulius proxy: protocol upgrades (WebSockets) are not proxied for hosts with agent secrets"
                    .to_owned(),
            ));
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
                EgressDecision::Refuse { status, message } => return Err((status, message)),
            }
        } else {
            headers.retain(|(name, _)| name != "x-yulius-secret");
            (path, headers)
        };
        Ok(Outgoing {
            method: parts.method.clone(),
            path,
            headers,
        })
    }

    /// Sends a prepared request upstream and streams the redacted response back.
    async fn send<B>(
        &self,
        client: &EgressClient,
        upstream: &Upstream,
        outgoing: Outgoing,
        body: B,
    ) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let Outgoing {
            method,
            path,
            headers,
        } = outgoing;
        let mut builder = self.http.request(method, upstream.url(&path));
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
    let mut server = Tracked::new(stream, &permit);
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

    /// LAP-002 (review round 1): the warnings stay bounded: each limit warns once a minute, and
    /// the map keeps only the last minute's.
    #[test]
    fn warnings_are_once_a_minute_per_limit() {
        let mut warned = HashMap::new();
        assert!(warn_due(&mut warned, "share", Some("s1")));
        assert!(!warn_due(&mut warned, "share", Some("s1")));
        assert!(
            warn_due(&mut warned, "gave way", Some("s1")),
            "another kind of warning about the same sandbox (review round 1)"
        );
        assert!(warn_due(&mut warned, "share", Some("s2")));
        assert!(warn_due(&mut warned, "full", None));
        assert!(!warn_due(&mut warned, "full", None));
        let old = Instant::now().checked_sub(WARN_EVERY);
        for at in warned.values_mut() {
            *at = old.unwrap_or(*at);
        }
        assert!(warn_due(&mut warned, "share", Some("s3")));
        assert_eq!(warned.len(), 1, "older warnings are forgotten");
    }

    /// SPEC-357 ESR-004 (review round 2): "the most" only when no other run holds as many, held
    /// connections included; "nothing to give up" names the margin the table used.
    #[test]
    fn a_share_refusal_names_the_busiest_run_only_when_it_is() {
        let busiest = share_refusal(8, 7, 1);
        assert!(busiest.contains("this run holds 7 of them, the most of the sandbox's runs"));
        // Another run holds 7 requests in flight: it has nothing to give up, but it is busier.
        let not = share_refusal(8, 1, 7);
        assert!(!not.contains("the most"), "{not}");
        assert!(
            not.contains(&format!(
                "this run holds 1 of them, and no other run has {} more to give up",
                connections::MARGIN
            )),
            "{not}"
        );
    }

    /// SPEC-357 ESR-005 (review round 2): a run-level and a sandbox-level take warn apart.
    #[test]
    fn takes_from_a_busier_run_and_sandbox_warn_apart() {
        let (run, sandbox) = (busier_kind(Busier::Owner(3)), busier_kind(Busier::Key(3)));
        assert_ne!(run, sandbox);
        let mut warned = HashMap::new();
        assert!(warn_due(&mut warned, run, Some("s1")));
        assert!(warn_due(&mut warned, sandbox, Some("s1")));
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
