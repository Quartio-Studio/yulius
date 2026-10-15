//! CD (deployment) (SPEC-134; docs/api.md "CD"). Reads need read access to the sandbox;
//! deploying dev and cancelling need write access; production needs administrator access and
//! the project's slug typed as `confirm`.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::cd::{CdDeploy, CdOverview, CdRun};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn overview(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<CdOverview>> {
    Ok(Json(app.cd_overview(&ctx, &sid).await?))
}

pub async fn deploy(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, env)): Path<(String, String)>,
    Json(req): Json<CdDeploy>,
) -> ApiResult<Json<CdRun>> {
    Ok(Json(app.cd_deploy(&ctx, &sid, &env, &req).await?))
}

pub async fn run(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, rid)): Path<(String, String)>,
) -> ApiResult<Json<CdRun>> {
    Ok(Json(app.cd_run_detail(&ctx, &sid, &rid).await?))
}

pub async fn cancel(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, rid)): Path<(String, String)>,
) -> ApiResult<Json<CdRun>> {
    Ok(Json(app.cd_cancel(&ctx, &sid, &rid).await?))
}
