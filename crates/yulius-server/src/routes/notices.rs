//! A person's notices (SPEC-624): the live ones they see, and dismissing one for themselves.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::notices::NoticeView;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/me/notices`: live notices, newest first; expired ones are left out.
pub async fn mine(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<NoticeView>>> {
    Ok(Json(app.my_notices(&ctx).await?))
}

/// `DELETE /api/me/notices/{id}`: dismissed for the caller only.
pub async fn dismiss(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.dismiss_notice(&ctx.user, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
