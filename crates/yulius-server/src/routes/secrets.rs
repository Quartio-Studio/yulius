//! Secrets vault routes (SPEC-001 VLT, SPEC-002 ADR-20): team secrets, re-authentication, agent
//! secrets, the activity trail, and the internal redaction endpoints of agent runs.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use futures::StreamExt as _;
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_app::model::Secret;
use yulius_app::uploads::UPLOAD_IDLE;
use yulius_app::vault::Revealed;
use yulius_app::vault::states::MAX_STATE_BYTES;
use yulius_contracts::api::secrets::{
    AgentSecretInput, AgentSecretPatch, AgentSecretSwitch, AgentSecretView, AgentStateView,
    CdProdAnswer, EnvFilesAnswer, HostRequestAnswer, ReauthPassword, ReauthStatus, RevealRequest,
    TeamSecretInput, TeamSecretPatch, TeamSecretView, UnlockRequest, VaultActivity,
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
        .route(
            "/api/sandboxes/{sid}/team-secrets",
            get(sandbox_team_list).post(sandbox_team_create),
        )
        .route("/api/auth/reauth", get(reauth_status).post(reauth))
        .route("/api/auth/two-factor", post(two_factor_setup))
        .route("/api/auth/unlock", post(unlock))
        .route("/api/users/{id}/two-factor", delete(two_factor_reset))
        .route("/api/secrets/activity", get(activity))
        .route(
            "/api/sandboxes/{sid}/agent-secrets",
            get(agent_list).post(agent_create),
        )
        .route(
            "/api/sandboxes/{sid}/agent-secrets/{id}/enabled",
            put(agent_switch),
        )
        .route(
            "/api/agent-secrets/{id}",
            patch(agent_update).delete(agent_delete),
        )
        // Agent states (SPEC-271): a raw gzip tar body, bounded while it is read.
        .route("/api/sandboxes/{sid}/agent-states", get(state_list))
        .route(
            "/api/sandboxes/{sid}/agent-states/{name}",
            put(state_import),
        )
        .route(
            "/api/sandboxes/{sid}/agent-states/{name}/archive",
            get(state_export),
        )
        .route("/api/agent-secrets/{id}/reveal", post(agent_reveal))
        .route("/api/agent-secrets/{id}/request", post(agent_answer))
        .route("/api/agent-secrets/{id}/env", post(agent_env_answer))
        .route(
            "/api/agent-secrets/{id}/cd-prod",
            post(agent_cd_prod_answer),
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

/// `403 reauth_required` with what is missing: a two-factor code (or setting it up), or, to set
/// it up, a recent sign-in or confirmation.
fn reauth_required(status: ReauthStatus) -> Response {
    let message = if status.two_factor == "locked" {
        "Too many wrong codes: confirm it is you (password or GitHub sign-in), then enter a code."
    } else if status.two_factor == "on" {
        "Enter your two-factor code to unlock the secrets for 10 minutes."
    } else if status.identity_fresh {
        "Set up two-factor to reveal secrets."
    } else {
        "Confirm it is you (password or GitHub sign-in) to set up two-factor."
    };
    no_store(
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "code": "reauth_required",
                    "message": message
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
        Json(app.create_team_secret(&ctx, None, &input).await?),
    ))
}

async fn sandbox_team_list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<TeamSecretView>>> {
    Ok(Json(app.sandbox_team_secrets(&ctx, &sid).await?))
}

async fn sandbox_team_create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(input): Json<TeamSecretInput>,
) -> ApiResult<(StatusCode, Json<TeamSecretView>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_team_secret(&ctx, Some(&sid), &input).await?),
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

async fn two_factor_setup(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> Result<Response, ApiError> {
    Ok(match app.setup_two_factor(&ctx).await? {
        Revealed::Values(setup) => no_store((StatusCode::CREATED, Json(setup)).into_response()),
        Revealed::ReauthRequired(status) => reauth_required(status),
    })
}

async fn two_factor_reset(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    Ok(match app.reset_two_factor(&ctx, &id).await? {
        Revealed::Values(()) => StatusCode::NO_CONTENT.into_response(),
        Revealed::ReauthRequired(status) => reauth_required(status),
    })
}

async fn unlock(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(body): Json<UnlockRequest>,
) -> ApiResult<Json<ReauthStatus>> {
    Ok(Json(app.unlock_vault(&ctx, &body.code).await?))
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

async fn agent_switch(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
    Json(input): Json<AgentSecretSwitch>,
) -> ApiResult<Json<AgentSecretView>> {
    Ok(Json(
        app.switch_agent_secret(&ctx, &sid, &id, input.enabled)
            .await?,
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

async fn agent_answer(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(answer): Json<HostRequestAnswer>,
) -> ApiResult<Json<AgentSecretView>> {
    Ok(Json(app.answer_host_request(&ctx, &id, &answer).await?))
}

async fn agent_env_answer(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(answer): Json<EnvFilesAnswer>,
) -> ApiResult<Json<AgentSecretView>> {
    Ok(Json(app.answer_env_files(&ctx, &id, &answer).await?))
}

async fn agent_cd_prod_answer(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(answer): Json<CdProdAnswer>,
) -> ApiResult<Json<AgentSecretView>> {
    Ok(Json(app.answer_cd_prod(&ctx, &id, answer.allow).await?))
}

async fn agent_delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_agent_secret(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn agent_reveal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    body: Option<Json<RevealRequest>>,
) -> Result<Response, ApiError> {
    let request = body.map(|Json(b)| b).unwrap_or_default();
    Ok(revealed(
        app.reveal_agent_secret(&ctx, &id, &request).await?,
    ))
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

#[derive(Deserialize)]
struct StateQuery {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    replace: bool,
}

async fn state_list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<AgentStateView>>> {
    Ok(Json(app.agent_states(&ctx, &sid).await?))
}

/// `PUT /api/sandboxes/{sid}/agent-states/{name}?description&replace` (SPEC-271 OPS-002): the
/// body is the packed state, at most [`MAX_STATE_BYTES`], each chunk within [`UPLOAD_IDLE`].
async fn state_import(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, name)): Path<(String, String)>,
    Query(q): Query<StateQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<AgentStateView>> {
    let too_large = || {
        AppError::Invalid(format!(
            "the state is larger than {} MiB packed; nothing was stored",
            MAX_STATE_BYTES >> 20
        ))
    };
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > MAX_STATE_BYTES) {
        return Err(too_large().into());
    }
    let mut archive = Vec::new();
    let mut stream = body.into_data_stream();
    loop {
        match tokio::time::timeout(UPLOAD_IDLE, stream.next()).await {
            Err(_) => return Err(AppError::Invalid("the upload stalled; try again".into()).into()),
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                return Err(AppError::Invalid("the upload was interrupted".into()).into());
            }
            Ok(Some(Ok(chunk))) => {
                if u64::try_from(archive.len() + chunk.len()).unwrap_or(u64::MAX) > MAX_STATE_BYTES
                {
                    return Err(too_large().into());
                }
                archive.extend_from_slice(&chunk);
            }
        }
    }
    Ok(Json(
        app.import_agent_state(
            &ctx,
            &sid,
            &name,
            q.description.as_deref(),
            q.replace,
            archive,
        )
        .await?,
    ))
}

/// `GET /api/sandboxes/{sid}/agent-states/{name}/archive` (SPEC-271 OPS-007): the newest packed
/// state, never cached.
async fn state_export(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, name)): Path<(String, String)>,
) -> ApiResult<Response> {
    let archive = app.export_agent_state(&ctx, &sid, &name).await?;
    Ok(no_store(
        ([(header::CONTENT_TYPE, "application/gzip")], archive).into_response(),
    ))
}
