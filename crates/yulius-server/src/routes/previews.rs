//! Live preview endpoints on the Yulius origin (SPEC-002 ADR-14; docs/api.md "Live previews").
//!
//! - `GET /api/sandboxes/{sid}/previews` (write): the sandbox's preview origins and listening
//!   ports for the browser panel.
//! - `GET /api/previews/authorize?host&to` (write, a navigation): step 1 of the handshake;
//!   redirects to the preview origin's handshake URL with a single-use token.
//! - `GET /internal/preview-tls?domain` (loopback listener): the `ask` endpoint of an
//!   on-demand TLS proxy (Caddy); 200 only for preview hosts of existing sandbox ports.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_contracts::browser::PreviewInfo;

use crate::AppState;
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};

pub async fn info(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<PreviewInfo>> {
    Ok(Json(app.preview_info(&ctx, &sid).await?))
}

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    host: String,
    #[serde(default)]
    to: String,
}

fn see_other(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, value),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response(),
        Err(_) => ApiError(AppError::Invalid("invalid redirect".into())).into_response(),
    }
}

/// Step 1 of the preview handshake. Refusals for a valid preview host go to that preview's
/// own refusal page, which can be shown inside the browser panel (Yulius's pages cannot).
pub async fn authorize(
    State(app): State<AppState>,
    auth: Result<Auth, ApiError>,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    let Ok(target) = app.preview_target(&query.host).await else {
        return ApiError(AppError::NotFound).into_response();
    };
    let denied = |why: &str| see_other(&format!("{}/__yulius/denied?why={why}", target.origin()));
    let Ok(Auth(ctx)) = auth else {
        return denied("signin");
    };
    match app.preview_authorize(&ctx, &query.host, &query.to).await {
        Ok(location) => see_other(&location),
        Err(AppError::Forbidden(_) | AppError::NotFound) => denied("access"),
        Err(err) => ApiError(err).into_response(),
    }
}

#[derive(Deserialize)]
pub struct AskQuery {
    #[serde(default)]
    domain: String,
}

/// On-demand TLS `ask`: 200 approves a certificate for `domain`, 404 refuses it.
pub async fn tls_ask(State(app): State<AppState>, Query(query): Query<AskQuery>) -> StatusCode {
    if app.preview_tls_allowed(&query.domain).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}
