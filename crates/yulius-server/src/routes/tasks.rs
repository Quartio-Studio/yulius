//! Task routes (non-runtime parts; lifecycle routes are added with the supervisor).

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;
use yulius_contracts::api::tasks::{FeedbackRequest, TaskDetail, TaskPrompt, TaskSummary};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;
use yulius_app::error::AppError;

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

/// `GET /api/tasks/{tid}/prompt` (SPEC-043).
pub async fn prompt(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskPrompt>> {
    Ok(Json(app.task_prompt(&ctx, &tid).await?))
}

/// `GET /api/tasks/{tid}/prompt/attachments/{n}` (SPEC-043): an image is shown, any other file
/// downloads; with `sandbox` in the content security policy, none runs as a page. The file is
/// streamed in bounded chunks while it holds its read slot (SPEC-080).
pub async fn prompt_attachment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((tid, index)): Path<(String, String)>,
) -> ApiResult<Response> {
    // An index that is not a number names no attachment: 404, like one out of range.
    let index = index.parse().map_err(|_| AppError::NotFound)?;
    let file = app.task_prompt_attachment(&ctx, &tid, index).await?;
    let (name, content_type, len) = (file.name.clone(), file.content_type, file.len);
    let chunks = futures::stream::try_unfold(file, |mut file| async move {
        Ok::<_, std::io::Error>(file.next_chunk().await?.map(|chunk| (chunk, file)))
    });
    let mut response = Body::from_stream(chunks).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Some(image) = content_type {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(image));
    } else {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        if let Ok(value) = HeaderValue::from_str(&content_disposition(&name)) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    Ok(response)
}

/// `attachment; filename="…"` naming the file, plus `filename*` (RFC 6266, RFC 8187) when the
/// name is not plain ASCII: the browser downloads the link itself and this name wins over the
/// page's (SPEC-325). The plain name keeps only safe characters (others become `_`).
fn content_disposition(name: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+');
    let plain: String = name
        .chars()
        .map(|c| if safe(c) { c } else { '_' })
        .collect();
    let plain = if plain.is_empty() {
        "attachment".into()
    } else {
        plain
    };
    if name.chars().all(safe) {
        return format!("attachment; filename=\"{plain}\"");
    }
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

pub async fn archive(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.archive_task(&ctx, &tid, true).await?))
}

/// `POST /api/tasks/{tid}/control-request` (SPEC-070): ask the owner for control.
pub async fn request_control(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.request_task_control(&ctx, &tid).await?))
}

/// `DELETE /api/tasks/{tid}/control-request`: the asker withdraws, or the owner declines.
pub async fn drop_control_request(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.drop_task_control_request(&ctx, &tid).await?))
}

/// `POST /api/tasks/{tid}/control-request/accept`: the owner hands the task over.
pub async fn accept_control(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.accept_task_control(&ctx, &tid).await?))
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

/// `POST /api/tasks/{tid}/pause` (SPEC-191 PER-001).
pub async fn pause(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.pause_task_for_person(&ctx, &tid).await?))
}

/// `POST /api/tasks/{tid}/resume` (SPEC-191 PER-002): ends a person's pause.
pub async fn unpause(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.resume_task_for_person(&ctx, &tid).await?))
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

pub async fn schedule(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Json(req): Json<yulius_contracts::api::tasks::ScheduleTask>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.schedule_task(&ctx, &tid, &req).await?))
}

pub async fn change_start(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Json(req): Json<yulius_contracts::api::tasks::ChangeStart>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.change_task_start(&ctx, &tid, &req).await?))
}

pub async fn queue_message(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Json(req): Json<yulius_contracts::api::tasks::QueueMessage>,
) -> ApiResult<Json<yulius_contracts::api::tasks::QueuedMessage>> {
    Ok(Json(app.queue_task_message(&ctx, &tid, &req).await?))
}

pub async fn held_prompt(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((tid, hid)): Path<(String, String)>,
    Json(req): Json<yulius_contracts::api::intake::ResolveHeldPrompt>,
) -> ApiResult<Json<yulius_contracts::api::intake::HeldPromptOutcome>> {
    Ok(Json(app.resolve_held_prompt(&ctx, &tid, &hid, &req).await?))
}
