//! A move to another host asked from Settings → Instance (SPEC-470): superadmin only.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use yulius_contracts::api::migration::{AskMigration, MigrationView};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/instance/migration`.
pub async fn view(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<MigrationView>> {
    Ok(Json(app.migration_view(&ctx).await?))
}

/// `POST /api/admin/instance/migration`: plan, move or abort.
pub async fn ask(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<AskMigration>,
) -> ApiResult<(StatusCode, Json<MigrationView>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(app.ask_migration(&ctx, &req).await?),
    ))
}
