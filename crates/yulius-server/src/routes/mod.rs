//! Route tables.

mod admin;
mod events;
mod sandboxes;
mod setup_auth;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, patch, post};
use yulius_app::context::App;

use crate::AppState;

/// Public API routes.
pub fn public(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/setup/status", get(setup_auth::setup_status))
        .route("/api/setup", post(setup_auth::setup))
        .route("/api/auth/password", post(setup_auth::password_login))
        .route("/api/auth/github/start", get(setup_auth::github_start))
        .route(
            "/api/auth/github/callback",
            get(setup_auth::github_callback),
        )
        .route("/api/auth/logout", post(setup_auth::logout))
        .route("/api/me", get(setup_auth::me))
        .route("/api/device/start", post(setup_auth::device_start))
        .route("/api/device/poll", post(setup_auth::device_poll))
        .route("/api/device/approve", post(setup_auth::device_approve))
        .route(
            "/api/admin/users",
            get(admin::list_users).post(admin::admit_user),
        )
        .route("/api/admin/users/{id}", patch(admin::update_user))
        .route("/api/sandboxes", get(sandboxes::list))
        .route("/api/sandboxes/{sid}", get(sandboxes::get_one))
        .route("/api/events", get(events::events))
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new().route("/internal/healthz", get(|| async { "ok" }))
}
