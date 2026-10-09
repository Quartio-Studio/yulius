//! Yulius interface layer: HTTP and WebSocket API (docs/api.md), MCP and hook endpoints,
//! and the embedded web UI. Converts between HTTP and application calls only.

mod assets;
pub mod auth;
pub mod error;
pub mod preview;
mod routes;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use yulius_app::context::App;

/// Maximum JSON request body (SPEC-002 §7.9).
pub const MAX_BODY_BYTES: usize = 1 << 20;

/// Shared handler state.
pub type AppState = Arc<App>;

/// Router for the public listener (UI and API).
pub fn public_router(app: Arc<App>) -> Router {
    let cors = cors_layer(&app.config.allowed_origins);
    let router = routes::public(app.clone())
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(axum::middleware::map_response_with_state(
            app.clone(),
            security_headers,
        ));
    let router = match cors {
        Some(cors) => router.layer(cors),
        None => router,
    };
    // Outermost: preview hosts never reach the UI, the API or their headers (ADR-14).
    router
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            preview::dispatch,
        ))
        .with_state(app)
}

/// Baseline security headers on every public response, unless the route set its own (the
/// sandbox-browser resources keep their stricter policy): nothing may frame Yulius's pages, not
/// even Yulius itself (clickjacking, and no Yulius inside its own browser, BRW-008), no MIME
/// sniffing, no referrer to other origins (#56). HSTS belongs to the TLS proxy
/// (docs/operations.md). `X-Yulius` carries this process's tag, so the preview proxy can refuse
/// to show Yulius itself.
async fn security_headers(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
    mut response: Response,
) -> Response {
    let headers = response.headers_mut();
    if let Ok(tag) = HeaderValue::from_str(&app.instance_tag) {
        headers.insert(
            axum::http::HeaderName::from_static(preview::MARKER_HEADER),
            tag,
        );
    }
    for (name, value) in [
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "same-origin"),
    ] {
        headers
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    }
    response
}

/// CORS for allowlisted frontend origins (bearer-token API use; cookies stay same-site).
fn cors_layer(origins: &[String]) -> Option<tower_http::cors::CorsLayer> {
    use axum::http::{HeaderName, HeaderValue, Method, header};
    let values: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o.trim_end_matches('/')).ok())
        .collect();
    if values.is_empty() {
        return None;
    }
    Some(
        tower_http::cors::CorsLayer::new()
            .allow_origin(tower_http::cors::AllowOrigin::list(values))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static("x-yulius-csrf"),
            ])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

/// Router for the loopback-only internal listener (MCP, hooks, git credentials).
pub fn internal_router(app: Arc<App>) -> Router {
    routes::internal(app.clone())
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app)
}
