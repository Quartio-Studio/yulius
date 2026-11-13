//! What a sandbox's browser reaches through the proxy (SPEC-483 E2, ADR-0321).
//!
//! A sandbox's Chrome runs on the host, outside the sandbox's network: left to itself it
//! reaches the host's loopback (Yulius's internal listener, other sandboxes' dev servers, the
//! host's own services) and link-local addresses (a cloud's metadata), which no process of the
//! sandbox does. Chrome is therefore started with this proxy as its only way out, on a listener
//! of its own per sandbox (which says whose browser asks; a browser sends no credential). The
//! proxy connects where the sandbox's own processes could: everywhere but local addresses,
//! plus the sandbox's dev-server ports on the loopback and Yulius's pages for the browser.
//! Nothing is intercepted, injected or redacted: the page is the sandbox's, not Yulius's.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Incoming};
use hyper::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use yulius_domain::vault::egress_destination_allowed;

use crate::proxy::{
    BoxError, CONNECT_TIMEOUT, EgressBody, EgressProxy, RESOLVE_TIMEOUT, connect_target, empty,
    hop, text, tunnel,
};

/// The run a browser's connections are counted as within its sandbox's share (SPEC-357).
const BROWSER_RUN: &str = "browser";

/// One sandbox's browser as the proxy sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEgress {
    /// The sandbox, whose share of the proxy's connections the browser's count toward.
    pub sandbox: String,
    /// The sandbox's dev-server ports: the only ports of the loopback the browser reaches (the
    /// host relays them into the sandbox).
    pub ports: RangeInclusive<u16>,
    /// Yulius's own pages for the browser (the welcome page): the listener serving them and the
    /// beginnings of the paths, query included, that a plain request may ask it for.
    pub pages: Option<(SocketAddr, Vec<String>)>,
}

impl BrowserEgress {
    /// Whether `addr` is the listener of Yulius's pages (an unspecified bind is reached over
    /// the loopback).
    fn serves_pages(&self, addr: SocketAddr) -> bool {
        self.pages.as_ref().is_some_and(|(own, _)| {
            own.port() == addr.port()
                && (own.ip() == addr.ip()
                    || (own.ip().is_unspecified()
                        && (addr.ip().is_loopback() || addr.ip().is_unspecified())))
        })
    }

    /// Whether the browser may connect to `addr`: for a plain request with its `path` (query
    /// included), for a tunnel with none.
    fn reaches(&self, addr: SocketAddr, path: Option<&str>) -> bool {
        if self.serves_pages(addr) {
            // Before the loopback's rule: the listener is the internal API, of which the
            // browser gets these pages and nothing else, and a tunnel would hide the path.
            return self.pages.as_ref().is_some_and(|(_, begins)| {
                path.is_some_and(|path| begins.iter().any(|begin| path.starts_with(begin)))
            });
        }
        if addr.ip().is_loopback() {
            return self.ports.contains(&addr.port());
        }
        egress_destination_allowed(addr.ip())
    }

    fn refusal(&self, host: &str, port: u16) -> String {
        format!(
            "Yulius: the sandbox browser does not load {host}:{port}. Of local addresses it reaches only its sandbox's own ports ({}-{} on localhost)",
            self.ports.start(),
            self.ports.end()
        )
    }
}

impl EgressProxy {
    /// Where the browser's request to `host:port` connects: the addresses `host` resolves to
    /// that the browser may reach (resolved here, so a name cannot mean another address by the
    /// time of the connection), or the text refusing it.
    async fn browser_destination(
        &self,
        browser: &BrowserEgress,
        host: &str,
        port: u16,
        path: Option<&str>,
    ) -> Result<Vec<SocketAddr>, String> {
        if let Some(addr) = self.resolve.get(host) {
            return Ok(vec![*addr]);
        }
        let found: Vec<SocketAddr> = if let Ok(ip) = host.parse() {
            vec![SocketAddr::new(ip, port)]
        } else {
            tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port)))
                .await
                .map_err(|_| format!("Yulius: resolving {host} timed out"))?
                .map_err(|_| format!("Yulius: {host} could not be resolved"))?
                .collect()
        };
        let reached: Vec<SocketAddr> = found
            .into_iter()
            .filter(|addr| browser.reaches(*addr, path))
            .collect();
        if reached.is_empty() {
            return Err(browser.refusal(host, port));
        }
        Ok(reached)
    }

    /// Handles one request of `browser`'s Chrome: `CONNECT host:port` (always a tunnel) or an
    /// absolute `http://` request (sent on unchanged). No credential is asked for or used.
    pub async fn handle_browser<B>(
        self: &Arc<Self>,
        browser: &BrowserEgress,
        mut request: Request<B>,
    ) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        if request.method() == Method::CONNECT {
            let Some((host, port)) = connect_target(request.uri()) else {
                return text(400, "Yulius: CONNECT needs host:port");
            };
            let addrs = match self.browser_destination(browser, &host, port, None).await {
                Ok(addrs) => addrs,
                Err(message) => return text(403, &message),
            };
            let (permit, ended) = match self.slot(&browser.sandbox, BROWSER_RUN, false) {
                Ok(slot) => slot,
                Err(refused) => return text(503, &refused),
            };
            let on_upgrade = hyper::upgrade::on(&mut request);
            tokio::spawn(async move {
                let serving = async move {
                    match on_upgrade.await {
                        Ok(upgraded) => tunnel(addrs, upgraded, permit).await,
                        Err(error) => tracing::debug!(%error, "browser tunnel was not opened"),
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
        let Some(authority) = request.uri().authority().filter(|_| absolute).cloned() else {
            return text(400, "Yulius: expected CONNECT or an absolute http:// URL");
        };
        let host = authority
            .host()
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        let port = authority.port_u16().unwrap_or(80);
        let path = request
            .uri()
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned();
        let addrs = match self
            .browser_destination(browser, &host, port, Some(&path))
            .await
        {
            Ok(addrs) => addrs,
            Err(message) => return text(403, &message),
        };
        // In flight until its answer's head arrives (as an agent's plain request, SPEC-369).
        let (permit, ended) = match self.slot(&browser.sandbox, BROWSER_RUN, true) {
            Ok(slot) => slot,
            Err(refused) => return text(503, &refused),
        };
        drop(ended);
        let response = self
            .send_plain(&addrs, authority.as_str(), &path, request)
            .await;
        drop(permit);
        response
    }

    /// Sends a plain request on to one of `addrs` and hands the answer back as it comes: the
    /// headers of one hop apart, nothing is added, removed or read.
    async fn send_plain<B>(
        &self,
        addrs: &[SocketAddr],
        authority: &str,
        path: &str,
        request: Request<B>,
    ) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let unreachable = || text(502, &format!("Yulius: {authority} could not be reached"));
        let Ok(Ok(stream)) = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addrs)).await
        else {
            return unreachable();
        };
        let _ = stream.set_nodelay(true);
        let Ok((mut sender, connection)) =
            hyper::client::conn::http1::handshake::<_, B>(TokioIo::new(stream)).await
        else {
            return unreachable();
        };
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "a browser's plain connection ended with an error");
            }
        });
        let (parts, body) = request.into_parts();
        let listed: Vec<String> = parts
            .headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(|name| name.trim().to_ascii_lowercase())
            .collect();
        let mut onward = Request::builder().method(parts.method).uri(path);
        for (name, value) in &parts.headers {
            if !hop(name.as_str()) && !listed.iter().any(|l| l == name.as_str()) {
                onward = onward.header(name, value);
            }
        }
        let Ok(host) = HeaderValue::from_str(authority) else {
            return text(400, "Yulius: the request names no valid host");
        };
        let Ok(onward) = onward.header(header::HOST, host).body(body) else {
            return text(400, "Yulius: the request could not be sent on");
        };
        let answered =
            tokio::time::timeout(self.response_head_timeout, sender.send_request(onward)).await;
        let response = match answered {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                tracing::debug!(%error, "a browser's plain request failed");
                return unreachable();
            }
            Err(_) => return text(504, &format!("Yulius: {authority} did not answer")),
        };
        let (mut head, body) = response.into_parts();
        let hops: Vec<header::HeaderName> = head
            .headers
            .keys()
            .filter(|name| hop(name.as_str()))
            .cloned()
            .collect();
        for name in hops {
            head.headers.remove(name);
        }
        Response::from_parts(head, body.map_err(std::io::Error::other).boxed_unsync())
    }
}

/// Serves `browser`'s Chrome on `listener`, the proxy of that one browser. Who connects is not
/// asked: the listener is the browser's own, and reaches nothing a credential would guard.
pub async fn serve_browser(
    listener: TcpListener,
    proxy: Arc<EgressProxy>,
    browser: Arc<BrowserEgress>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (proxy, browser) = (Arc::clone(&proxy), Arc::clone(&browser));
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let (proxy, browser) = (Arc::clone(&proxy), Arc::clone(&browser));
                async move { Ok::<_, Infallible>(proxy.handle_browser(&browser, request).await) }
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
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn browser() -> BrowserEgress {
        BrowserEgress {
            sandbox: "s1".into(),
            ports: 20100..=20119,
            pages: Some((
                SocketAddr::from((Ipv4Addr::LOCALHOST, 8081)),
                vec!["/browser/welcome?".into()],
            )),
        }
    }

    fn at(ip: impl Into<IpAddr>, port: u16) -> SocketAddr {
        SocketAddr::new(ip.into(), port)
    }

    /// SPEC-483 E2: of the loopback, the sandbox's own ports and nothing else; no link-local,
    /// unspecified or mapped form of them; everything a sandbox's process reaches otherwise.
    #[test]
    fn a_browser_reaches_its_sandboxes_ports_and_no_other_local_address() {
        let browser = browser();
        for (addr, reached) in [
            (at(Ipv4Addr::LOCALHOST, 20100), true),
            (at(Ipv4Addr::LOCALHOST, 20119), true),
            (at(Ipv6Addr::LOCALHOST, 20105), true),
            (at(Ipv4Addr::new(127, 0, 0, 2), 20105), true),
            // Another sandbox's port, the public listener, a database of the host.
            (at(Ipv4Addr::LOCALHOST, 20120), false),
            (at(Ipv4Addr::LOCALHOST, 20099), false),
            (at(Ipv4Addr::LOCALHOST, 8080), false),
            (at(Ipv6Addr::LOCALHOST, 5432), false),
            // A cloud's metadata, the unspecified address (the loopback on Linux), mapped forms.
            (at(Ipv4Addr::new(169, 254, 169, 254), 80), false),
            (at(Ipv4Addr::UNSPECIFIED, 20100), false),
            (at(Ipv4Addr::LOCALHOST.to_ipv6_mapped(), 20100), false),
            (
                at(Ipv4Addr::new(169, 254, 169, 254).to_ipv6_mapped(), 80),
                false,
            ),
            // What the sandbox's own processes reach: the internet, the host's network.
            (at(Ipv4Addr::new(93, 184, 216, 34), 443), true),
            (at(Ipv4Addr::new(10, 0, 0, 5), 8080), true),
        ] {
            assert_eq!(browser.reaches(addr, None), reached, "tunnel to {addr}");
            assert_eq!(browser.reaches(addr, Some("/")), reached, "plain to {addr}");
        }
    }

    /// Yulius's listener gives the browser its welcome page by plain request and nothing else:
    /// no other path, no tunnel (whose path is not seen), also when the listener is bound to
    /// every address or its port lies in the sandbox's range.
    #[test]
    fn of_yulius_a_browser_gets_its_pages_only() {
        let browser = browser();
        let own = at(Ipv4Addr::LOCALHOST, 8081);
        assert!(browser.reaches(own, Some("/browser/welcome?sandbox=s1")));
        for path in ["/browser/welcome", "/internal/version", "/", "/mcp"] {
            assert!(!browser.reaches(own, Some(path)), "{path}");
        }
        assert!(!browser.reaches(own, None), "a tunnel");
        assert!(!browser.reaches(at(Ipv6Addr::LOCALHOST, 8081), Some("/browser/welcome?")));

        let everywhere = BrowserEgress {
            ports: 8000..=8999,
            pages: Some((
                at(Ipv4Addr::UNSPECIFIED, 8081),
                vec!["/browser/welcome?".into()],
            )),
            ..browser
        };
        for ip in [
            IpAddr::from(Ipv4Addr::LOCALHOST),
            Ipv6Addr::LOCALHOST.into(),
        ] {
            assert!(everywhere.reaches(at(ip, 8081), Some("/browser/welcome?sandbox=s1")));
            assert!(!everywhere.reaches(at(ip, 8081), Some("/internal/version")));
            assert!(!everywhere.reaches(at(ip, 8081), None));
            assert!(
                everywhere.reaches(at(ip, 8082), None),
                "a port of the sandbox"
            );
        }
    }
}
