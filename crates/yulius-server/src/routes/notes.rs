//! Auto-saved notes (SPEC-041): the team's notepad and one per sandbox.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::notes::{Note, SaveNote};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn team(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<Note>> {
    Ok(Json(app.note(&ctx, None).await?))
}

pub async fn save_team(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SaveNote>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.save_note(&ctx, None, &req).await?))
}

pub async fn sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.note(&ctx, Some(&sid)).await?))
}

pub async fn save_sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<SaveNote>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.save_note(&ctx, Some(&sid), &req).await?))
}
