//! The personal Telegram channel (SPEC-444): the instance's bot and each person's linked chat.

use axum::Json;
use axum::extract::State;
use yulius_app::model::Secret;
use yulius_contracts::api::telegram::{
    MyTelegramView, SetTelegramBot, TelegramBotView, TelegramChoices, TelegramLinkCode,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// `GET /api/admin/telegram`.
pub async fn bot(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<TelegramBotView>> {
    Ok(Json(app.telegram_bot_view(&ctx).await?))
}

/// `PUT /api/admin/telegram`.
pub async fn set_bot(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<SetTelegramBot>,
) -> ApiResult<Json<TelegramBotView>> {
    Ok(Json(app.set_telegram_bot(&ctx, &Secret(req.token)).await?))
}

/// `DELETE /api/admin/telegram`.
pub async fn remove_bot(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<TelegramBotView>> {
    Ok(Json(app.remove_telegram_bot(&ctx).await?))
}

/// `GET /api/me/telegram`.
pub async fn me(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<MyTelegramView>> {
    Ok(Json(app.my_telegram(&ctx).await?))
}

/// `DELETE /api/me/telegram`.
pub async fn unlink(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<MyTelegramView>> {
    Ok(Json(app.unlink_telegram(&ctx).await?))
}

/// `POST /api/me/telegram/link`.
pub async fn link(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<TelegramLinkCode>> {
    Ok(Json(app.telegram_link_code(&ctx).await?))
}

/// `PUT /api/me/telegram/choices`.
pub async fn choices(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<TelegramChoices>,
) -> ApiResult<Json<MyTelegramView>> {
    Ok(Json(app.set_telegram_choices(&ctx, &req).await?))
}
