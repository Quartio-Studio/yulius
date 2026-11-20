//! The end-of-day digest (SPEC-512 part 3): the instance's setting and a person's latest digest.

use axum::Json;
use axum::extract::State;
use yulius_contracts::api::digest::{DigestSetting, DigestView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/digest`.
pub async fn setting(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<DigestSetting>> {
    Ok(Json(app.digest_setting(&ctx).await?))
}

/// `PUT /api/admin/digest`.
pub async fn set_setting(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<DigestSetting>,
) -> ApiResult<Json<DigestSetting>> {
    Ok(Json(app.set_digest_setting(&ctx, &req).await?))
}

/// `GET /api/me/digest`: `null` before the first one.
pub async fn mine(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Option<DigestView>>> {
    Ok(Json(app.my_digest(&ctx).await?))
}
