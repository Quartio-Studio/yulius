//! Route tables.

mod accounts;
mod admin;
mod analytics;
mod events;
mod imports;
mod internal;
mod mcp;
mod projects;
mod sandboxes;
mod setup_auth;
mod status;
mod tasks;
mod terminal;

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
        .route(
            "/api/admin/accounts",
            get(accounts::list).post(accounts::create),
        )
        .route(
            "/api/admin/accounts/{id}",
            patch(accounts::update).delete(accounts::delete),
        )
        .route("/api/admin/accounts/{id}/relogin", post(accounts::relogin))
        .route(
            "/api/admin/accounts/login/{lid}/terminal",
            get(accounts::login_terminal),
        )
        .route("/api/analytics/summary", get(analytics::summary))
        .route("/api/analytics/tasks", get(analytics::tasks))
        .route("/api/status", get(status::status))
        .route("/api/github/repositories", get(projects::repositories))
        .route("/api/templates", get(projects::templates))
        .route("/api/sandboxes", post(projects::create))
        .route(
            "/api/sandboxes/{sid}",
            axum::routing::delete(projects::remove),
        )
        .route(
            "/api/sandboxes/{sid}/onboarding/skip",
            post(projects::skip_onboarding),
        )
        .route(
            "/api/sandboxes/{sid}/onboarding/approve",
            post(projects::approve_onboarding),
        )
        .route("/api/import", post(imports::create))
        .route("/api/import/{job}/files/{name}", imports::chunk_route())
        .route("/api/import/{job}/complete", post(imports::complete))
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/internal/healthz", get(|| async { "ok" }))
        .route("/mcp", post(mcp::post).get(mcp::get))
        .route("/hooks/{harness}/{event}", post(internal::hook))
        .route("/internal/git-credential", post(internal::git_credential))
}
