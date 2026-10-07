//! Route tables.

mod admin;
mod delivery;
mod events;
mod git;
mod mcp;
mod sandboxes;
mod setup_auth;
mod tasks;

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
        .route("/api/events", get(events::events))
        .route("/api/sandboxes/{sid}", patch(delivery::update_sandbox))
        .route("/api/delivery/presets", get(delivery::presets))
        .route("/api/sandboxes/{sid}/git", get(git::overview))
        .route("/api/tasks/{tid}/delivery", get(delivery::task_delivery))
        .route("/api/tasks/{tid}/delivery/pr", post(delivery::open_pr))
        .route("/api/tasks/{tid}/delivery/merge", post(delivery::merge))
        .route(
            "/api/sandboxes/{sid}/delivery/promote",
            post(delivery::promote),
        )
        .route(
            "/api/sandboxes/{sid}/delivery/environments/{env}/runs",
            post(delivery::run_environment),
        )
        .route(
            "/api/sandboxes/{sid}/delivery/environments/{env}/confirm",
            post(delivery::confirm_environment),
        )
        .route("/api/sandboxes/{sid}/delivery/runs", get(delivery::runs))
        .route(
            "/api/sandboxes/{sid}/delivery/runs/{run_id}",
            get(delivery::run),
        )
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/internal/healthz", get(|| async { "ok" }))
        .route("/mcp", post(mcp::post).get(mcp::get))
}
