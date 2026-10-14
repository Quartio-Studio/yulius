//! Auto-saved notes (SPEC-041, SPEC-103): the team's notepad, one per sandbox and the caller's own,
//! and who has a shared one open (SPEC-112).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_app::notes::NoteTarget;
use yulius_contracts::api::notes::{Note, NotePresence, SaveNote};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn team(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<Note>> {
    Ok(Json(app.note(&ctx, NoteTarget::Team).await?))
}

pub async fn save_team(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SaveNote>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.save_note(&ctx, NoteTarget::Team, &req).await?))
}

pub async fn sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.note(&ctx, NoteTarget::Sandbox(&sid)).await?))
}

pub async fn save_sandbox(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<SaveNote>,
) -> ApiResult<Json<Note>> {
    Ok(Json(
        app.save_note(&ctx, NoteTarget::Sandbox(&sid), &req).await?,
    ))
}

pub async fn me(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<Note>> {
    Ok(Json(app.note(&ctx, NoteTarget::Me).await?))
}

pub async fn save_me(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SaveNote>,
) -> ApiResult<Json<Note>> {
    Ok(Json(app.save_note(&ctx, NoteTarget::Me, &req).await?))
}

pub async fn team_presence(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<NotePresence>,
) -> ApiResult<StatusCode> {
    app.note_presence(&ctx, NoteTarget::Team, &req).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn sandbox_presence(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<NotePresence>,
) -> ApiResult<StatusCode> {
    app.note_presence(&ctx, NoteTarget::Sandbox(&sid), &req)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
