//! Brokered commands (SPEC-626): a project's command definitions, which its administrators
//! change. Binding a secret people allow use by use asks for a fresh two-factor code first
//! (`403 reauth_required`, as the Secrets page's answers do).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::vault::Revealed;
use yulius_contracts::api::commands::{CommandDefinition, CommandList, CommandView};

use super::secrets::reauth_required;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `PUT …/commands/{id}?version=N`: the version the editor read.
#[derive(Deserialize)]
pub struct Version {
    version: Option<i64>,
}

fn answered(outcome: Revealed<CommandView>, status: StatusCode) -> Response {
    match outcome {
        Revealed::Values(view) => (status, Json(view)).into_response(),
        Revealed::ReauthRequired(reauth) => reauth_required(reauth),
    }
}

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
) -> ApiResult<Response> {
    Ok(answered(
        app.create_command(&ctx, &sid, req).await?,
        StatusCode::CREATED,
    ))
}

/// `PUT /api/sandboxes/{sid}/commands/{id}`.
pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
    Query(Version { version }): Query<Version>,
    Json(req): Json<CommandDefinition>,
) -> ApiResult<Response> {
    Ok(answered(
        app.update_command(&ctx, &sid, &id, version, req).await?,
        StatusCode::OK,
    ))
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
