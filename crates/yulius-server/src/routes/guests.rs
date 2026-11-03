//! A playground's guests (SPEC-410, ADR-0260): its admins' expiring links, and joining through one
//! without GitHub.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use yulius_contracts::api::guests::{
    GuestInviteCreate, GuestInviteCreated, GuestInvitePeek, GuestInvites, GuestJoinRequest,
};

use crate::AppState;
use crate::auth::{Auth, session_cookie};
use crate::client::ClientAddress;
use yulius_app::error::AppError;

use crate::error::{ApiError, ApiResult};

/// Public routes of a playground's guests.
pub fn public() -> Router<AppState> {
    Router::new()
        .route("/api/sandboxes/{sid}/guest-invites", get(list).post(create))
        .route("/api/sandboxes/{sid}/guest-invites/{id}", delete(revoke))
        // No session: the link is the credential (GST-002).
        .route("/api/guest-invites/{code}", get(peek))
        .route("/api/guest-invites/{code}/join", post(join))
}

async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<GuestInvites>> {
    Ok(Json(app.guest_invites(&ctx, &sid).await?))
}

async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(input): Json<GuestInviteCreate>,
) -> ApiResult<(StatusCode, Json<GuestInviteCreated>)> {
    let created = app.create_guest_invite(&ctx, &sid, input.days).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn revoke(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    app.revoke_guest_invite(&ctx, &sid, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A link that is unknown, revoked or expired, or whose playground is gone, is one answer (410),
/// so the page can say the link ended and no code is told apart from another.
fn ended(err: AppError) -> Response {
    if matches!(err, AppError::NotFound) {
        let body = serde_json::json!({"error": {
            "code": "gone",
            "message": "this guest link has ended; ask whoever sent it for a new one",
        }});
        return (StatusCode::GONE, Json(body)).into_response();
    }
    ApiError::from(err).into_response()
}

async fn peek(
    State(app): State<AppState>,
    client: ClientAddress,
    Path(code): Path<String>,
) -> Response {
    match app.peek_guest_invite(&code, &client.limit_key()).await {
        Ok(peek) => Json::<GuestInvitePeek>(peek).into_response(),
        Err(err) => ended(err),
    }
}

/// Joins as a guest: the answer is the new session's `Me` with the playground's `slug`, and its
/// cookie.
async fn join(
    State(app): State<AppState>,
    client: ClientAddress,
    Path(code): Path<String>,
    Json(input): Json<GuestJoinRequest>,
) -> Response {
    let (session, slug) = match app
        .join_as_guest(&code, &input.name, &client.limit_key())
        .await
    {
        Ok(joined) => joined,
        Err(err) => return ended(err),
    };
    let mut body = serde_json::to_value(&session.me).unwrap_or_default();
    if let Some(object) = body.as_object_mut() {
        object.insert("slug".into(), slug.into());
    }
    let mut response = (StatusCode::CREATED, Json(body)).into_response();
    let cookie = session_cookie(
        session.cookie.expose(),
        session.max_age_s,
        app.config.secure_cookies,
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}
