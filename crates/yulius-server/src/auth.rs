//! Request authentication: session cookie (with CSRF on state-changing methods) or bearer token.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Method, header};
use yulius_app::auth::AuthContext;
use yulius_app::error::AppError;

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
            // A read-only token reads (SPEC-247 RMT-002).
            if ctx.read_only() && !is_safe(&parts.method) {
                return Err(AppError::Forbidden(
                    "this API token is read-only; make an operate token on Settings to change things"
                        .into(),
                )
                .into());
            }
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
                return Err(AppError::Forbidden("missing or invalid CSRF token".into()).into());
            }
        }
        Ok(Self(ctx))
    }
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
