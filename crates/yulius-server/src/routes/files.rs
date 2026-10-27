//! Sandbox files for the built-in editor (docs/api.md; SPEC-001 EDT-001..004).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_contracts::api::files::{
    CreateEntry, DirListing, FileEntry, FilePresence, FileRoots, FileWritten, WriteFile,
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
