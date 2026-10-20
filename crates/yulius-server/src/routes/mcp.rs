//! `POST /mcp`: on the loopback listener, the Yulius MCP server for agents (task-token bearer
//! auth) and Cockpits (`ck_` bearer, SPEC-037); on the public listener, the remote MCP for a
//! person's own agent with a personal API token (SPEC-247).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use yulius_app::error::AppError;
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

/// Public `POST /mcp` (SPEC-247 RMT-010): MCP streamable HTTP with JSON answers, for personal
/// API tokens only. A cookie is never read here, so no browser session can be ridden.
pub async fn remote_post(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    };
    let request: RpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Json(serde_json::json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})).into_response();
        }
    };
    let caller = match app.remote_caller(token).await {
        Ok(caller) => caller,
        // 401 lets an MCP client say the token is wrong; 403 and 429 carry the reason.
        Err(err) => {
            let status = match &err {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::Capacity(_) => StatusCode::TOO_MANY_REQUESTS,
                AppError::Unauthenticated => StatusCode::UNAUTHORIZED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let body = serde_json::json!({"jsonrpc": "2.0", "id": request.id, "error": {"code": -32001, "message": err.public_message()}});
            let mut response = (status, Json(body)).into_response();
            if status == StatusCode::UNAUTHORIZED {
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
            }
            return response;
        }
    };
    match app.remote_mcp_handle(&caller, request).await {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}
