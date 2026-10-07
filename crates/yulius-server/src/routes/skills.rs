//! Skill catalog routes (SPEC-001 SKL-005).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::skills::{SetSkillEnabled, SkillContent, SkillView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<SkillView>>> {
    Ok(Json(app.list_skills(&ctx, &sid).await?))
}

pub async fn content(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, name)): Path<(String, String)>,
) -> ApiResult<Json<SkillContent>> {
    Ok(Json(app.skill_content(&ctx, &sid, &name).await?))
}

pub async fn set_enabled(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, name)): Path<(String, String)>,
    Json(req): Json<SetSkillEnabled>,
) -> ApiResult<StatusCode> {
    app.set_skill_enabled(&ctx, &sid, &name, req.enabled)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
