//! Internal `POST /mcp`: the Yulius MCP server for agents (task-token bearer auth) and Cockpits
//! (`ck_` bearer, SPEC-037).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use yulius_contracts::mcp::RpcRequest;

use crate::AppState;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

pub async fn post(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let request: RpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Json(serde_json::json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})).into_response();
        }
    };
    match app.mcp_handle(token, request).await {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

pub async fn get() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
