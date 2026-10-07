//! Internal agent endpoints: `POST /hooks/{harness}/{event}` and `POST /internal/git-credential`
//! (task-token bearer auth, loopback listener only).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_contracts::hooks::HookInput;

use crate::AppState;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn status_of(err: &AppError) -> StatusCode {
    match err {
        AppError::Unauthenticated => StatusCode::UNAUTHORIZED,
        AppError::Forbidden(_) => StatusCode::FORBIDDEN,
        AppError::NotFound => StatusCode::NOT_FOUND,
        AppError::Invalid(_) => StatusCode::BAD_REQUEST,
        AppError::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

/// Hook forwarding: the body is the CLI's hook JSON; the response body is printed by the hook
/// client as the hook's stdout (empty = no decision).
pub async fn hook(
    State(app): State<AppState>,
    Path((harness, event)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let input: HookInput = serde_json::from_slice(&body).unwrap_or_default();
    match app.hook_event(token, &harness, &event, &input).await {
        Ok(output) if output.is_empty() => StatusCode::OK.into_response(),
        Ok(output) => Json(output).into_response(),
        Err(err) => {
            tracing::debug!(error = %err, %harness, %event, "hook refused");
            status_of(&err).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CredentialRequest {
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    path: Option<String>,
}

pub async fn git_credential(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CredentialRequest>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if request.protocol != "https" {
        return StatusCode::FORBIDDEN.into_response();
    }
    match app
        .agent_git_credential(token, &request.host, request.path.as_deref())
        .await
    {
        Ok(credential) => {
            Json(serde_json::json!({"token": credential.token.expose()})).into_response()
        }
        Err(err) => {
            tracing::info!(error = %err, "git credential refused");
            status_of(&err).into_response()
        }
    }
}
