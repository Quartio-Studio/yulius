//! Yulius interface layer: HTTP and WebSocket API (docs/api.md), MCP and hook endpoints,
//! and the embedded web UI. Converts between HTTP and application calls only.

mod assets;
pub mod auth;
pub mod error;
mod routes;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
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
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    let router = match cors {
        Some(cors) => router.layer(cors),
        None => router,
    };
    router.with_state(app)
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
