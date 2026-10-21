//! Project routes: repository and template listings, sandbox create/import/remove and
//! onboarding (docs/api.md "Sandboxes").

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::sandboxes::{
    AvailableRepo, CreateSandbox, MoveToGitHub, OnboardingPlan, PurgeState, Sandbox, Template,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn repositories(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<AvailableRepo>>> {
    Ok(Json(app.github_repositories(&ctx).await?))
}

pub async fn templates(State(app): State<AppState>, Auth(_ctx): Auth) -> Json<Vec<Template>> {
    Json(app.templates())
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<CreateSandbox>,
) -> ApiResult<(StatusCode, Json<Sandbox>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_sandbox(&ctx, &req).await?),
    ))
}

pub async fn remove(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_sandbox(&ctx, &sid).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a playground into an empty GitHub repository of the instance owner (SPEC-176).
pub async fn move_to_github(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<MoveToGitHub>,
) -> ApiResult<Json<Sandbox>> {
    Ok(Json(
        app.move_playground_to_github(&ctx, &sid, &req.name).await?,
    ))
}

/// Removes a playground (when still live) and deletes its code for good once nothing of it runs
/// (SPEC-171).
pub async fn purge(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<PurgeState>> {
    Ok(Json(app.purge_playground(&ctx, &sid).await?))
}

pub async fn skip_onboarding(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Sandbox>> {
    Ok(Json(app.skip_onboarding(&ctx, &sid).await?))
}

pub async fn onboarding_plan(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<OnboardingPlan>> {
    Ok(Json(app.onboarding_plan(&ctx, &sid).await?))
}

pub async fn approve_onboarding(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    body: Option<Json<yulius_contracts::api::sandboxes::ApproveOnboarding>>,
) -> ApiResult<Json<Sandbox>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(
        app.approve_onboarding(&ctx, &sid, req.notes.as_deref())
            .await?,
    ))
}
