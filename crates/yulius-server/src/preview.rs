//! Live previews: the reverse proxy behind every preview origin `<port>-<key>.<domain>`
//! (SPEC-002 ADR-14; SPEC-001 BRW-005, BRW-008; docs/api.md "Live previews").
//!
//! [`dispatch`] runs before everything else on the public listener: a request whose `Host` is a
//! preview host never reaches the Yulius UI or API, and everything else never reaches the
//! proxy. A preview request is then, in order:
//!
//! 1. one of the proxy's own paths under `/__yulius/` (handshake, landing, bridge script,
//!    reachability ping, refusal page), or
//! 2. resolved to its sandbox and port (only the sandbox's range, never Yulius's own ports),
//! 3. checked for cross-site use (fetch metadata and `Origin`: only this preview and the same
//!    sandbox's other previews may make credentialed requests or open WebSockets),
//! 4. checked for a preview grant (cookie); a navigation without one goes through Yulius to get
//!    one (`/api/previews/authorize`, then the handshake), anything else gets 401,
//! 5. forwarded to `127.0.0.1:<port>` over a fresh HTTP/1.1 connection, so that the app believes
//!    it is served at `http://localhost:<port>` (`Host`, `Origin` and `Referer` rewritten), with
//!    the response rewritten so that it works inside the Yulius page: `X-Frame-Options` and CSP
//!    `frame-ancestors` replaced by "this preview and Yulius", `localhost` redirects and CORS
//!    origins pointed at the preview origins, cookie `Domain` removed, and a small bridge script
//!    appended to HTML documents (address, title, back and forward for the browser panel).
//!
//! WebSockets (hot reload) are spliced through and re-checked every 60 s. Bodies stream both
//! ways without buffering. Bounds: 5 s to connect, 120 s for the response head (first compiles
//! are slow), [`MAX_PENDING`] requests waiting for a head and [`MAX_UPGRADED`] open WebSockets.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use yulius_app::preview::{GRANT_TTL_MS, PreviewAccess, PreviewRefusal, PreviewTarget};

use crate::AppState;
use crate::auth::{SESSION_COOKIE, SESSION_COOKIE_SECURE};

/// The preview grant cookie over HTTPS (`__Host-`: host-only, `Secure`, `Path=/`).
pub const ACCESS_COOKIE_SECURE: &str = "__Host-yulius_preview";
/// The preview grant cookie over plain HTTP (local development).
pub const ACCESS_COOKIE: &str = "yulius_preview";
/// Marker header carrying the server's instance tag on every response.
pub const MARKER_HEADER: &str = "x-yulius";
/// Requests waiting for an upstream response head (all previews).
pub const MAX_PENDING: usize = 512;
/// Open WebSockets through previews (all previews).
pub const MAX_UPGRADED: usize = 256;
/// Connection timeout to the dev server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Wait for the dev server's response head.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
/// Grant re-check interval of open WebSockets.
const RECHECK: Duration = Duration::from_secs(60);
/// Appended to proxied HTML documents.
const BRIDGE_TAG: &str = "\n<script src=\"/__yulius/bridge.js\" async></script>\n";

static PENDING: AtomicUsize = AtomicUsize::new(0);
static UPGRADED: AtomicUsize = AtomicUsize::new(0);

/// One slot of a bounded counter, released on drop.
struct Slot(&'static AtomicUsize);

impl Slot {
    fn acquire(counter: &'static AtomicUsize, max: usize) -> Option<Self> {
        counter
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(counter))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Headers that apply to one connection only (RFC 9110 §7.6.1).
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Request headers that describe the outer proxies, never forwarded: the app sees a direct
/// request to `localhost`, and the reverse proxy's shared secret never reaches a sandbox
/// (ACC-013).
const FORWARDING: [&str; 8] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
    crate::client::CLIENT_ADDRESS_HEADER,
    crate::client::PROXY_SECRET_HEADER,
];

/// The `Host` of a request (header, or the URI's authority).
fn host_of(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|a| a.as_str().to_owned()))
}

/// Outermost middleware of the public listener: preview hosts go to the proxy.
pub async fn dispatch(State(app): State<AppState>, request: Request, next: Next) -> Response {
    match host_of(&request) {
        Some(host) if app.is_preview_host(&host) => {
            let mut response = handle(&app, &host, request).await;
            if let Ok(tag) = HeaderValue::from_str(&app.instance_tag) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static(MARKER_HEADER), tag);
            }
            response
        }
        _ => next.run(request).await,
    }
}

/// The grant cookie's name for this instance.
fn access_cookie(app: &AppState) -> &'static str {
    if app.preview_config().is_some_and(|c| c.secure()) {
        ACCESS_COOKIE_SECURE
    } else {
        ACCESS_COOKIE
    }
}

fn cookie_pairs(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            Some((name.trim(), value.trim()))
        })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// A top-level or frame navigation (or, without fetch metadata, a GET for HTML).
fn is_navigation(parts: &Parts) -> bool {
    match header_str(&parts.headers, "sec-fetch-mode") {
        Some(mode) => mode == "navigate",
        None => {
            parts.method == Method::GET
                && header_str(&parts.headers, "accept").is_some_and(|a| a.contains("text/html"))
        }
    }
}

/// Whether the response to this request is a document (the bridge script may be appended).
fn wants_document(parts: &Parts) -> bool {
    if parts.method != Method::GET {
        return false;
    }
    match header_str(&parts.headers, "sec-fetch-dest") {
        Some(dest) => matches!(dest, "document" | "iframe" | "frame"),
        None => is_navigation(parts),
    }
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    header_str(headers, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Cross-site use of a preview (SPEC-002 ADR-14 "isolation"): only this preview and the same
/// sandbox's other previews may send credentialed requests or open WebSockets; every site may
/// navigate to it (the grant cookie then decides). Every other `*.sslip.io` host is same-site
/// with the previews, so `SameSite` cookies alone do not stop them.
fn isolation_allows(parts: &Parts, target: &PreviewTarget) -> bool {
    let headers = &parts.headers;
    let origin_ok = |origin: &str| target.port_of_origin(origin).is_some();
    let origin = header_str(headers, "origin");
    let safe_method = matches!(parts.method, Method::GET | Method::HEAD);
    match header_str(headers, "sec-fetch-site") {
        Some("same-origin" | "none") => true,
        Some("same-site" | "cross-site") => {
            (is_navigation(parts) && safe_method) || origin.is_some_and(origin_ok)
        }
        // Older browsers and non-browser clients: an announced origin must be ours.
        _ => origin.is_none_or(origin_ok),
    }
}

/// A small HTML page on the preview origin (refusals, errors, waiting for a dev server).
fn page(app: &AppState, status: StatusCode, title: &str, text: &str, refresh: bool) -> Response {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let refresh = if refresh {
        "<meta http-equiv=\"refresh\" content=\"3\">"
    } else {
        ""
    };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">{refresh}\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title>\
<style>:root{{color-scheme:light dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;\
font:15px/1.5 system-ui,sans-serif;background:Canvas;color:CanvasText}}main{{max-width:34rem;padding:2rem}}\
h1{{font-size:1.1rem}}p{{opacity:.8}}</style></head><body><main><h1>{title}</h1><p>{text}</p></main>\
<script src=\"/__yulius/bridge.js\" async></script></body></html>",
        title = escape(title),
        text = escape(text),
    );
    let mut response = (status, html).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(csp) = HeaderValue::from_str(&format!(
        "default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; frame-ancestors 'self' {}",
        app.config.public_url
    )) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

fn refusal_page(app: &AppState, refusal: &PreviewRefusal) -> Response {
    let status = match refusal {
        PreviewRefusal::Disabled | PreviewRefusal::UnknownHost => StatusCode::NOT_FOUND,
        PreviewRefusal::OutsideRange { .. } | PreviewRefusal::Yulius => StatusCode::FORBIDDEN,
    };
    let message = refusal.message();
    page(
        app,
        status,
        "This preview is not available",
        &message,
        false,
    )
}

async fn handle(app: &AppState, host: &str, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if let Some(own) = path.strip_prefix("/__yulius/") {
        return own_path(app, host, own, request).await;
    }
    let target = match app.preview_target(host).await {
        Ok(target) => target,
        Err(refusal) => return refusal_page(app, &refusal),
    };
    let (mut parts, body) = request.into_parts();
    if !isolation_allows(&parts, &target) {
        return page(
            app,
            StatusCode::FORBIDDEN,
            "Cross-site request refused",
            "Only this preview and the same sandbox's other previews may call it.",
            false,
        );
    }
    let cookie = access_cookie(app);
    let grant = cookie_pairs(&parts.headers)
        .find(|(name, _)| *name == cookie)
        .map(|(_, value)| value.to_owned());
    let allowed = match &grant {
        Some(grant) => app.preview_access(&target, grant).await == PreviewAccess::Allowed,
        None => false,
    };
    let Some(grant) = grant.filter(|_| allowed) else {
        if is_navigation(&parts) && matches!(parts.method, Method::GET | Method::HEAD) {
            let to = parts.uri.path_and_query().map_or("/", |pq| pq.as_str());
            let location = format!(
                "{}/api/previews/authorize?host={}&to={}",
                app.config.public_url,
                encode(&target.host()),
                encode(to)
            );
            return redirect(StatusCode::FOUND, &location);
        }
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CACHE_CONTROL, "no-store")],
            "Open this preview from Yulius first.",
        )
            .into_response();
    };
    let upgrade = is_upgrade(&parts.headers);
    let client_upgrade = parts.extensions.remove::<OnUpgrade>();
    forward(app, &target, &parts, body, upgrade, client_upgrade, grant).await
}

/// Percent-encodes everything but RFC 3986 unreserved characters and `/`.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn redirect(status: StatusCode, location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => (
            status,
            [
                (header::LOCATION, value),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ReturnQuery {
    #[serde(default)]
    t: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    why: String,
}

/// The proxy's own paths on every preview origin.
async fn own_path(app: &AppState, host: &str, path: &str, request: Request) -> Response {
    let query = Query::<ReturnQuery>::try_from_uri(request.uri())
        .map(|Query(q)| q)
        .unwrap_or(ReturnQuery {
            t: String::new(),
            to: String::new(),
            why: String::new(),
        });
    match path {
        // Reachability of the preview origin (DNS and TLS) for the browser panel.
        "ping" => (
            StatusCode::NO_CONTENT,
            [(header::CACHE_CONTROL, "no-store")],
        )
            .into_response(),
        "bridge.js" => bridge(app),
        "handshake" => handshake(app, host, &query),
        "landing" => landing(app, host, request.headers(), &query).await,
        "denied" => {
            let text = if query.why == "signin" {
                "Sign in to Yulius in this browser, then open the preview again."
            } else {
                "You need write access to this sandbox to open its previews."
            };
            page(
                app,
                StatusCode::FORBIDDEN,
                "Preview not available",
                text,
                false,
            )
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Step 2 of the handshake: trades the token for the grant cookie, then checks it stuck.
fn handshake(app: &AppState, host: &str, query: &ReturnQuery) -> Response {
    let grant = match app.preview_handshake(host, &query.t) {
        Ok(grant) => grant,
        Err(err) => {
            return page(
                app,
                StatusCode::FORBIDDEN,
                "This preview link expired",
                &err.public_message(),
                false,
            );
        }
    };
    let secure = access_cookie(app) == ACCESS_COOKIE_SECURE;
    let cookie = format!(
        "{}={grant}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{}",
        access_cookie(app),
        GRANT_TTL_MS / 1000,
        if secure { "; Secure" } else { "" }
    );
    let to = yulius_app::preview::safe_return_path(&query.to);
    let mut response = redirect(
        StatusCode::SEE_OTHER,
        &format!("/__yulius/landing?to={}", encode(&to)),
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// After the handshake: on to the page when the browser kept the cookie; otherwise say so
/// (cookies of frames blocked) instead of looping through Yulius forever.
async fn landing(app: &AppState, host: &str, headers: &HeaderMap, query: &ReturnQuery) -> Response {
    let to = yulius_app::preview::safe_return_path(&query.to);
    let cookie = access_cookie(app);
    let grant = cookie_pairs(headers)
        .find(|(name, _)| *name == cookie)
        .map(|(_, value)| value.to_owned());
    let kept = match (grant, app.preview_target(host).await) {
        (Some(grant), Ok(target)) => {
            app.preview_access(&target, &grant).await == PreviewAccess::Allowed
        }
        _ => false,
    };
    if kept {
        return redirect(StatusCode::SEE_OTHER, &to);
    }
    page(
        app,
        StatusCode::FORBIDDEN,
        "The preview could not sign in",
        "This browser did not keep the preview's cookie (cookies in frames may be blocked). \
         Open the preview in its own window from the browser panel.",
        false,
    )
}

/// The bridge script appended to proxied documents: tells the browser panel the page's address
/// and title and takes back, forward and reload from it — only between the preview and the
/// Yulius page that frames it.
fn bridge(app: &AppState) -> Response {
    let origin = serde_json::to_string(&app.config.public_url).unwrap_or_else(|_| "\"\"".into());
    let script = format!(
        r"(() => {{
  'use strict';
  const YULIUS = {origin};
  if (window.parent === window || window.parent !== window.top || window.__yuliusBridge) return;
  window.__yuliusBridge = true;
  const nav = window.navigation;
  let sent = '';
  const report = () => {{
    const state = {{ source: 'yulius-preview', type: 'location', href: location.href, title: String(document.title || '').slice(0, 300),
      back: nav ? Boolean(nav.canGoBack) : null, forward: nav ? Boolean(nav.canGoForward) : null }};
    const key = JSON.stringify(state);
    if (key === sent) return;
    sent = key;
    try {{ window.parent.postMessage(state, YULIUS); }} catch (_) {{}}
  }};
  window.addEventListener('message', (event) => {{
    if (event.origin !== YULIUS || event.source !== window.parent) return;
    const data = event.data;
    if (!data || data.source !== 'yulius') return;
    if (data.type === 'back' && nav && nav.canGoBack) nav.back();
    else if (data.type === 'forward' && nav && nav.canGoForward) nav.forward();
    else if (data.type === 'reload') location.reload();
    else if (data.type === 'hello') {{ sent = ''; report(); }}
  }});
  // F6 leaves the page for the next region of Yulius, as everywhere else in the app.
  window.addEventListener('keydown', (event) => {{
    if (event.key !== 'F6' || event.altKey || event.metaKey || event.ctrlKey) return;
    event.preventDefault();
    try {{ window.parent.postMessage({{ source: 'yulius-preview', type: 'f6', shift: event.shiftKey }}, YULIUS); }} catch (_) {{}}
  }}, true);
  if (nav) nav.addEventListener('currententrychange', () => setTimeout(report, 0));
  window.addEventListener('popstate', report);
  window.addEventListener('hashchange', report);
  window.addEventListener('pageshow', report);
  setInterval(report, 1000);
  report();
}})();
"
    );
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        script,
    )
        .into_response()
}

/// Removes hop-by-hop headers and every header named in `Connection`.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    for name in HOP_BY_HOP
        .iter()
        .copied()
        .chain(listed.iter().map(String::as_str))
    {
        headers.remove(name);
    }
}

/// Whether `host` (no port) is the VM's loopback as an app would write it.
fn is_loopback_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "[::1]"
        || host == "0.0.0.0"
        || host
            .strip_prefix("127.")
            .is_some_and(|rest| rest.split('.').count() == 3)
}

/// Maps a sandbox port to its preview origin (`None`: not previewable).
type OriginOf<'a> = &'a dyn Fn(u16) -> Option<String>;

/// Replaces every `http(s)://<loopback>:<port>` and `ws(s)://<loopback>:<port>` of the sandbox's
/// ports in `text` with that port's preview origin (`origin_of`); other text is kept.
pub fn rewrite_loopback_urls(text: &str, secure: bool, origin_of: OriginOf<'_>) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let sep = search + found;
        search = sep + 3;
        let mut start = sep;
        while start > 0 && bytes[start - 1].is_ascii_alphabetic() {
            start -= 1;
        }
        let scheme = text[start..sep].to_ascii_lowercase();
        let websocket = match scheme.as_str() {
            "http" | "https" => false,
            "ws" | "wss" => true,
            _ => continue,
        };
        let rest = &text[sep + 3..];
        let host_len = if rest.starts_with('[') {
            match rest.find(']') {
                Some(end) => end + 1,
                None => continue,
            }
        } else {
            rest.bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'.' || *b == b'-')
                .count()
        };
        let host = &rest[..host_len];
        let after = &rest[host_len..];
        let Some(digits) = after.strip_prefix(':') else {
            continue;
        };
        let port_len = digits.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(port) = digits[..port_len].parse::<u16>() else {
            continue;
        };
        if !is_loopback_host(host) {
            continue;
        }
        let Some(origin) = origin_of(port) else {
            continue;
        };
        let origin = if websocket {
            origin.replacen(
                if secure { "https://" } else { "http://" },
                if secure { "wss://" } else { "ws://" },
                1,
            )
        } else {
            origin
        };
        let end = sep + 3 + host_len + 1 + port_len;
        out.push_str(&text[copied..start]);
        out.push_str(&origin);
        copied = end;
        search = end;
    }
    out.push_str(&text[copied..]);
    out
}

/// A preview origin of this sandbox (with an optional path) as the app knows it:
/// `http://localhost:<port>…`; `port_of` maps a preview origin of the sandbox to its port.
fn to_loopback(value: &str, port_of: &dyn Fn(&str) -> Option<u16>) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..authority_len]);
    let port = port_of(&origin)?;
    Some(format!("http://localhost:{port}{}", &rest[authority_len..]))
}

/// Request headers for the dev server.
fn upstream_headers(
    source: &HeaderMap,
    target: &PreviewTarget,
    drop_cookies: &[&str],
    document: bool,
    upgrade: bool,
) -> HeaderMap {
    let mut headers = source.clone();
    let upgrade_value = headers.get(header::UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    for name in FORWARDING {
        headers.remove(name);
    }
    headers.remove(HeaderName::from_static(MARKER_HEADER));
    if let Ok(host) = HeaderValue::from_str(&format!("localhost:{}", target.port)) {
        headers.insert(header::HOST, host);
    }
    let cookies: Vec<String> = cookie_pairs(source)
        .filter(|(name, _)| !drop_cookies.contains(name))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    headers.remove(header::COOKIE);
    if !cookies.is_empty()
        && let Ok(value) = HeaderValue::from_str(&cookies.join("; "))
    {
        headers.insert(header::COOKIE, value);
    }
    for name in [header::ORIGIN, header::REFERER] {
        let mapped = headers
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| to_loopback(v, &|origin| target.port_of_origin(origin)))
            .and_then(|v| HeaderValue::from_str(&v).ok());
        match mapped {
            Some(value) => {
                headers.insert(name, value);
            }
            None => {
                headers.remove(name);
            }
        }
    }
    if document {
        // Documents come back unencoded so the bridge script can be appended.
        headers.remove(header::ACCEPT_ENCODING);
    }
    if upgrade && let Some(value) = upgrade_value {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, value);
    }
    headers
}

/// A `Set-Cookie` of the app made fit for the preview origin: no `Domain` (the cookie becomes
/// host-only on this preview), `Secure` over HTTPS. Cookies named like Yulius's own are dropped.
fn rewrite_set_cookie(value: &str, secure: bool, reserved: &[&str]) -> Option<String> {
    let mut parts = value.split(';').map(str::trim);
    let pair = parts.next()?;
    let name = pair.split('=').next().unwrap_or_default().trim();
    if name.is_empty() || reserved.contains(&name) {
        return None;
    }
    let mut out = vec![pair.to_owned()];
    let mut has_secure = false;
    for attribute in parts.filter(|p| !p.is_empty()) {
        let key = attribute
            .split('=')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if key == "domain" {
            continue;
        }
        has_secure |= key == "secure";
        out.push(attribute.to_owned());
    }
    if secure && !has_secure {
        out.push("Secure".to_owned());
    }
    Some(out.join("; "))
}

/// A CSP of the app without `frame-ancestors` (Yulius sets its own) and with its loopback
/// sources pointed at the preview origins; `None` when nothing is left.
fn rewrite_csp(value: &str, secure: bool, origin_of: OriginOf<'_>) -> Option<String> {
    let kept: Vec<String> = value
        .split(';')
        .map(str::trim)
        .filter(|directive| {
            !directive.is_empty()
                && !directive
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .map(|directive| rewrite_loopback_urls(directive, secure, origin_of))
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// Response headers for the user's browser.
fn client_headers(
    source: &HeaderMap,
    target: &PreviewTarget,
    app: &AppState,
    switching: bool,
) -> HeaderMap {
    let secure = target.config.secure();
    let origin_of = |port: u16| target.origin_of(port);
    let mut headers = source.clone();
    let upgrade = headers.get(header::UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    if switching && let Some(value) = upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, value);
    }
    for name in [
        header::X_FRAME_OPTIONS,
        header::STRICT_TRANSPORT_SECURITY,
        HeaderName::from_static("alt-svc"),
        HeaderName::from_static(MARKER_HEADER),
    ] {
        headers.remove(name);
    }
    for name in [
        header::LOCATION,
        header::CONTENT_LOCATION,
        header::REFRESH,
        header::LINK,
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
    ] {
        let rewritten: Vec<HeaderValue> = headers
            .get_all(&name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| {
                HeaderValue::from_str(&rewrite_loopback_urls(v, secure, &origin_of)).ok()
            })
            .collect();
        if !rewritten.is_empty() {
            headers.remove(&name);
            for value in rewritten {
                headers.append(&name, value);
            }
        }
    }
    for name in [
        header::CONTENT_SECURITY_POLICY,
        header::CONTENT_SECURITY_POLICY_REPORT_ONLY,
    ] {
        let rewritten: Vec<HeaderValue> = headers
            .get_all(&name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| rewrite_csp(v, secure, &origin_of))
            .filter_map(|v| HeaderValue::from_str(&v).ok())
            .collect();
        headers.remove(&name);
        for value in rewritten {
            headers.append(&name, value);
        }
    }
    if let Ok(value) =
        HeaderValue::from_str(&format!("frame-ancestors 'self' {}", app.config.public_url))
    {
        headers.append(header::CONTENT_SECURITY_POLICY, value);
    }
    let reserved = [
        ACCESS_COOKIE,
        ACCESS_COOKIE_SECURE,
        SESSION_COOKIE,
        SESSION_COOKIE_SECURE,
    ];
    let cookies: Vec<HeaderValue> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| rewrite_set_cookie(v, secure, &reserved))
        .filter_map(|v| HeaderValue::from_str(&v).ok())
        .collect();
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        headers.append(header::SET_COOKIE, cookie);
    }
    headers
}

fn upstream_error(
    app: &AppState,
    target: &PreviewTarget,
    document: bool,
    detail: &str,
) -> Response {
    if document {
        return page(
            app,
            StatusCode::BAD_GATEWAY,
            &format!("Nothing is answering on localhost:{}", target.port),
            &format!(
                "Start a dev server on port {} in this sandbox (for example PORT={} npm run dev). \
                 This page retries every few seconds. ({detail})",
                target.port, target.port
            ),
            true,
        );
    }
    (StatusCode::BAD_GATEWAY, detail.to_owned()).into_response()
}

async fn forward(
    app: &AppState,
    target: &PreviewTarget,
    parts: &Parts,
    body: Body,
    upgrade: bool,
    client_upgrade: Option<OnUpgrade>,
    grant: String,
) -> Response {
    let document = wants_document(parts);
    let Some(pending) = Slot::acquire(&PENDING, MAX_PENDING) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many preview requests").into_response();
    };
    let upgraded_slot = if upgrade {
        match Slot::acquire(&UPGRADED, MAX_UPGRADED) {
            Some(slot) => Some(slot),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many open preview connections",
                )
                    .into_response();
            }
        }
    } else {
        None
    };
    let stream = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(("127.0.0.1", target.port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => return upstream_error(app, target, document, &err.to_string()),
        Err(_) => return upstream_error(app, target, document, "connection timed out"),
    };
    let _ = stream.set_nodelay(true);
    let (mut sender, connection) =
        match hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await {
            Ok(pair) => pair,
            Err(err) => return upstream_error(app, target, document, &err.to_string()),
        };
    tokio::spawn(async move {
        if let Err(err) = connection.with_upgrades().await {
            tracing::debug!(error = %err, "preview upstream connection ended with an error");
        }
    });
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |pq| pq.as_str())
        .parse::<Uri>()
        .unwrap_or_else(|_| Uri::from_static("/"));
    let reserved = [
        access_cookie(app),
        ACCESS_COOKIE,
        ACCESS_COOKIE_SECURE,
        SESSION_COOKIE,
        SESSION_COOKIE_SECURE,
    ];
    let mut request = hyper::Request::new(if upgrade { Body::empty() } else { body });
    *request.method_mut() = parts.method.clone();
    *request.uri_mut() = path;
    *request.headers_mut() = upstream_headers(&parts.headers, target, &reserved, document, upgrade);
    let mut response =
        match tokio::time::timeout(RESPONSE_TIMEOUT, sender.send_request(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => return upstream_error(app, target, document, &err.to_string()),
            Err(_) => return upstream_error(app, target, document, "no answer in time"),
        };
    drop(pending);
    if header_str(response.headers(), MARKER_HEADER) == Some(app.instance_tag.as_str()) {
        tracing::info!(port = target.port, "refused a preview of Yulius itself");
        return page(
            app,
            StatusCode::LOOP_DETECTED,
            "Yulius cannot be opened inside itself",
            "This port leads back to this Yulius server. Open Yulius in a tab of your own browser instead.",
            false,
        );
    }
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let (Some(client_upgrade), Some(slot)) = (client_upgrade, upgraded_slot) else {
            return (
                StatusCode::BAD_GATEWAY,
                "the dev server switched protocols unexpectedly",
            )
                .into_response();
        };
        let upstream_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(splice(
            app.clone(),
            grant,
            client_upgrade,
            upstream_upgrade,
            slot,
        ));
        let mut out = Response::new(Body::empty());
        *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        *out.headers_mut() = client_headers(response.headers(), target, app, true);
        return out;
    }
    let status = response.status();
    let mut headers = client_headers(response.headers(), target, app, false);
    let html = header_str(&headers, "content-type")
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
    let encoded = header_str(&headers, "content-encoding")
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("identity"));
    let inject = document
        && html
        && !encoded
        && !matches!(
            status,
            StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED | StatusCode::RESET_CONTENT
        );
    let body = Body::new(response.into_body());
    let body = if inject {
        headers.remove(header::CONTENT_LENGTH);
        let tail = futures::stream::once(async {
            Ok::<_, axum::Error>(axum::body::Bytes::from_static(BRIDGE_TAG.as_bytes()))
        });
        Body::from_stream(body.into_data_stream().chain(tail))
    } else {
        body
    };
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    out
}

/// Copies bytes between the upgraded browser and dev-server connections until either side
/// closes or the grant is revoked.
async fn splice(app: AppState, grant: String, client: OnUpgrade, upstream: OnUpgrade, _slot: Slot) {
    let (client, upstream) = tokio::join!(client, upstream);
    let (Ok(client), Ok(upstream)) = (client, upstream) else {
        tracing::debug!("preview WebSocket upgrade did not complete");
        return;
    };
    let mut client = TokioIo::new(client);
    let mut upstream = TokioIo::new(upstream);
    let copy = tokio::io::copy_bidirectional(&mut client, &mut upstream);
    tokio::pin!(copy);
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    loop {
        tokio::select! {
            _ = &mut copy => return,
            _ = recheck.tick() => {
                if !app.preview_grant_still_valid(&grant).await {
                    tracing::info!("closing a preview WebSocket: access ended");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use yulius_app::preview::PreviewConfig;

    use super::*;

    const KEY: &str = "0123456789ab";

    fn config() -> Result<PreviewConfig, Box<dyn std::error::Error>> {
        Ok(PreviewConfig::new("https", "yulius.example.com", None).ok_or("config")?)
    }

    /// The sandbox's ports 20000-20019 with 20010 belonging to Yulius.
    fn origin_of(config: &PreviewConfig) -> impl Fn(u16) -> Option<String> + '_ {
        move |port| {
            ((20000..=20019).contains(&port) && port != 20010).then(|| config.origin(port, KEY))
        }
    }

    #[test]
    fn loopback_urls_of_the_sandbox_become_preview_origins()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = config()?;
        let origin_of = origin_of(&config);
        let rewrite = |text: &str| rewrite_loopback_urls(text, true, &origin_of);
        let p1 = "https://20001-0123456789ab.yulius.example.com";
        let p2 = "https://20002-0123456789ab.yulius.example.com";
        assert_eq!(rewrite("http://localhost:20001/a?b"), format!("{p1}/a?b"));
        assert_eq!(rewrite("http://127.0.0.1:20002"), p2);
        assert_eq!(
            rewrite("connect-src 'self' ws://localhost:20001 http://[::1]:20002/x"),
            format!("connect-src 'self' wss://20001-0123456789ab.yulius.example.com {p2}/x")
        );
        assert_eq!(
            rewrite("</a>; rel=preload, <http://app.localhost:20001/b>"),
            format!("</a>; rel=preload, <{p1}/b>")
        );
        for kept in [
            "http://localhost:3000/",
            "http://localhost:20010/",
            "https://example.com:20001/",
            "http://localhost/",
            "ftp://localhost:20001/",
            "http://localhost:200019/",
            "not a url ://",
        ] {
            assert_eq!(rewrite(kept), kept, "{kept}");
        }
        let port_of = |origin: &str| {
            let host = origin.strip_prefix("https://")?;
            let (port, key) = config.parse_host(host)?;
            (key == KEY).then_some(port)
        };
        assert_eq!(
            to_loopback(&format!("{p2}/page?x=1"), &port_of).as_deref(),
            Some("http://localhost:20002/page?x=1")
        );
        assert_eq!(to_loopback("https://evil.example/", &port_of), None);
        Ok(())
    }

    #[test]
    fn cookies_lose_their_domain_and_reserved_names() {
        let reserved = [ACCESS_COOKIE, ACCESS_COOKIE_SECURE, SESSION_COOKIE];
        assert_eq!(
            rewrite_set_cookie("sid=1; Domain=localhost; Path=/; HttpOnly", true, &reserved)
                .as_deref(),
            Some("sid=1; Path=/; HttpOnly; Secure")
        );
        assert_eq!(
            rewrite_set_cookie("sid=1; secure; SameSite=None", true, &reserved).as_deref(),
            Some("sid=1; secure; SameSite=None")
        );
        assert_eq!(
            rewrite_set_cookie("sid=1", false, &reserved).as_deref(),
            Some("sid=1")
        );
        assert_eq!(
            rewrite_set_cookie("yulius_session=x; Path=/", true, &reserved),
            None
        );
        assert_eq!(
            rewrite_set_cookie("__Host-yulius_preview=x", true, &reserved),
            None
        );
    }

    #[test]
    fn csp_loses_frame_ancestors_only() -> Result<(), Box<dyn std::error::Error>> {
        let config = config()?;
        let origin_of = origin_of(&config);
        assert_eq!(
            rewrite_csp(
                "default-src 'self'; frame-ancestors 'none'; script-src http://localhost:20001",
                true,
                &origin_of
            )
            .as_deref(),
            Some("default-src 'self'; script-src https://20001-0123456789ab.yulius.example.com")
        );
        assert_eq!(
            rewrite_csp("FRAME-ANCESTORS 'self'", true, &origin_of),
            None
        );
        Ok(())
    }

    #[test]
    fn hop_by_hop_and_connection_listed_headers_are_removed() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("close, X-Custom-Hop"),
        );
        headers.insert("x-custom-hop", HeaderValue::from_static("1"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
        strip_hop_by_hop(&mut headers);
        let names: Vec<&str> = headers.keys().map(HeaderName::as_str).collect();
        assert_eq!(names, ["content-type"]);
    }

    #[test]
    fn loopback_hosts_are_recognized() {
        for host in [
            "localhost",
            "LOCALHOST",
            "app.localhost",
            "127.0.0.1",
            "127.1.2.3",
            "[::1]",
            "0.0.0.0",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "127.0.0.1.example.com",
            "10.0.0.1",
            "localhost.example.com",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }
}
