//! Superadmin user administration.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use yulius_contracts::api::admin::{AdminUser, AdmitUser, UpdateUser};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list_users(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<AdminUser>>> {
    Ok(Json(app.admin_users(&ctx).await?))
}

pub async fn admit_user(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<AdmitUser>,
) -> ApiResult<(StatusCode, Json<AdminUser>)> {
    Ok((StatusCode::CREATED, Json(app.admit_user(&ctx, &req).await?)))
}

pub async fn update_user(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<AdminUser>> {
    Ok(Json(app.update_user(&ctx, &id, &req).await?))
}
