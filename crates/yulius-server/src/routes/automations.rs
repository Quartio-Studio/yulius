//! Automation routes (SPEC-001 AUT-001..005).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::automations::{Automation, AutomationRunView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<Automation>>> {
    Ok(Json(app.list_automations(&ctx, &sid).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<Automation>,
) -> ApiResult<(StatusCode, Json<Automation>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_automation(&ctx, &sid, &req).await?),
    ))
}

pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<Automation>,
) -> ApiResult<Json<Automation>> {
    Ok(Json(app.update_automation(&ctx, &id, &req).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_automation(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn run_now(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<AutomationRunView>> {
    Ok(Json(app.run_automation_now(&ctx, &id).await?))
}

pub async fn runs(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<AutomationRunView>>> {
    Ok(Json(app.automation_runs(&ctx, &id).await?))
}
