//! Route tables.

mod admin;
mod browser;
mod editor;
mod events;
mod internal;
mod mcp;
mod sandboxes;
mod setup_auth;
mod shells;
mod tasks;
mod terminal;

use std::sync::Arc;

use axum::Router;
use axum::routing::{any, get, patch, post};
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
        .route(
            "/api/sandboxes/{sid}/tasks",
            get(tasks::list).post(tasks::create),
        )
        .route("/api/tasks/{tid}/stop", post(tasks::stop))
        .route("/api/tasks/{tid}/continue", post(tasks::resume))
        .route("/api/tasks/{tid}/switch", post(tasks::switch))
        .route("/api/archived-tasks", get(tasks::archived))
        .route("/api/tasks/{tid}", get(tasks::detail))
        .route("/api/tasks/{tid}/archive", post(tasks::archive))
        .route("/api/tasks/{tid}/restore", post(tasks::restore))
        .route("/api/tasks/{tid}/feedback", post(tasks::feedback))
        .route(
            "/api/tasks/{tid}/messages",
            get(tasks::messages).post(tasks::post_message),
        )
        .route("/api/tasks/{tid}/terminal", get(terminal::terminal))
        .route("/api/events", get(events::events))
        .route("/api/sandboxes/{sid}/browser", get(browser::browser))
        .route("/api/sandboxes/{sid}/shells", post(shells::create))
        .route("/api/shells/{id}/terminal", get(shells::terminal))
        .route("/s/{sid}/editor", any(editor::redirect_root))
        .route("/s/{sid}/editor/", any(editor::proxy))
        .route("/s/{sid}/editor/{*rest}", any(editor::proxy))
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/internal/healthz", get(|| async { "ok" }))
        .route("/mcp", post(mcp::post).get(mcp::get))
        .route("/hooks/{harness}/{event}", post(internal::hook))
        .route("/internal/git-credential", post(internal::git_credential))
}
