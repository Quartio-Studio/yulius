//! Delivery profiles and the delivery flow (SPEC-001 DLV-001..005, GIT-002): pull requests,
//! merges, promotion and environment runs. Every state change here is an explicit request of
//! an authenticated person; there is no agent-facing equivalent.

use axum::Json;
use axum::extract::{Path, State};
use yulius_app::delivery::preset_profiles;
use yulius_contracts::api::git::{Delivery, EnvironmentRun, OpenPr, PromoteResult, RunEnvironment};
use yulius_contracts::api::sandboxes::{Profile, Sandbox, UpdateSandbox};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn update_sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<UpdateSandbox>,
) -> ApiResult<Json<Sandbox>> {
    Ok(Json(app.update_sandbox_profile(&ctx, &sid, &req).await?))
}

pub async fn presets(Auth(_ctx): Auth) -> Json<Vec<Profile>> {
    Json(preset_profiles())
}

pub async fn task_delivery(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<Delivery>> {
    Ok(Json(app.task_delivery(&ctx, &tid).await?))
}

pub async fn open_pr(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    body: Option<Json<OpenPr>>,
) -> ApiResult<Json<Delivery>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(app.open_task_pr(&ctx, &tid, &req).await?))
}

pub async fn merge(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<Delivery>> {
    Ok(Json(app.merge_task_pr(&ctx, &tid).await?))
}

pub async fn promote(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<PromoteResult>> {
    Ok(Json(app.promote_sandbox(&ctx, &sid).await?))
}

pub async fn run_environment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, env)): Path<(String, String)>,
    body: Option<Json<RunEnvironment>>,
) -> ApiResult<Json<EnvironmentRun>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(
        app.start_environment_run(&ctx, &sid, &env, &req).await?,
    ))
}

pub async fn confirm_environment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, env)): Path<(String, String)>,
    body: Option<Json<RunEnvironment>>,
) -> ApiResult<Json<EnvironmentRun>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(app.confirm_environment(&ctx, &sid, &env, &req).await?))
}

pub async fn runs(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<EnvironmentRun>>> {
    Ok(Json(app.environment_runs(&ctx, &sid).await?))
}

pub async fn run(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, run_id)): Path<(String, String)>,
) -> ApiResult<Json<EnvironmentRun>> {
    Ok(Json(app.environment_run(&ctx, &sid, &run_id).await?))
}
