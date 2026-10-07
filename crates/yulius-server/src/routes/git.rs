//! Git page (SPEC-001 GIT-001): branches, task branch facts, recent commits and pull requests.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::git::GitOverview;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn overview(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<GitOverview>> {
    Ok(Json(app.git_overview(&ctx, &sid).await?))
}
