//! Analytics summary and per-task export (read access; visible sandboxes only).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::analytics::tasks_csv;
use yulius_app::error::AppError;
use yulius_contracts::api::analytics::AnalyticsSummary;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct RangeQuery {
    #[serde(default)]
    sandbox: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    format: Option<String>,
}

pub async fn summary(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<AnalyticsSummary>> {
    Ok(Json(
        app.analytics_summary(
            &ctx,
            q.sandbox.as_deref(),
            q.from.as_deref(),
            q.to.as_deref(),
        )
        .await?,
    ))
}

/// Safe file-name part from a `YYYY-MM-DD` parameter (already validated by the app).
fn name_part(value: Option<&str>, fallback: &str) -> String {
    value
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.chars()
                .filter(|c| c.is_ascii_digit() || *c == '-')
                .collect()
        })
        .unwrap_or_else(|| fallback.to_owned())
}

pub async fn tasks(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Response> {
    let format = q.format.as_deref().unwrap_or("json");
    if format != "json" && format != "csv" {
        return Err(AppError::Invalid("format must be json or csv".into()).into());
    }
    let rows = app
        .analytics_tasks(
            &ctx,
            q.sandbox.as_deref(),
            q.from.as_deref(),
            q.to.as_deref(),
        )
        .await?;
    if format == "json" {
        return Ok(Json(rows).into_response());
    }
    let filename = format!(
        "yulius-tasks-{}-{}.csv",
        name_part(q.from.as_deref(), "start"),
        name_part(q.to.as_deref(), "now")
    );
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        tasks_csv(&rows),
    )
        .into_response())
}
