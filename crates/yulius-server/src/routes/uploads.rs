//! Uploads: files dropped onto the editor or a terminal (docs/api.md "Browser, editor, shell";
//! SPEC-001 EDT-006, UI-048). The raw body is one file, streamed to disk by the application
//! (`yulius_app::uploads`): no JSON body limit applies, the size limit and the idle bound do.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use futures::StreamExt as _;
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_app::playground::FolderUpload;
use yulius_app::uploads::{UPLOAD_IDLE, Upload};
use yulius_contracts::api::files::UploadedFile;
use yulius_contracts::api::sandboxes::Sandbox;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct FileUploadQuery {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    path: String,
    /// Replace an existing file (the UI asked the user first).
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize)]
pub struct AttachmentQuery {
    #[serde(default)]
    name: String,
}

/// The request's declared length, checked against the limit before any byte is read.
fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Streams the body into `upload` (each chunk must arrive within [`UPLOAD_IDLE`]) and finishes it.
async fn receive(app: &AppState, mut upload: Upload, body: Body) -> ApiResult<UploadedFile> {
    let mut stream = body.into_data_stream();
    loop {
        match tokio::time::timeout(UPLOAD_IDLE, stream.next()).await {
            Err(_) => {
                return Err(AppError::Invalid("the upload stalled; try again".into()).into());
            }
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                return Err(AppError::Invalid("the upload was interrupted".into()).into());
            }
            Ok(Some(Ok(chunk))) => upload.write(&chunk).await?,
        }
    }
    Ok(app.finish_upload(upload).await?)
}

/// `PUT /api/sandboxes/{sid}/files/upload?root&path&overwrite`.
pub async fn file(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<FileUploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<UploadedFile>)> {
    let upload = app
        .begin_file_upload(
            &ctx,
            &sid,
            q.root.as_deref(),
            &q.path,
            q.overwrite,
            declared_length(&headers),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(receive(&app, upload, body).await?),
    ))
}

/// `POST /api/tasks/{tid}/attachments?name`.
pub async fn task_attachment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    Query(q): Query<AttachmentQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<UploadedFile>)> {
    let upload = app
        .begin_task_attachment(&ctx, &tid, &q.name, declared_length(&headers))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(receive(&app, upload, body).await?),
    ))
}

/// `POST /api/sandboxes/{sid}/attachments?name`: a file dropped on the new-task composer.
pub async fn draft_attachment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Query(q): Query<AttachmentQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<UploadedFile>)> {
    let upload = app
        .begin_draft_attachment(&ctx, &sid, &q.name, declared_length(&headers))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(receive(&app, upload, body).await?),
    ))
}

/// `POST /api/sandboxes/{sid}/shells/{id}/attachments?name`.
pub async fn shell_attachment(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
    Query(q): Query<AttachmentQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<UploadedFile>)> {
    let upload = app
        .begin_shell_attachment(&ctx, &sid, &id, &q.name, declared_length(&headers))
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(receive(&app, upload, body).await?),
    ))
}

#[derive(Deserialize)]
pub struct PlaygroundQuery {
    #[serde(default)]
    name: String,
}

/// `POST /api/playgrounds?name`: a folder packed by the browser (gzip tar) becomes a playground
/// (SPEC-099 PLY-001, PLY-003, PLY-004).
pub async fn playground(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Query(q): Query<PlaygroundQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Sandbox>)> {
    let mut upload: FolderUpload = app
        .begin_folder_upload(&ctx, declared_length(&headers))
        .await?;
    let mut stream = body.into_data_stream();
    loop {
        match tokio::time::timeout(UPLOAD_IDLE, stream.next()).await {
            Err(_) => {
                return Err(AppError::Invalid("the upload stalled; try again".into()).into());
            }
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                return Err(AppError::Invalid("the upload was interrupted".into()).into());
            }
            Ok(Some(Ok(chunk))) => upload.write(&chunk).await?,
        }
    }
    // The setup runs on its own task: a client that goes away while it runs (a closed tab) never
    // cancels it halfway, which would leave its directories behind; it ends or cleans up alone.
    let setup =
        tokio::spawn(async move { app.playground_from_folder(&ctx, upload, &q.name).await });
    let sandbox = setup
        .await
        .map_err(|err| AppError::Internal(format!("the playground setup stopped: {err}")))??;
    Ok((StatusCode::CREATED, Json(sandbox)))
}
