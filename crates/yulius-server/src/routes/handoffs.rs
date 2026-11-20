//! Tasks hand work to another sandbox (SPEC-512 part 5): the instance's setting, a sandbox's
//! opt-out, a task's handoffs and the owner's answer.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::handoffs::{
    AnswerHandoff, HandoffSetting, SandboxHandoffs, TaskHandoffs,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/handoffs`.
pub async fn setting(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<HandoffSetting>> {
    Ok(Json(app.handoff_setting(&ctx).await?))
}

/// `PUT /api/admin/handoffs`.
pub async fn set_setting(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<HandoffSetting>,
) -> ApiResult<Json<HandoffSetting>> {
    Ok(Json(app.set_handoff_setting(&ctx, &req).await?))
}

/// `GET /api/sandboxes/{sid}/handoffs`.
pub async fn sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<SandboxHandoffs>> {
    Ok(Json(app.sandbox_handoffs(&ctx, &sid).await?))
}

/// `PUT /api/sandboxes/{sid}/handoffs`.
pub async fn set_sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<SandboxHandoffs>,
) -> ApiResult<Json<SandboxHandoffs>> {
    Ok(Json(app.set_sandbox_handoffs(&ctx, &sid, &req).await?))
}

/// `GET /api/tasks/{tid}/handoffs`.
pub async fn task(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskHandoffs>> {
    Ok(Json(app.task_handoffs(&ctx, &tid).await?))
}

/// `POST /api/tasks/{tid}/handoffs/{hid}`.
pub async fn answer(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((tid, hid)): Path<(String, String)>,
    Json(req): Json<AnswerHandoff>,
) -> ApiResult<Json<TaskHandoffs>> {
    Ok(Json(
        app.answer_handoff(&ctx, &tid, &hid, req.approve).await?,
    ))
}
