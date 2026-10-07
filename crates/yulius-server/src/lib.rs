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
    routes::public(app.clone())
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app)
}

/// Router for the loopback-only internal listener (MCP, hooks, git credentials).
pub fn internal_router(app: Arc<App>) -> Router {
    routes::internal(app.clone())
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app)
}
