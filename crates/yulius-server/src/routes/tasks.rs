//! Task routes (non-runtime parts; lifecycle routes are added with the supervisor).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use yulius_contracts::api::tasks::{
    FeedbackRequest, MessageView, PostMessage, TaskDetail, TaskSummary,
};

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

pub async fn post_message(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Json(req): Json<PostMessage>,
) -> ApiResult<Json<MessageView>> {
    Ok(Json(app.post_user_message(&ctx, &tid, &req.body).await?))
}
