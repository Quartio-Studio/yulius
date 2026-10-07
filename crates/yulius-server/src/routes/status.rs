//! Status panel data (signed-in users).

use axum::Json;
use axum::extract::State;
use yulius_contracts::api::status::StatusView;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn status(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<StatusView>> {
    Ok(Json(app.status(&ctx).await?))
}
