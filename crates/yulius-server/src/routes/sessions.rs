//! Sessions as read-only documents (docs/api.md "Sessions") and the onboarding plan of
//! `yulius onboard` (docs/api.md "Import").

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use yulius_app::session_docs::{SessionFilter, TranscriptRead, parse_kind};
use yulius_contracts::api::sessions::{SessionDoc, SessionList, TaskTranscript, TranscriptPage};
use yulius_contracts::api::tasks::TaskSummary;
use yulius_contracts::import::{OnboardPlan, OnboardPlanRequest};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct ListQuery {
    kind: Option<String>,
    author: Option<String>,
    q: Option<String>,
    limit: Option<usize>,
}

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<SessionList>> {
    let filter = SessionFilter {
        kind: parse_kind(query.kind.as_deref())?,
        author: query.author.filter(|a| !a.trim().is_empty()),
        query: query.q,
        limit: query.limit,
    };
    Ok(Json(app.list_sessions(&ctx, &sid, &filter).await?))
}

pub async fn detail(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionDoc>> {
    Ok(Json(app.session_detail(&ctx, &id).await?))
}

#[derive(Deserialize)]
pub struct PageQuery {
    from: Option<u64>,
    limit: Option<usize>,
}

pub async fn transcript(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<TranscriptPage>> {
    Ok(Json(
        app.session_transcript(
            &ctx,
            &id,
            query.from.unwrap_or(0),
            query.limit.unwrap_or(200),
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct TaskTranscriptQuery {
    from: Option<u64>,
    limit: Option<usize>,
    seen: Option<u64>,
    session: Option<String>,
}

/// `GET /api/tasks/{tid}/transcript`: the task's live session for its watchers (SPEC-179).
pub async fn task_transcript(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Query(query): Query<TaskTranscriptQuery>,
) -> ApiResult<Json<TaskTranscript>> {
    let read = TranscriptRead {
        from: query.from,
        limit: query.limit.unwrap_or(200),
        seen: query.seen,
        session: query.session,
    };
    Ok(Json(app.task_transcript(&ctx, &tid, read).await?))
}

pub async fn continue_session(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskSummary>> {
    Ok(Json(app.continue_session(&ctx, &id).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_session(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn plan(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<OnboardPlanRequest>,
) -> ApiResult<Json<OnboardPlan>> {
    Ok(Json(app.onboard_plan(&ctx, &req).await?))
}
