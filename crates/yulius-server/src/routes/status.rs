//! Status panel and subscription pool data (signed-in users).

use axum::Json;
use axum::extract::State;
use yulius_contracts::api::admin::CapacityView;
use yulius_contracts::api::status::StatusView;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn status(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<StatusView>> {
    Ok(Json(app.status(&ctx).await?))
}

/// `GET /api/capacity`: the subscription pool, burn rate and forecast (SPEC-001 ROT-013).
pub async fn capacity(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<CapacityView>> {
    Ok(Json(app.capacity_view(&ctx).await?))
}
