//! Request authentication: session cookie (with CSRF on state-changing methods) or bearer token.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Method, header};
use yulius_app::auth::AuthContext;
use yulius_app::error::AppError;
use yulius_app::playground::GUEST_FENCE;

use crate::AppState;
use crate::error::ApiError;

/// Session cookie name over plain HTTP (local development).
pub const SESSION_COOKIE: &str = "yulius_session";
/// Session cookie name over HTTPS. The `__Host-` prefix makes browsers refuse the cookie from
/// any other host, so no sibling host (a preview origin, or any other `*.sslip.io` site, which
/// shares the registrable domain) can plant or shadow it (SPEC-002 ADR-14).
pub const SESSION_COOKIE_SECURE: &str = "__Host-yulius_session";

/// The session cookie's name for an instance.
#[must_use]
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE_SECURE
    } else {
        SESSION_COOKIE
    }
}
/// CSRF header name.
pub const CSRF_HEADER: &str = "x-yulius-csrf";

/// Extracts a cookie value from the `Cookie` header.
#[must_use]
pub fn cookie_value(parts: &Parts, name: &str) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|raw| {
            raw.split(';').find_map(|pair| {
                let (k, v) = pair.trim().split_once('=')?;
                (k == name).then(|| v.to_owned())
            })
        })
}

/// An authenticated request.
pub struct Auth(pub AuthContext);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(value) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            let token = value
                .strip_prefix("Bearer ")
                .ok_or(AppError::Unauthenticated)?;
            let ctx = app
                .authenticate_bearer(token.trim())
                .await?
                .ok_or(AppError::Unauthenticated)?;
            // A read-only token reads (SPEC-247 RMT-002): no state-changing method, and no
            // WebSocket, whose GET opens a channel that writes (a terminal types into a task).
            if ctx.read_only() && (!is_safe(&parts.method) || is_upgrade(parts)) {
                return Err(AppError::Forbidden(
                    "this API token is read-only; make an operate token on Settings to change things"
                        .into(),
                )
                .into());
            }
            refuse_guests_outside(app, parts, &ctx).await?;
            return Ok(Self(ctx));
        }
        let cookie = cookie_value(parts, session_cookie_name(app.config.secure_cookies))
            .ok_or(AppError::Unauthenticated)?;
        refuse_foreign_sites(app, parts)?;
        let ctx = app
            .authenticate_cookie(&cookie)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        if !is_safe(&parts.method) {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if ctx.csrf_token.as_deref() != Some(sent) || sent.is_empty() {
                return Err(AppError::CsrfInvalid.into());
            }
        }
        refuse_guests_outside(app, parts, &ctx).await?;
        Ok(Self(ctx))
    }
}

/// A playground's guest reaches only the routes of [`guest_may_reach`] (SPEC-410 GST-004);
/// access then decides which sandbox, and `App::require` refuses them its configuration.
async fn refuse_guests_outside(
    app: &AppState,
    parts: &Parts,
    ctx: &AuthContext,
) -> Result<(), AppError> {
    if guest_may_reach(&parts.method, parts.uri.path()) || !app.is_guest(&ctx.user).await {
        return Ok(());
    }
    Err(AppError::Forbidden(GUEST_FENCE.into()))
}

/// The pages of a sandbox a guest may use: its tasks and what working on them needs. Never its
/// secrets, variables, MCP servers, skills, automations, workflows, saved agent states, Superagent,
/// guest links or delivery settings.
const GUEST_SANDBOX_PAGES: &[&str] = &[
    "attachments",
    "browser",
    "ci",
    "files",
    "git",
    "intake",
    "notes",
    "previews",
    "sessions",
    "shells",
    "stages",
    "tasks",
];

/// Whether a playground's guest may send `method` to `path` (an allowlist, so a route added later
/// stays closed to guests until it is listed): their account (`/api/me`, sign-out, their own notes),
/// the event stream, the sandbox list, a sandbox and its [`GUEST_SANDBOX_PAGES`], tasks,
/// sessions and shells (each decided by the sandbox's access) and preview authorization.
#[must_use]
pub fn guest_may_reach(method: &Method, path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    match path {
        "/api/me"
        | "/api/me/notes"
        | "/api/auth/logout"
        | "/api/events"
        | "/api/archived-tasks"
        | "/api/previews/authorize" => return true,
        "/api/sandboxes" => return *method == Method::GET,
        _ => {}
    }
    if ["/api/tasks/", "/api/sessions/", "/api/shells/"]
        .iter()
        .any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty())
        })
    {
        return true;
    }
    let Some(rest) = path.strip_prefix("/api/sandboxes/") else {
        return false;
    };
    let mut segments = rest.split('/');
    match (segments.next(), segments.next()) {
        (Some(sid), None) => !sid.is_empty(),
        (Some(sid), Some(page)) => !sid.is_empty() && GUEST_SANDBOX_PAGES.contains(&page),
        _ => false,
    }
}

/// Whether a request asks to switch protocols (a WebSocket).
fn is_upgrade(parts: &Parts) -> bool {
    parts.headers.contains_key(header::UPGRADE)
}

/// Whether a request only reads.
fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// A cookie session may be used only by Yulius's own pages (SPEC-002 ADR-14, §8): a request
/// announcing another origin, or one the browser marks as coming from another site (a preview
/// origin or any other host of the same registrable domain is "same-site"), is refused unless
/// it is a plain navigation. Bearer tokens are not affected.
fn refuse_foreign_sites(app: &AppState, parts: &Parts) -> Result<(), AppError> {
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
    let navigation = header("sec-fetch-mode") == Some("navigate")
        && matches!(parts.method, Method::GET | Method::HEAD);
    if matches!(header("sec-fetch-site"), Some("same-site" | "cross-site")) && !navigation {
        return Err(AppError::Forbidden(
            "requests from other sites cannot use a Yulius session".into(),
        ));
    }
    if let Some(origin) = header("origin")
        && !app.origin_allowed(origin)
    {
        return Err(AppError::Forbidden(
            "requests from other origins cannot use a Yulius session".into(),
        ));
    }
    Ok(())
}

/// Builds the `Set-Cookie` value for a session.
#[must_use]
pub fn session_cookie(value: &str, max_age_s: i64, secure: bool) -> String {
    let name = session_cookie_name(secure);
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_s}{secure}")
}

/// Builds a `Set-Cookie` value that clears the session.
#[must_use]
pub fn clear_cookie(secure: bool) -> String {
    session_cookie("", 0, secure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guest_reaches_only_their_playgrounds_work() {
        let get = |path: &str| guest_may_reach(&Method::GET, path);
        for path in [
            "/api/me",
            "/api/me/notes",
            "/api/events",
            "/api/archived-tasks",
            "/api/previews/authorize",
            "/api/sandboxes",
            "/api/sandboxes/p1",
            "/api/sandboxes/p1/",
            "/api/sandboxes/p1/tasks",
            "/api/sandboxes/p1/files/content",
            "/api/sandboxes/p1/git",
            "/api/sandboxes/p1/shells/s1/attachments",
            "/api/sandboxes/p1/stages/timeline",
            "/api/tasks/t1",
            "/api/tasks/t1/terminal",
            "/api/sessions/x/transcript",
            "/api/shells/x/terminal",
        ] {
            assert!(get(path), "{path}");
        }
        assert!(guest_may_reach(&Method::POST, "/api/auth/logout"));
        assert!(guest_may_reach(&Method::POST, "/api/sandboxes/p1/tasks"));
        for path in [
            "/api/sandboxes/p1/agent-secrets",
            "/api/sandboxes/p1/variables",
            "/api/sandboxes/p1/team-secrets",
            "/api/sandboxes/p1/mcp",
            "/api/sandboxes/p1/skills",
            "/api/sandboxes/p1/automations",
            "/api/sandboxes/p1/automation-templates",
            "/api/sandboxes/p1/workflows",
            "/api/sandboxes/p1/agent-states",
            "/api/sandboxes/p1/cockpit",
            "/api/sandboxes/p1/superagent/requests",
            "/api/sandboxes/p1/guest-invites",
            "/api/sandboxes/p1/delivery/runs",
            "/api/sandboxes/p1/cd",
            "/api/sandboxes/p1/hygiene",
            "/api/sandboxes/p1/vidoc",
            "/api/sandboxes/p1/onboarding/plan",
            "/api/sandboxes/p1/move-to-github",
            "/api/sandboxes/p1/purge",
            "/api/sandboxes//tasks",
            "/api/tasks/",
            "/api/team-secrets",
            "/api/secrets/activity",
            "/api/agent-secrets/x",
            "/api/me/tokens",
            "/api/device/approve",
            "/api/playgrounds",
            "/api/import",
            "/api/onboard/plan",
            "/api/github/repositories",
            "/api/status",
            "/api/disk",
            "/api/capacity",
            "/api/connections",
            "/api/update",
            "/api/notes",
            "/api/notes/presence",
            "/api/analytics/summary",
            "/api/admin/users",
            "/api/mcp/x/test",
            "/api/workflows/x",
            "/api/automations/x",
            "/api/auth/reauth",
            "/api/templates",
            "/mcp",
            "/api/me/tokensx",
        ] {
            assert!(!get(path), "{path}");
            assert!(!guest_may_reach(&Method::POST, path), "{path}");
        }
        // A guest makes no sandbox or playground.
        assert!(!guest_may_reach(&Method::POST, "/api/sandboxes"));
    }
}
