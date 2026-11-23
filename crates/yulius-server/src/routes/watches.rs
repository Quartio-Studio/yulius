//! Script watches (SPEC-630 WCH-011): a project's scheduled scripts, managed on its Watches page.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::watches::{WatchInput, WatchView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/sandboxes/{sid}/watches`.
pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<WatchView>>> {
    Ok(Json(app.watches_for(&ctx, &sid).await?))
}

/// `POST /api/sandboxes/{sid}/watches`.
pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<WatchInput>,
) -> ApiResult<(StatusCode, Json<WatchView>)> {
    let view = app.watch_create_for(&ctx, &sid, req, "ui").await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `GET /api/watches/{id}`.
pub async fn get(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<WatchView>> {
    Ok(Json(app.watch_for(&ctx, &id).await?))
}

/// `PUT /api/watches/{id}`.
pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<WatchInput>,
) -> ApiResult<Json<WatchView>> {
    Ok(Json(app.watch_update_for(&ctx, &id, req).await?))
}

/// `DELETE /api/watches/{id}`.
pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.watch_remove_for(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/watches/{id}/run`: it runs at once; the answer does not wait for its end.
pub async fn run(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<WatchView>)> {
    let view = app.watch_run_now(&ctx, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(view)))
}

/// `POST /api/watches/{id}/pause`.
pub async fn pause(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<WatchView>> {
    Ok(Json(app.watch_set_paused_for(&ctx, &id, true).await?))
}

/// `POST /api/watches/{id}/resume`.
pub async fn resume(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<WatchView>> {
    Ok(Json(app.watch_set_paused_for(&ctx, &id, false).await?))
}
