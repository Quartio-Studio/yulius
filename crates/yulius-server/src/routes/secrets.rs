//! Secrets vault routes (SPEC-001 VLT, SPEC-002 ADR-20): team secrets, re-authentication, agent
//! secrets, the activity trail, and the internal redaction endpoints of agent runs.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use yulius_app::model::Secret;
use yulius_app::vault::Revealed;
use yulius_contracts::api::secrets::{
    AgentSecretInput, AgentSecretPatch, AgentSecretView, ReauthPassword, ReauthStatus,
    RevealRequest, TeamSecretInput, TeamSecretPatch, TeamSecretView, VaultActivity,
};
use yulius_contracts::scrub::{FingerprintSet, RedactionReport};

use crate::AppState;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};

/// Public routes of the vault.
pub fn public() -> Router<AppState> {
    Router::new()
        .route("/api/team-secrets", get(team_list).post(team_create))
        .route(
            "/api/team-secrets/{id}",
            patch(team_update).delete(team_delete),
        )
        .route("/api/team-secrets/{id}/reveal", post(team_reveal))
        .route("/api/auth/reauth", get(reauth_status).post(reauth))
        .route("/api/secrets/activity", get(activity))
        .route(
            "/api/sandboxes/{sid}/agent-secrets",
            get(agent_list).post(agent_create),
        )
        .route(
            "/api/agent-secrets/{id}",
            patch(agent_update).delete(agent_delete),
        )
}

/// Internal (loopback, task token) routes of the vault.
pub fn internal() -> Router<AppState> {
    Router::new()
        .route("/internal/scrub/fingerprints", post(fingerprints))
        .route("/internal/scrub/report", post(report))
}

/// A revealed value must not be cached anywhere on the way.
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `403 reauth_required` with the ways to confirm.
fn reauth_required(status: ReauthStatus) -> Response {
    no_store(
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "code": "reauth_required",
                    "message": "Confirm it is you to reveal secrets (password or GitHub sign-in in the last 10 minutes)."
                },
                "reauth": status,
            })),
        )
            .into_response(),
    )
}

fn revealed<T: serde::Serialize>(outcome: Revealed<T>) -> Response {
    match outcome {
        Revealed::Values(values) => no_store(Json(values).into_response()),
        Revealed::ReauthRequired(status) => reauth_required(status),
    }
}

async fn team_list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<TeamSecretView>>> {
    Ok(Json(app.team_secrets(&ctx).await?))
}

async fn team_create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(input): Json<TeamSecretInput>,
) -> ApiResult<(StatusCode, Json<TeamSecretView>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_team_secret(&ctx, &input).await?),
    ))
}

async fn team_update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(patch): Json<TeamSecretPatch>,
) -> ApiResult<Json<TeamSecretView>> {
    Ok(Json(app.update_team_secret(&ctx, &id, &patch).await?))
}

async fn team_delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_team_secret(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn team_reveal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    body: Option<Json<RevealRequest>>,
) -> Result<Response, ApiError> {
    let request = body.map(|Json(b)| b).unwrap_or_default();
    Ok(revealed(app.reveal_team_secret(&ctx, &id, &request).await?))
}

async fn reauth_status(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<ReauthStatus>> {
    Ok(Json(app.reauth_status(&ctx).await?))
}

async fn reauth(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(body): Json<ReauthPassword>,
) -> ApiResult<Json<ReauthStatus>> {
    Ok(Json(
        app.reauth_with_password(&ctx, &Secret(body.password))
            .await?,
    ))
}

#[derive(Deserialize)]
struct ActivityQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn activity(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Query(query): Query<ActivityQuery>,
) -> ApiResult<Json<Vec<VaultActivity>>> {
    Ok(Json(
        app.vault_activity(&ctx, query.limit.unwrap_or(100)).await?,
    ))
}

async fn agent_list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<AgentSecretView>>> {
    Ok(Json(app.agent_secrets(&ctx, &sid).await?))
}

async fn agent_create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(input): Json<AgentSecretInput>,
) -> ApiResult<(StatusCode, Json<AgentSecretView>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_agent_secret(&ctx, &sid, &input).await?),
    ))
}

async fn agent_update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(patch): Json<AgentSecretPatch>,
) -> ApiResult<Json<AgentSecretView>> {
    Ok(Json(app.update_agent_secret(&ctx, &id, &patch).await?))
}

async fn agent_delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_agent_secret(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

async fn fingerprints(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FingerprintSet>, StatusCode> {
    let token = bearer(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    app.scrub_fingerprints(token)
        .await
        .map(Json)
        .map_err(|err| {
            tracing::debug!(error = %err, "fingerprints refused");
            crate::error::status_for(&err)
        })
}

async fn report(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RedactionReport>,
) -> StatusCode {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED;
    };
    match app.scrub_report(token, &body).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(err) => crate::error::status_for(&err),
    }
}
