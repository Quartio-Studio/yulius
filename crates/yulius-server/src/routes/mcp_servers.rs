//! MCP server configuration routes (SPEC-001 MCP-001..003). Secrets are write-only.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::mcp::{McpServerInput, McpServerView, McpTestResult};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<McpServerView>>> {
    Ok(Json(app.list_mcp_servers(&ctx, &sid).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<McpServerInput>,
) -> ApiResult<(StatusCode, Json<McpServerView>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_mcp_server(&ctx, &sid, &req).await?),
    ))
}

pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<McpServerInput>,
) -> ApiResult<Json<McpServerView>> {
    Ok(Json(app.update_mcp_server(&ctx, &id, &req).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_mcp_server(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<McpTestResult>> {
    Ok(Json(app.test_mcp_server(&ctx, &id).await?))
}
