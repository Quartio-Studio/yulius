//! Task routes (non-runtime parts; lifecycle routes are added with the supervisor).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use yulius_contracts::api::tasks::{FeedbackRequest, MessageView, TaskDetail, TaskSummary};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    archived: bool,
}

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<TaskSummary>>> {
    Ok(Json(app.list_tasks(&ctx, &sid, q.archived).await?))
}

pub async fn archived(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<TaskSummary>>> {
    Ok(Json(app.archived_tasks(&ctx).await?))
}

pub async fn detail(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskDetail>> {
    Ok(Json(app.task_detail(&ctx, &tid).await?))
}

pub async fn archive(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.archive_task(&ctx, &tid, true).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_task(&ctx, &tid).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.archive_task(&ctx, &tid, false).await?))
}

pub async fn feedback(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Json(req): Json<FeedbackRequest>,
) -> ApiResult<StatusCode> {
    app.task_feedback(&ctx, &tid, &req.verdict, &req.comment)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn messages(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<Vec<MessageView>>> {
    Ok(Json(app.task_messages(&ctx, &tid).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<yulius_contracts::api::tasks::CreateTask>,
) -> ApiResult<(StatusCode, Json<TaskSummary>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_task(&ctx, &sid, &req).await?),
    ))
}

pub async fn stop(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.stop_task(&ctx, &tid).await?))
}

pub async fn resume(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    body: Option<Json<yulius_contracts::api::tasks::ContinueTask>>,
) -> ApiResult<Json<TaskSummary>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(app.continue_task(&ctx, &tid, &req).await?))
}

pub async fn switch(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    body: Option<Json<yulius_contracts::api::tasks::ContinueTask>>,
) -> ApiResult<Json<TaskSummary>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(app.switch_task(&ctx, &tid, &req).await?))
}

pub async fn intake_check(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<yulius_contracts::api::intake::IntakeRequest>,
) -> ApiResult<Json<yulius_contracts::api::intake::IntakeVerdict>> {
    Ok(Json(app.intake_check(&ctx, &sid, &req).await?))
}

pub async fn intake_proposal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<yulius_contracts::api::intake::IntakeRequest>,
) -> ApiResult<Json<yulius_contracts::api::intake::SplitProposal>> {
    Ok(Json(app.intake_proposal(&ctx, &sid, &req).await?))
}

pub async fn start_now(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.start_chained_now(&ctx, &tid).await?))
}

pub async fn held_prompt(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((tid, hid)): Path<(String, String)>,
    Json(req): Json<yulius_contracts::api::intake::ResolveHeldPrompt>,
) -> ApiResult<Json<yulius_contracts::api::intake::HeldPromptOutcome>> {
    Ok(Json(app.resolve_held_prompt(&ctx, &tid, &hid, &req).await?))
}
