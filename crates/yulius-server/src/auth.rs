//! Request authentication: session cookie (with CSRF on state-changing methods) or bearer token.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{Method, header};
use yulius_app::auth::AuthContext;
use yulius_app::error::AppError;

use crate::AppState;
use crate::error::ApiError;

/// Session cookie name.
pub const SESSION_COOKIE: &str = "yulius_session";
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
            return Ok(Self(ctx));
        }
        let cookie = cookie_value(parts, SESSION_COOKIE).ok_or(AppError::Unauthenticated)?;
        let ctx = app
            .authenticate_cookie(&cookie)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe {
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

/// Builds the `Set-Cookie` value for a session.
#[must_use]
pub fn session_cookie(value: &str, max_age_s: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={value}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_s}{secure}")
}

/// Builds a `Set-Cookie` value that clears the session.
#[must_use]
pub fn clear_cookie(secure: bool) -> String {
    session_cookie("", 0, secure)
}
