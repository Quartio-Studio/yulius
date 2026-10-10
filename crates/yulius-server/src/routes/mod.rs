//! Route tables.

mod accounts;
mod admin;
mod analytics;
mod automations;
mod browser;
mod delivery;
mod events;
mod files;
mod git;
mod imports;
mod internal;
mod mcp;
mod mcp_servers;
mod previews;
mod projects;
mod sandboxes;
mod setup_auth;
mod shells;
mod skills;
mod status;
mod tasks;
mod terminal;
mod workflows;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
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
        .route("/api/auth/recovery", get(setup_auth::recovery_status))
        .route("/api/auth/recover", post(setup_auth::recover))
        .route(
            "/api/admin/recovery-code",
            post(setup_auth::regenerate_recovery),
        )
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
        .route("/api/admin/github", get(admin::github))
        .route("/api/admin/github/manifest", post(admin::github_manifest))
        .route(
            "/api/admin/github/manifest/callback",
            get(admin::github_manifest_callback),
        )
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
        .route("/api/tasks/{tid}", get(tasks::detail).delete(tasks::delete))
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
            "/api/admin/accounts/{id}/usage",
            post(accounts::refresh_usage),
        )
        .route(
            "/api/admin/accounts/login/{lid}/terminal",
            get(accounts::login_terminal),
        )
        .route("/api/analytics/summary", get(analytics::summary))
        .route("/api/analytics/tasks", get(analytics::tasks))
        .route("/api/status", get(status::status))
        .route("/api/capacity", get(status::capacity))
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
        .route("/api/sandboxes/{sid}", patch(delivery::update_sandbox))
        .route("/api/delivery/presets", get(delivery::presets))
        .route("/api/sandboxes/{sid}/git", get(git::overview))
        .route("/api/tasks/{tid}/delivery", get(delivery::task_delivery))
        .route("/api/tasks/{tid}/changes", get(delivery::task_changes))
        .route("/api/tasks/{tid}/commits", get(delivery::task_commits))
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
        .route("/api/sandboxes/{sid}/browser", get(browser::browser))
        .route("/api/sandboxes/{sid}/previews", get(previews::info))
        .route("/api/previews/authorize", get(previews::authorize))
        .route(
            "/api/sandboxes/{sid}/shells",
            get(shells::list).post(shells::create),
        )
        .route(
            "/api/sandboxes/{sid}/shells/{id}",
            patch(shells::rename).delete(shells::remove),
        )
        .route(
            "/api/sandboxes/{sid}/files",
            get(files::list).post(files::create).delete(files::delete),
        )
        .route(
            "/api/sandboxes/{sid}/files/content",
            // JSON-escaped file content up to yulius_app::files::MAX_FILE_BYTES.
            get(files::read)
                .put(files::write)
                .layer(DefaultBodyLimit::max(6 << 20)),
        )
        .route("/api/shells/{id}/terminal", get(shells::terminal))
}

/// Internal (loopback) routes.
pub fn internal(_app: Arc<App>) -> Router<AppState> {
    Router::new()
        .route("/internal/healthz", get(|| async { "ok" }))
        .route("/internal/preview-tls", get(previews::tls_ask))
        .route("/browser/welcome", get(internal::browser_welcome))
        .route(
            "/browser/welcome/ports",
            get(internal::browser_welcome_ports),
        )
        .route("/mcp", post(mcp::post).get(mcp::get))
        .route("/hooks/{harness}/{event}", post(internal::hook))
        .route("/internal/git-credential", post(internal::git_credential))
}
