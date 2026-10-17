//! Leftovers on the server's disk (SPEC-215): the superadmin's routes and the loopback routes of
//! `yulius leftovers`, which present the key the server wrote to `<data>/leftovers-cli.key`.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use yulius_app::error::AppError;
use yulius_app::leftovers::Selection;
use yulius_contracts::api::leftovers::{DeleteLeftoversRequest, LeftoversView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/leftovers` (LFT-010).
pub async fn view(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<LeftoversView>> {
    Ok(Json(app.leftovers_view(&ctx).await?))
}

/// `POST /api/admin/leftovers/scan`.
pub async fn scan(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<LeftoversView>> {
    Ok(Json(app.scan_leftovers(&ctx).await?))
}

/// `POST /api/admin/leftovers/delete`.
pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(request): Json<DeleteLeftoversRequest>,
) -> ApiResult<Json<LeftoversView>> {
    Ok(Json(app.delete_leftovers_request(&ctx, request).await?))
}

/// The CLI's key from `Authorization: Bearer <key>`, checked against the server's.
fn cli_key(app: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if app.leftovers_key_matches(presented) {
        Ok(())
    } else {
        Err(AppError::Unauthenticated)
    }
}

/// `GET /internal/leftovers` (loopback, `yulius leftovers`).
pub async fn cli_view(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<LeftoversView>> {
    cli_key(&app, &headers)?;
    Ok(Json(app.leftovers_overview().await))
}

/// `POST /internal/leftovers/scan`.
pub async fn cli_scan(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<LeftoversView>> {
    cli_key(&app, &headers)?;
    Ok(Json(app.start_leftovers_scan("cli").await?))
}

/// `POST /internal/leftovers/delete`.
pub async fn cli_delete(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeleteLeftoversRequest>,
) -> ApiResult<Json<LeftoversView>> {
    cli_key(&app, &headers)?;
    Ok(Json(
        app.start_leftovers_delete("cli", Selection::from(request))
            .await?,
    ))
}
