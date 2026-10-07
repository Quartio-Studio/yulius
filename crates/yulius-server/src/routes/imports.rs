//! `yulius import` upload protocol (docs/api.md "Import", `yulius_contracts::import`).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{MethodRouter, put};
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_contracts::import::{
    CHUNK_SHA256_HEADER, CreateImportRequest, CreateImportResponse, FileProgress, ImportResult,
    MAX_CHUNK_BYTES,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct ChunkQuery {
    offset: Option<u64>,
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<CreateImportRequest>,
) -> ApiResult<Json<CreateImportResponse>> {
    Ok(Json(app.create_import(&ctx, &req).await?))
}

/// `PUT /api/import/{job}/files/{name}` with a body limit of one chunk (8 MiB) instead of the
/// default JSON limit.
pub fn chunk_route() -> MethodRouter<AppState> {
    let limit = usize::try_from(MAX_CHUNK_BYTES).unwrap_or(crate::MAX_BODY_BYTES);
    put(put_chunk).layer(DefaultBodyLimit::max(limit))
}

pub async fn put_chunk(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((job, name)): Path<(String, String)>,
    Query(query): Query<ChunkQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<FileProgress>> {
    let offset = query
        .offset
        .ok_or_else(|| AppError::Invalid("the offset query parameter is required".into()))?;
    let digest = headers
        .get(CHUNK_SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::Invalid(format!("the {CHUNK_SHA256_HEADER} header is required"))
        })?;
    Ok(Json(
        app.put_import_chunk(&ctx, &job, &name, offset, digest, &body)
            .await?,
    ))
}

pub async fn complete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(job): Path<String>,
) -> ApiResult<Json<ImportResult>> {
    Ok(Json(app.complete_import(&ctx, &job).await?))
}
