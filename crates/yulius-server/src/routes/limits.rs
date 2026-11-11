//! The instance's resource limits (SPEC-459): superadmin-only.

use axum::Json;
use axum::extract::{Path, State};
use yulius_contracts::api::updates::{LimitsView, SetLimit};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/limits`.
pub async fn view(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<LimitsView>> {
    Ok(Json(app.limits_view(&ctx).await?))
}

/// `PUT /api/admin/limits/{key}`: a number, or `null` for host-sized.
pub async fn set(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(key): Path<String>,
    Json(req): Json<SetLimit>,
) -> ApiResult<Json<LimitsView>> {
    Ok(Json(app.set_limit(&ctx, &key, req.value).await?))
}

/// `DELETE /api/admin/limits/{key}`: back to the unit's value, else host-sized.
pub async fn remove(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(key): Path<String>,
) -> ApiResult<Json<LimitsView>> {
    Ok(Json(app.remove_limit(&ctx, &key).await?))
}

/// `POST /api/admin/limits/adapt`: host-sized for every pin that no longer fits this host.
pub async fn adapt(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<LimitsView>> {
    Ok(Json(app.adapt_limits(&ctx).await?))
}

/// `POST /api/admin/limits/keep`: the pins that no longer fit stay, recorded for this host.
pub async fn keep(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<LimitsView>> {
    Ok(Json(app.keep_limits(&ctx).await?))
}
