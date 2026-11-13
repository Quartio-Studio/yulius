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
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use http_body_util::{BodyExt, BodyStream, StreamBody};
use hyper::body::{Body, Bytes, Incoming};
use hyper::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use yulius_domain::vault::egress_destination_allowed;

use crate::proxy::{
    BoxError, CONNECT_TIMEOUT, EgressBody, EgressProxy, HEADER_TIMEOUT, Progress, RESOLVE_TIMEOUT,
    connect_target, empty, hop, text, tunnel, url_host,
};

/// The run a browser's connections are counted as within its sandbox's share (SPEC-357).
const BROWSER_RUN: &str = "browser";
/// Wait after a failed accept (no file descriptor left) before the next one.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

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
    /// Yulius's other listeners (the public one): not reached at their own address, whatever
    /// the sandbox's ports are.
    pub listeners: Vec<SocketAddr>,
}

/// Whether a connection to `addr` arrives at the listener bound to `own` (one bound to every
/// address is reached over the loopback).
fn arrives_at(own: SocketAddr, addr: SocketAddr) -> bool {
    own.port() == addr.port()
        && (own.ip() == addr.ip()
            || (own.ip().is_unspecified()
                && (addr.ip().is_loopback() || addr.ip().is_unspecified())))
}

impl BrowserEgress {
    /// Whether the browser may connect to `addr`: for a plain request with its `path` (query
    /// included), for a tunnel with none. `local`: the request named the address itself or
    /// `localhost`. A name that the resolver answered with a local address is never one: a
    /// name's owner chooses its answer, and may give a public address first and the loopback
    /// next, so that a page from the first reads the sandbox's dev server through the second.
    fn reaches(&self, addr: SocketAddr, path: Option<&str>, local: bool) -> bool {
        if let Some((own, begins)) = &self.pages
            && local
            && arrives_at(*own, addr)
        {
            // Before the loopback's rule: the listener is the internal API, of which the
            // browser gets these pages and nothing else, and a tunnel would hide the path.
            return path.is_some_and(|path| begins.iter().any(|begin| path.starts_with(begin)));
        }
        if self.listeners.iter().any(|own| arrives_at(*own, addr)) {
            return false;
        }
        if local && addr.ip().is_loopback() {
            return self.ports.contains(&addr.port());
        }
        egress_destination_allowed(addr.ip())
    }

    fn refusal(&self, host: &str, port: u16) -> String {
        let own = if self.ports.is_empty() {
            "none (its sandbox's ports could not be read)".to_owned()
        } else {
            format!(
                "only its sandbox's own ports ({}-{} on localhost, by that name or by address)",
                self.ports.start(),
                self.ports.end()
            )
        };
        format!(
            "Yulius: the sandbox browser does not load {}:{port}. Of local addresses it reaches {own}",
            url_host(host)
        )
    }
}

/// Whether `host` is `localhost` or a name below it (RFC 6761: always the loopback, whatever a
/// resolver says).
fn is_localhost(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    host == "localhost" || host.ends_with(".localhost")
}

impl EgressProxy {
    /// Where the browser's request to `host:port` connects: the addresses `host` stands for
    /// that the browser may reach (resolved here, so a name cannot mean another address by the
    /// time of the connection), or the text refusing it.
    async fn browser_destination(
        &self,
        browser: &BrowserEgress,
        host: &str,
        port: u16,
        path: Option<&str>,
    ) -> Result<Vec<SocketAddr>, String> {
        let (found, local): (Vec<SocketAddr>, bool) = if let Some(addr) = self.resolve.get(host) {
            // A name's answer, as a resolver gave it (tests).
            (vec![*addr], false)
        } else if is_localhost(host) {
            // Not asked of a resolver: hosts without one for these names exist.
            let both = [
                SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
            ];
            (both.to_vec(), true)
        } else if let Ok(ip) = host.parse() {
            (vec![SocketAddr::new(ip, port)], true)
        } else {
            let found =
                tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port)))
                    .await
                    .map_err(|_| format!("Yulius: resolving {host} timed out"))?
                    .map_err(|_| format!("Yulius: {host} could not be resolved"))?
                    .collect();
            (found, false)
        };
        let reached: Vec<SocketAddr> = found
            .into_iter()
            .filter(|addr| browser.reaches(*addr, path, local))
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
        let Some(host) = request
            .uri()
            .host()
            .map(|host| host.trim_matches(['[', ']']).to_ascii_lowercase())
            .filter(|host| absolute && !host.is_empty())
        else {
            return text(400, "Yulius: expected CONNECT or an absolute http:// URL");
        };
        let port = request.uri().port_u16().unwrap_or(80);
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
        // The host as the origin is to see it: without what an address carries before it
        // (`user:password@`), which is not the proxy's to pass on or to repeat.
        let named = if port == 80 {
            url_host(&host)
        } else {
            format!("{}:{port}", url_host(&host))
        };
        let response = self.send_plain(&addrs, &named, &path, request).await;
        drop(permit);
        response
    }

    /// Sends a plain request on to one of `addrs` and hands the answer back as it comes: the
    /// headers of one hop apart, nothing is added, removed or read. The answer's head is waited
    /// for as an agent's (SPEC-369 PHT-001): the bound restarts with every piece of the
    /// request's body sent on, so a long upload is not cut.
    async fn send_plain<B>(
        &self,
        addrs: &[SocketAddr],
        named: &str,
        path: &str,
        request: Request<B>,
    ) -> Response<EgressBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let unreachable = || text(502, &format!("Yulius: {named} could not be reached"));
        let Ok(Ok(stream)) = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addrs)).await
        else {
            return unreachable();
        };
        let _ = stream.set_nodelay(true);
        let Ok((mut sender, connection)) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
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
        let Ok(host) = HeaderValue::from_str(named) else {
            return text(400, "Yulius: the request names no valid host");
        };
        let progress = Progress::default();
        let sent_on = progress.clone();
        let body = StreamBody::new(
            TryStreamExt::map_err(BodyStream::new(body), Into::<BoxError>::into)
                .inspect_ok(move |_| sent_on.touch()),
        );
        let Ok(onward) = onward.header(header::HOST, host).body(body) else {
            return text(400, "Yulius: the request could not be sent on");
        };
        let answered = progress
            .within(self.response_head_timeout, sender.send_request(onward))
            .await;
        let response = match answered {
            Some(Ok(response)) => response,
            Some(Err(error)) => {
                tracing::debug!(%error, "a browser's plain request failed");
                return unreachable();
            }
            None => return text(504, &format!("Yulius: {named} did not answer")),
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
            // Out of file descriptors, most likely: not a loop that spins meanwhile.
            tokio::time::sleep(ACCEPT_BACKOFF).await;
            continue;
        };
        let (proxy, browser) = (Arc::clone(&proxy), Arc::clone(&browser));
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let (proxy, browser) = (Arc::clone(&proxy), Arc::clone(&browser));
                async move { Ok::<_, Infallible>(proxy.handle_browser(&browser, request).await) }
            });
            // A connection that sends no request for a while is closed (Chrome opens another).
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    fn browser() -> BrowserEgress {
        BrowserEgress {
            sandbox: "s1".into(),
            ports: 20100..=20119,
            pages: Some((
                SocketAddr::from((Ipv4Addr::LOCALHOST, 8081)),
                vec!["/browser/welcome?".into()],
            )),
            listeners: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 8080))],
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
            assert_eq!(
                browser.reaches(addr, None, true),
                reached,
                "tunnel to {addr}"
            );
            assert_eq!(
                browser.reaches(addr, Some("/"), true),
                reached,
                "plain to {addr}"
            );
        }
    }

    /// A name is not an address: what a resolver answered for it is reached only where an
    /// agent run's request is, so no name leads to the loopback, the sandbox's own ports and
    /// the welcome page included (its owner could answer a public address for the page and the
    /// loopback for the page's next request). `localhost` and the names below it are the
    /// loopback by definition.
    #[test]
    fn a_name_that_resolved_to_a_local_address_is_refused() {
        let browser = browser();
        for addr in [
            at(Ipv4Addr::LOCALHOST, 20100),
            at(Ipv6Addr::LOCALHOST, 20105),
            at(Ipv4Addr::LOCALHOST, 8081),
            at(Ipv4Addr::new(169, 254, 169, 254), 80),
        ] {
            assert!(!browser.reaches(addr, None, false), "tunnel to {addr}");
            assert!(
                !browser.reaches(addr, Some("/browser/welcome?sandbox=s1"), false),
                "plain to {addr}"
            );
        }
        assert!(browser.reaches(at(Ipv4Addr::new(93, 184, 216, 34), 443), None, false));
        for (host, local) in [
            ("localhost", true),
            ("app.localhost", true),
            ("localhost.", true),
            ("a.b.localhost", true),
            ("localhost.example", false),
            ("notlocalhost", false),
            ("example.com", false),
        ] {
            assert_eq!(is_localhost(host), local, "{host}");
        }
    }

    /// Yulius's listeners: the internal one gives the browser its welcome page by plain
    /// request and nothing else (no other path, no tunnel, whose path is not seen), also when
    /// it is bound to every address or its port lies in the sandbox's range; the public one
    /// gives nothing at its own address, also with its port in the sandbox's range.
    #[test]
    fn of_yulius_a_browser_gets_its_pages_only() {
        let browser = browser();
        let own = at(Ipv4Addr::LOCALHOST, 8081);
        assert!(browser.reaches(own, Some("/browser/welcome?sandbox=s1"), true));
        for path in ["/browser/welcome", "/internal/version", "/", "/mcp"] {
            assert!(!browser.reaches(own, Some(path), true), "{path}");
        }
        assert!(!browser.reaches(own, None, true), "a tunnel");
        assert!(!browser.reaches(
            at(Ipv6Addr::LOCALHOST, 8081),
            Some("/browser/welcome?"),
            true
        ));

        let everywhere = BrowserEgress {
            ports: 8000..=8999,
            pages: Some((
                at(Ipv4Addr::UNSPECIFIED, 8081),
                vec!["/browser/welcome?".into()],
            )),
            listeners: vec![at(Ipv4Addr::UNSPECIFIED, 8080)],
            ..browser
        };
        for ip in [
            IpAddr::from(Ipv4Addr::LOCALHOST),
            Ipv6Addr::LOCALHOST.into(),
        ] {
            assert!(everywhere.reaches(at(ip, 8081), Some("/browser/welcome?sandbox=s1"), true));
            assert!(!everywhere.reaches(at(ip, 8081), Some("/internal/version"), true));
            assert!(!everywhere.reaches(at(ip, 8081), None, true));
            assert!(
                everywhere.reaches(at(ip, 8082), None, true),
                "a port of the sandbox"
            );
            for path in [None, Some("/")] {
                assert!(
                    !everywhere.reaches(at(ip, 8080), path, true),
                    "the public listener, though its port is in the sandbox's range"
                );
            }
        }
        // A listener at an address of the host's network, named by that address.
        let on_the_network = BrowserEgress {
            listeners: vec![at(Ipv4Addr::new(10, 0, 0, 5), 8080)],
            ..everywhere
        };
        assert!(!on_the_network.reaches(at(Ipv4Addr::new(10, 0, 0, 5), 8080), None, true));
        assert!(on_the_network.reaches(at(Ipv4Addr::new(10, 0, 0, 5), 9090), None, true));
    }

    /// What a refused page reads: the sandbox's ports, or that none is known.
    #[test]
    fn a_refusal_names_the_sandboxes_ports() {
        let known = browser().refusal("::1", 5432);
        assert!(known.contains("does not load [::1]:5432"), "{known}");
        assert!(known.contains("(20100-20119 on localhost"), "{known}");
        #[allow(clippy::reversed_empty_ranges)]
        let unknown = BrowserEgress {
            ports: 1..=0,
            ..browser()
        }
        .refusal("localhost", 3000);
        assert!(
            unknown.contains("it reaches none (its sandbox's ports could not be read)"),
            "{unknown}"
        );
    }
}
