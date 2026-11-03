//! Sandbox files for the built-in editor (docs/api.md; SPEC-001 EDT-001..004).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_contracts::api::files::{
    CreateEntry, DirListing, FileEntry, FilePresence, FileRoots, FileWritten, LiveApplied, LiveDoc,
    LiveEdit, LiveFileAction, LiveJoin, LiveState, WriteFile,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct FileQuery {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    path: String,
    /// The `sha256` the caller has: an unchanged file answers 204 (SPEC-181).
    #[serde(default)]
    known: Option<String>,
}

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<DirListing>> {
    Ok(Json(
        app.files_list(&ctx, &sid, q.root.as_deref(), &q.path)
            .await?,
    ))
}

/// The folders the editor can open, each with its branch (SPEC-340 EWT-001).
pub async fn roots(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<FileRoots>> {
    Ok(Json(app.files_roots(&ctx, &sid).await?))
}

pub async fn read(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Response> {
    let file = app
        .file_read(&ctx, &sid, q.root.as_deref(), &q.path, q.known.as_deref())
        .await?;
    Ok(file.map_or_else(
        || StatusCode::NO_CONTENT.into_response(),
        |file| Json(file).into_response(),
    ))
}

pub async fn write(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<WriteFile>,
) -> ApiResult<Json<FileWritten>> {
    Ok(Json(app.file_write(&ctx, &sid, &req).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<CreateEntry>,
) -> ApiResult<(StatusCode, Json<FileEntry>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.file_create(&ctx, &sid, &req).await?),
    ))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<StatusCode> {
    app.file_delete(&ctx, &sid, q.root.as_deref(), &q.path)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Who has a file open and where (SPEC-127): relayed to the sandbox's writers, stored nowhere.
pub async fn presence(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<FilePresence>,
) -> ApiResult<StatusCode> {
    app.file_presence(&ctx, &sid, &req).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Joins a file's shared text (SPEC-411 EDT-017).
pub async fn live_join(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<LiveJoin>,
) -> ApiResult<Json<LiveDoc>> {
    Ok(Json(app.live_join(&ctx, &sid, &req).await?))
}

#[derive(Deserialize)]
pub struct LiveQuery {
    #[serde(default)]
    root: Option<String>,
    path: String,
    session: String,
    since: u64,
}

/// The steps of a file's shared text after `since`, or its whole text (SPEC-411 EDT-022).
pub async fn live_catch_up(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<LiveQuery>,
) -> ApiResult<Json<LiveDoc>> {
    Ok(Json(
        app.live_catch_up(&ctx, &sid, q.root.as_deref(), &q.path, &q.session, q.since)
            .await?,
    ))
}

/// One edit of a file's shared text (SPEC-411 EDT-017).
pub async fn live_edit(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<LiveEdit>,
) -> ApiResult<Json<LiveApplied>> {
    Ok(Json(app.live_edit(&ctx, &sid, &req).await?))
}

/// Writes a file's shared text to disk (SPEC-411 EDT-019).
pub async fn live_save(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<LiveFileAction>,
) -> ApiResult<Json<LiveState>> {
    Ok(Json(app.live_save(&ctx, &sid, &req).await?))
}

/// Takes a newer version on disk into a file's shared text (SPEC-411 EDT-020).
pub async fn live_disk(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<LiveFileAction>,
) -> ApiResult<Json<LiveState>> {
    Ok(Json(app.live_disk(&ctx, &sid, &req).await?))
}
