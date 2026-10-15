//! Stages of tasks, CI and CD (SPEC-137; docs/api.md "Stages"): one subject's timeline, the
//! stages' figures over a range and their export. Read access to the sandbox; nothing writes
//! spans through the API.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_app::stages::{StageQuery, spans_csv};
use yulius_contracts::api::stages::{StageSummary, TimelineView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct SubjectQuery {
    #[serde(default)]
    subject: String,
}

pub async fn timeline(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<SubjectQuery>,
) -> ApiResult<Json<TimelineView>> {
    Ok(Json(app.stage_timeline(&ctx, &sid, &q.subject).await?))
}

pub async fn summary(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<StageQuery>,
) -> ApiResult<Json<StageSummary>> {
    Ok(Json(app.stage_summary(&ctx, &sid, &q).await?))
}

pub async fn spans(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<StageQuery>,
) -> ApiResult<Response> {
    let format = q.format.as_deref().unwrap_or("json");
    if format != "json" && format != "csv" {
        return Err(AppError::Invalid("format must be json or csv".into()).into());
    }
    let rows = app.stage_spans(&ctx, &sid, &q).await?;
    if format == "json" {
        return Ok(Json(rows).into_response());
    }
    let day = |value: Option<&str>| -> String {
        value
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '-')
            .collect()
    };
    let (from, to) = (day(q.from.as_deref()), day(q.to.as_deref()));
    let filename = format!(
        "yulius-stages-{}-{}.csv",
        if from.is_empty() { "start" } else { &from },
        if to.is_empty() { "now" } else { &to }
    );
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        spans_csv(&rows),
    )
        .into_response())
}
