//! Status panel and subscription pool data (signed-in users).

use axum::Json;
use axum::extract::State;
use yulius_contracts::api::admin::CapacityView;
use yulius_contracts::api::status::{ConnectionsView, DiskView, StatusView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn status(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<StatusView>> {
    Ok(Json(app.status(&ctx).await?))
}

/// `GET /api/disk`: how full the server's data disk is (SPEC-276 RDA-007), for everyone signed in.
pub async fn disk(State(app): State<AppState>, Auth(_ctx): Auth) -> ApiResult<Json<DiskView>> {
    Ok(Json(app.disk_view().await))
}

/// `GET /api/connections`: the open connection troubles of the sandboxes the person can view and
/// of the egress proxy itself (SPEC-361 CHR-004).
pub async fn connections(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<ConnectionsView>> {
    Ok(Json(app.connections_view(&ctx.user).await?))
}

/// `GET /api/capacity`: the subscription pool, burn rate and forecast (SPEC-001 ROT-013).
pub async fn capacity(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<CapacityView>> {
    Ok(Json(app.capacity_view(&ctx).await?))
}
