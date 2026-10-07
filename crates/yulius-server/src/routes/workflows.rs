//! Workflow routes (SPEC-001 AUT-004).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::workflows::Workflow;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<Workflow>>> {
    Ok(Json(app.list_workflows(&ctx, &sid).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<Workflow>,
) -> ApiResult<(StatusCode, Json<Workflow>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_workflow(&ctx, &sid, &req).await?),
    ))
}

pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<Workflow>,
) -> ApiResult<Json<Workflow>> {
    Ok(Json(app.update_workflow(&ctx, &id, &req).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_workflow(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
