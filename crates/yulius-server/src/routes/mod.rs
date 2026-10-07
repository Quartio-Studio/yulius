//! Route tables.

mod admin;
mod automations;
mod events;
mod internal;
mod mcp;
mod mcp_servers;
mod sandboxes;
mod setup_auth;
mod skills;
mod tasks;
mod terminal;
mod workflows;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, patch, post, put};
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
            "/api/sandboxes/{sid}/workflows",
            get(workflows::list).post(workflows::create),
        )
        .route(
            "/api/workflows/{id}",
            put(workflows::update).delete(workflows::delete),
        )
        .route(
            "/api/sandboxes/{sid}/automations",
            get(automations::list).post(automations::create),
        )
        .route(
            "/api/automations/{id}",
            put(automations::update).delete(automations::delete),
        )
        .route("/api/automations/{id}/run", post(automations::run_now))
        .route("/api/automations/{id}/runs", get(automations::runs))
        .route("/api/sandboxes/{sid}/skills", get(skills::list))
        .route(
            "/api/sandboxes/{sid}/skills/{name}",
            get(skills::content).put(skills::set_enabled),
        )
        .route(
            "/api/sandboxes/{sid}/mcp",
            get(mcp_servers::list).post(mcp_servers::create),
        )
        .route(
            "/api/mcp/{id}",
            put(mcp_servers::update).delete(mcp_servers::delete),
        )
        .route("/api/mcp/{id}/test", post(mcp_servers::test))
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/internal/healthz", get(|| async { "ok" }))
        .route("/mcp", post(mcp::post).get(mcp::get))
        .route("/hooks/{harness}/{event}", post(internal::hook))
        .route("/internal/git-credential", post(internal::git_credential))
}
