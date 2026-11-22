//! Brokered commands (SPEC-626): a project's command definitions, which its administrators
//! change.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::commands::{CommandDefinition, CommandList, CommandView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/sandboxes/{sid}/commands`.
pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<CommandList>> {
    Ok(Json(app.commands(&ctx, &sid).await?))
}

/// `POST /api/sandboxes/{sid}/commands`.
pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(req): Json<CommandDefinition>,
) -> ApiResult<(StatusCode, Json<CommandView>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_command(&ctx, &sid, req).await?),
    ))
}

/// `PUT /api/sandboxes/{sid}/commands/{id}`.
pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
    Json(req): Json<CommandDefinition>,
) -> ApiResult<Json<CommandView>> {
    Ok(Json(app.update_command(&ctx, &sid, &id, req).await?))
}

/// `DELETE /api/sandboxes/{sid}/commands/{id}`.
pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    app.delete_command(&ctx, &sid, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
