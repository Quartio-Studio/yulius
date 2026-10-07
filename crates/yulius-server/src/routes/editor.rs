//! `/s/{sid}/editor/*`: reverse proxy to the sandbox's openvscode-server, HTTP and WebSocket
//! upgrades (docs/api.md; SPEC-001 EDT-001/002, AC-EDT-1; SPEC-002 ADR-10).
//!
//! - Only users with write access pass; the session cookie or bearer token is checked per
//!   request and open WebSockets are re-checked every 60 s. State-changing requests and
//!   upgrades must be same-origin (VS Code cannot send the Yulius CSRF header).
//! - The connection token is added as a cookie to every forwarded request. The browser never
//!   sees it: client `vscode-tkn` cookies are replaced, upstream `Set-Cookie` for it is dropped.
//!   Yulius credentials (session cookie, `Authorization`, CSRF header) are not forwarded.
//! - Hop-by-hop headers are stripped both ways; request bodies are capped at
//!   [`MAX_REQUEST_BODY`]; connecting and waiting for the response head are bounded.
//! - `?worktree=<task id>` on the editor root redirects to `?folder=<worktree>` (only worktrees
//!   of that sandbox). A missing or disabled openvscode-server is a 503 with a clear message.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use http_body_util::Limited;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use yulius_app::auth::AuthContext;
use yulius_app::editor::{EditorDenied, EditorRoute};
use yulius_app::error::AppError;
use yulius_app::ports::editor::EditorEndpoint;
use yulius_contracts::api::common::{ErrorBody, ErrorDetail};

use super::events::check_origin;
use crate::AppState;
use crate::auth::{CSRF_HEADER, SESSION_COOKIE, cookie_value};
use crate::error::ApiError;

/// Largest request body forwarded to the editor.
pub const MAX_REQUEST_BODY: usize = 8 * 1024 * 1024;
/// Connection timeout to the loopback editor.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Wait for the response head of a forwarded request.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// Permission re-check and idle-touch interval of upgraded connections.
const RECHECK: Duration = Duration::from_secs(60);
/// Concurrently proxied editor WebSockets (all sandboxes).
pub const MAX_UPGRADED: usize = 256;
/// Cookie carrying the editor's connection token.
const TOKEN_COOKIE: &str = "vscode-tkn";

static UPGRADED: AtomicUsize = AtomicUsize::new(0);

/// Holds one slot of [`MAX_UPGRADED`].
struct UpgradeSlot;

impl UpgradeSlot {
    fn acquire() -> Option<Self> {
        UPGRADED
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_UPGRADED).then_some(n + 1)
            })
            .ok()
            .map(|_| Self)
    }
}

impl Drop for UpgradeSlot {
    fn drop(&mut self) {
        UPGRADED.fetch_sub(1, Ordering::SeqCst);
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

fn valid_sandbox_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn error_response(status: StatusCode, code: &str, message: String) -> Response {
    let body = ErrorBody {
        error: ErrorDetail {
            code: code.to_owned(),
            message,
        },
    };
    (status, Json(body)).into_response()
}

/// `/s/{sid}/editor` → `/s/{sid}/editor/` (the editor serves under a trailing slash).
pub async fn redirect_root(Path(sid): Path<String>, RawQuery(query): RawQuery) -> Response {
    if !valid_sandbox_id(&sid) {
        return ApiError(AppError::NotFound).into_response();
    }
    let query = query.map(|q| format!("?{q}")).unwrap_or_default();
    let location = format!("/s/{sid}/editor/{query}");
    match HeaderValue::from_str(&location) {
        Ok(value) => (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, value)]).into_response(),
        Err(_) => ApiError(AppError::NotFound).into_response(),
    }
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

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Same-origin check for state-changing requests: `Origin` equals the public URL, or the
/// browser marks the request `Sec-Fetch-Site: same-origin`.
fn same_origin(app: &AppState, headers: &HeaderMap) -> bool {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let fetch_site = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok());
    origin.is_some_and(|o| o.trim_end_matches('/') == app.config.public_url)
        || fetch_site == Some("same-origin")
}

async fn authenticate(app: &AppState, parts: &Parts) -> Result<AuthContext, AppError> {
    if let Some(value) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let token = value
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthenticated)?;
        return app
            .authenticate_bearer(token.trim())
            .await?
            .ok_or(AppError::Unauthenticated);
    }
    let cookie = cookie_value(parts, SESSION_COOKIE).ok_or(AppError::Unauthenticated)?;
    let ctx = app
        .authenticate_cookie(&cookie)
        .await?
        .ok_or(AppError::Unauthenticated)?;
    let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe && !same_origin(app, &parts.headers) {
        return Err(AppError::Forbidden(
            "cross-site editor request refused".into(),
        ));
    }
    Ok(ctx)
}

/// `worktree` query value, only on the editor root.
fn worktree_param(parts: &Parts, sid: &str) -> Option<String> {
    if parts.method != Method::GET || parts.uri.path() != format!("/s/{sid}/editor/") {
        return None;
    }
    parts.uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "worktree" && !value.is_empty()).then(|| value.to_owned())
    })
}

/// The `Cookie` header for upstream: client cookies minus Yulius' session and any client
/// supplied token, plus the real token.
fn upstream_cookie(headers: &HeaderMap, endpoint: &EditorEndpoint) -> Option<HeaderValue> {
    let mut pairs: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .map(str::trim)
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or_default().trim();
            !pair.is_empty() && name != SESSION_COOKIE && name != TOKEN_COOKIE
        })
        .map(str::to_owned)
        .collect();
    pairs.push(endpoint.cookie.expose().to_owned());
    HeaderValue::from_str(&pairs.join("; ")).ok()
}

/// Headers forwarded upstream.
fn upstream_headers(source: &HeaderMap, endpoint: &EditorEndpoint, upgrade: bool) -> HeaderMap {
    let mut headers = source.clone();
    let upgrade_value = headers.get(header::UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    for name in [header::AUTHORIZATION, header::COOKIE] {
        headers.remove(name);
    }
    headers.remove(CSRF_HEADER);
    if let Some(cookie) = upstream_cookie(source, endpoint) {
        headers.insert(header::COOKIE, cookie);
    }
    if upgrade && let Some(value) = upgrade_value {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, value);
    }
    if !headers.contains_key(header::HOST)
        && let Ok(host) = HeaderValue::from_str(&format!("127.0.0.1:{}", endpoint.port))
    {
        headers.insert(header::HOST, host);
    }
    headers
}

/// Response headers for the client: hop-by-hop removed (kept for a 101), token cookies dropped.
fn client_headers(source: &HeaderMap, switching: bool) -> HeaderMap {
    let mut headers = source.clone();
    let upgrade = headers.get(header::UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    if switching && let Some(value) = upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, value);
    }
    let cookies: Vec<HeaderValue> = headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter(|value| {
            let name = value
                .to_str()
                .ok()
                .and_then(|v| v.split(['=', ';']).next())
                .map(str::trim)
                .unwrap_or_default();
            name != TOKEN_COOKIE && name != SESSION_COOKIE
        })
        .cloned()
        .collect();
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        headers.append(header::SET_COOKIE, cookie);
    }
    headers
}

/// Proxies `/s/{sid}/editor/` and `/s/{sid}/editor/{*rest}`.
pub async fn proxy(
    State(app): State<AppState>,
    Path(params): Path<HashMap<String, String>>,
    request: Request,
) -> Response {
    let Some(sid) = params.get("sid").cloned() else {
        return ApiError(AppError::NotFound).into_response();
    };
    let (mut parts, body) = request.into_parts();
    let ctx = match authenticate(&app, &parts).await {
        Ok(ctx) => ctx,
        Err(err) => return ApiError(err).into_response(),
    };
    let upgrade = is_upgrade(&parts.headers);
    if upgrade && let Err(err) = check_origin(&app, &parts.headers) {
        return ApiError(err).into_response();
    }
    let worktree = worktree_param(&parts, &sid);
    let endpoint = match app.editor_route(&ctx, &sid, worktree.as_deref()).await {
        Ok(EditorRoute::Proxy(endpoint)) => endpoint,
        Ok(EditorRoute::Redirect(location)) => {
            return match HeaderValue::from_str(&location) {
                Ok(value) => (StatusCode::FOUND, [(header::LOCATION, value)]).into_response(),
                Err(_) => ApiError(AppError::internal("unencodable redirect")).into_response(),
            };
        }
        Err(EditorDenied::App(err)) => return ApiError(err).into_response(),
        Err(EditorDenied::Unavailable(message)) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message);
        }
    };
    let too_large = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|len| len > MAX_REQUEST_BODY);
    if too_large {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid",
            format!("request body exceeds {MAX_REQUEST_BODY} bytes"),
        );
    }
    let client_upgrade = parts.extensions.remove::<OnUpgrade>();
    let slot = if upgrade {
        match UpgradeSlot::acquire() {
            Some(slot) => Some(slot),
            None => {
                return ApiError(AppError::Capacity(
                    "too many open editor connections".into(),
                ))
                .into_response();
            }
        }
    } else {
        None
    };
    match forward(&parts, body, &endpoint, upgrade).await {
        Ok(mut response) => {
            if response.status() == StatusCode::SWITCHING_PROTOCOLS {
                let (Some(client_upgrade), Some(slot)) = (client_upgrade, slot) else {
                    return ApiError(AppError::Upstream(
                        "the editor switched protocols unexpectedly".into(),
                    ))
                    .into_response();
                };
                let upstream_upgrade = hyper::upgrade::on(&mut response);
                tokio::spawn(splice(
                    app.clone(),
                    ctx,
                    sid,
                    client_upgrade,
                    upstream_upgrade,
                    slot,
                ));
                let headers = client_headers(response.headers(), true);
                let mut out = Response::new(Body::empty());
                *out.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                *out.headers_mut() = headers;
                return out;
            }
            let status = response.status();
            let headers = client_headers(response.headers(), false);
            let mut out = Response::new(Body::new(response.into_body()));
            *out.status_mut() = status;
            *out.headers_mut() = headers;
            out
        }
        Err(err) => ApiError(err).into_response(),
    }
}

/// Sends the request to the editor over a fresh loopback HTTP/1.1 connection.
async fn forward(
    parts: &Parts,
    body: Body,
    endpoint: &EditorEndpoint,
    upgrade: bool,
) -> Result<hyper::Response<hyper::body::Incoming>, AppError> {
    let unreachable =
        |detail: String| AppError::Upstream(format!("the editor is not reachable: {detail}"));
    let stream = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(("127.0.0.1", endpoint.port)),
    )
    .await
    .map_err(|_| unreachable("connection timed out".into()))?
    .map_err(|err| unreachable(err.to_string()))?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake::<_, Limited<Body>>(TokioIo::new(stream))
            .await
            .map_err(|err| unreachable(err.to_string()))?;
    tokio::spawn(async move {
        if let Err(err) = connection.with_upgrades().await {
            tracing::debug!(error = %err, "editor upstream connection ended with an error");
        }
    });
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |pq| pq.as_str())
        .parse::<Uri>()
        .map_err(AppError::internal)?;
    let body = if upgrade {
        Limited::new(Body::empty(), 0)
    } else {
        Limited::new(body, MAX_REQUEST_BODY)
    };
    let mut request = hyper::Request::new(body);
    *request.method_mut() = parts.method.clone();
    *request.uri_mut() = path;
    *request.headers_mut() = upstream_headers(&parts.headers, endpoint, upgrade);
    tokio::time::timeout(RESPONSE_TIMEOUT, sender.send_request(request))
        .await
        .map_err(|_| AppError::Upstream("the editor did not answer in time".into()))?
        .map_err(|err| unreachable(err.to_string()))
}

/// Copies bytes between the upgraded client and editor connections until either side closes
/// or the viewer loses write access; keeps the editor from idling out meanwhile.
async fn splice(
    app: AppState,
    ctx: AuthContext,
    sid: String,
    client: OnUpgrade,
    upstream: OnUpgrade,
    _slot: UpgradeSlot,
) {
    let (client, upstream) = tokio::join!(client, upstream);
    let (Ok(client), Ok(upstream)) = (client, upstream) else {
        tracing::debug!("editor WebSocket upgrade did not complete");
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
                app.editor_touch(&sid);
                if !app.editor_still_allowed(&ctx, &sid).await {
                    tracing::info!(sandbox = %sid, "closing an editor connection: access revoked");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::header::HeaderName;

    use super::*;

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
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
        strip_hop_by_hop(&mut headers);
        let names: Vec<&str> = headers.keys().map(HeaderName::as_str).collect();
        assert_eq!(names, ["content-type"]);
    }

    #[test]
    fn sandbox_ids_in_redirects_are_validated() {
        assert!(valid_sandbox_id("sbx_1-a"));
        assert!(!valid_sandbox_id(""));
        assert!(!valid_sandbox_id("a/b"));
        assert!(!valid_sandbox_id("a\r\nb"));
        assert!(!valid_sandbox_id(&"a".repeat(65)));
    }
}
