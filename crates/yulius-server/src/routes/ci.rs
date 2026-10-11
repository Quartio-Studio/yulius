//! Yulius-run CI and repository hygiene (SPEC-001 DLV-006/007, SPEC-002 ADR-21; docs/api.md
//! "CI"). Reads need read access to the sandbox; running, cancelling and cleaning up need write
//! access.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::ci::{
    CiOverview, CiRun, HygieneCleanup, HygieneCleanupResult, HygieneReport,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn overview(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<CiOverview>> {
    Ok(Json(app.ci_overview(&ctx, &sid).await?))
}

pub async fn sync(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<CiOverview>> {
    Ok(Json(app.ci_sync_now(&ctx, &sid).await?))
}

pub async fn run(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, rid)): Path<(String, String)>,
) -> ApiResult<Json<CiRun>> {
    Ok(Json(app.ci_run_detail(&ctx, &sid, &rid).await?))
}

pub async fn cancel(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, rid)): Path<(String, String)>,
) -> ApiResult<Json<CiRun>> {
    Ok(Json(app.ci_cancel_run(&ctx, &sid, &rid).await?))
}

pub async fn rerun(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, rid)): Path<(String, String)>,
) -> ApiResult<Json<CiRun>> {
    Ok(Json(app.ci_rerun_run(&ctx, &sid, &rid).await?))
}

pub async fn hygiene(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<HygieneReport>> {
    Ok(Json(app.hygiene_view(&ctx, &sid).await?))
}

pub async fn hygiene_scan(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<HygieneReport>> {
    Ok(Json(app.hygiene_scan_now(&ctx, &sid).await?))
}

pub async fn hygiene_cleanup(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    body: Option<Json<HygieneCleanup>>,
) -> ApiResult<Json<HygieneCleanupResult>> {
    let request = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(app.hygiene_cleanup(&ctx, &sid, &request).await?))
}
