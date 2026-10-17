//! Instance updates, the Jev key and the Vidoc key (SPEC-001 OPS-006..OPS-009, ADM-001, ADM-002;
//! SPEC-002 ADR-18; SPEC-195 VDK-002). Every route needs a session; everything but `GET /api/update` is superadmin-only.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use yulius_app::model::Secret;
use yulius_contracts::api::updates::{
    JevKeyView, JevTestResult, SandboxDockerView, SetJevKey, SetVidocKey, StartUpdate, UpdateView,
    VidocKeyView, VidocTestResult,
};
use yulius_contracts::update::VersionInfo;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/update`.
pub async fn view(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<UpdateView>> {
    Ok(Json(app.update_view(&ctx).await?))
}

/// `POST /api/admin/update/check`.
pub async fn check(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<UpdateView>> {
    Ok(Json(app.check_updates_now(&ctx).await?))
}

/// `POST /api/admin/update`.
pub async fn start(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<StartUpdate>,
) -> ApiResult<(StatusCode, Json<UpdateView>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(app.start_update(&ctx, &req).await?),
    ))
}

/// `POST /api/admin/update/cancel`.
pub async fn cancel(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<UpdateView>> {
    Ok(Json(app.cancel_update(&ctx).await?))
}

/// `POST /api/admin/update/proceed`.
pub async fn proceed(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<UpdateView>> {
    Ok(Json(app.proceed_update(&ctx).await?))
}

/// `GET /api/admin/jev`.
pub async fn jev(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<JevKeyView>> {
    Ok(Json(app.jev_key_view(&ctx).await?))
}

/// `PUT /api/admin/jev`.
pub async fn set_jev(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SetJevKey>,
) -> ApiResult<Json<JevKeyView>> {
    Ok(Json(app.set_jev_key(&ctx, &Secret(req.key)).await?))
}

/// `DELETE /api/admin/jev`.
pub async fn remove_jev(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<JevKeyView>> {
    Ok(Json(app.remove_jev_key(&ctx).await?))
}

/// `POST /api/admin/jev/adopt`: saves the key of `YULIUS_JEV_API_KEY_FILE` in settings.
pub async fn adopt_jev(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<JevKeyView>> {
    Ok(Json(app.adopt_config_jev_key(&ctx).await?))
}

/// `POST /api/admin/jev/test`.
pub async fn test_jev(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<JevTestResult>> {
    Ok(Json(app.test_jev_key(&ctx).await?))
}

/// `GET /api/admin/vidoc` (SPEC-195 VDK-002).
pub async fn vidoc(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<VidocKeyView>> {
    Ok(Json(app.vidoc_key_view(&ctx).await?))
}

/// `PUT /api/admin/vidoc`.
pub async fn set_vidoc(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SetVidocKey>,
) -> ApiResult<Json<VidocKeyView>> {
    Ok(Json(app.set_vidoc_key(&ctx, &Secret(req.key)).await?))
}

/// `DELETE /api/admin/vidoc`.
pub async fn remove_vidoc(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<VidocKeyView>> {
    Ok(Json(app.remove_vidoc_key(&ctx).await?))
}

/// `POST /api/admin/vidoc/adopt`: saves the key of `YULIUS_VIDOC_API_KEY_FILE` in settings.
pub async fn adopt_vidoc(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<VidocKeyView>> {
    Ok(Json(app.adopt_config_vidoc_key(&ctx).await?))
}

/// `POST /api/admin/vidoc/test`.
pub async fn test_vidoc(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<VidocTestResult>> {
    Ok(Json(app.test_vidoc_key(&ctx).await?))
}

/// `GET /api/admin/sandbox-docker` (SPEC-203 HST-014).
pub async fn sandbox_docker(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<SandboxDockerView>> {
    Ok(Json(app.sandbox_docker_view(&ctx).await?))
}

/// `POST /api/admin/sandbox-docker/smoke` (SPEC-203 HST-015).
pub async fn sandbox_docker_smoke(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<(StatusCode, Json<SandboxDockerView>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(app.start_docker_smoke(&ctx).await?),
    ))
}

/// `GET /internal/version` (internal listener): the build commit the update helper's health
/// check compares with the commit it installed.
pub async fn version(State(app): State<AppState>) -> Json<VersionInfo> {
    Json(VersionInfo {
        commit: app.build_commit().map(str::to_owned),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}
