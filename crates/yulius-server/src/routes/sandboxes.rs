//! Sandbox listing.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::sandboxes::Sandbox;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<Vec<Sandbox>>> {
    Ok(Json(app.list_sandboxes(&ctx).await?))
}

pub async fn get_one(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Sandbox>> {
    Ok(Json(app.get_sandbox(&ctx, &sid).await?))
}
