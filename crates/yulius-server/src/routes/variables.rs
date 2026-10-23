//! Project variables (SPEC-275, ADR-0203): a project's non-secret values, in plain.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use yulius_contracts::api::variables::{
    ProjectVariables, ProjectVariablesSet, ProjectVariablesSetResult,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// Public routes of project variables.
pub fn public() -> Router<AppState> {
    Router::new()
        .route("/api/sandboxes/{sid}/variables", get(list).put(set))
        .route("/api/sandboxes/{sid}/variables/{name}", delete(remove))
}

async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<ProjectVariables>> {
    Ok(Json(app.project_variables(&ctx, &sid).await?))
}

async fn set(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    Json(input): Json<ProjectVariablesSet>,
) -> ApiResult<Json<ProjectVariablesSetResult>> {
    Ok(Json(
        app.set_project_variables(&ctx, &sid, &input.variables, input.only_new)
            .await?,
    ))
}

async fn remove(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, name)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    app.delete_project_variable(&ctx, &sid, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}
