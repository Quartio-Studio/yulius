//! Setup, sign-in, current user and CLI device approval.

use std::net::SocketAddr;

use axum::Json;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use yulius_app::auth::NewSession;
use yulius_app::model::Secret;
use yulius_contracts::api::auth::{
    DeviceApprove, DevicePoll, DevicePollRequest, DeviceStart, Me, PasswordLogin, RecoverRequest,
    RecoveryCode, RecoveryStatus, RegenerateRecovery, SetupRequest, SetupStatus,
};

use crate::AppState;
use crate::auth::{Auth, clear_cookie, session_cookie};
use crate::error::ApiResult;

fn with_cookie(app: &AppState, session: NewSession, status: StatusCode) -> Response {
    // Setup also returns the recovery code, once (ACC-011).
    let mut body = serde_json::to_value(&session.me).unwrap_or_default();
    if let (Some(code), Some(object)) = (&session.recovery_code, body.as_object_mut()) {
        object.insert("recovery_code".into(), code.expose().into());
    }
    let mut response = (status, Json(body)).into_response();
    let cookie = session_cookie(
        session.cookie.expose(),
        session.max_age_s,
        app.config.secure_cookies,
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

pub async fn setup_status(State(app): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    Ok(Json(SetupStatus {
        setup_required: app.setup_required().await?,
    }))
}

pub async fn setup(
    State(app): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> ApiResult<Response> {
    let session = app
        .complete_setup(
            &req.token,
            &Secret(req.password),
            &Secret(req.password_confirm),
            req.confirm_saved,
        )
        .await?;
    Ok(with_cookie(&app, session, StatusCode::CREATED))
}

pub async fn password_login(
    State(app): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<PasswordLogin>,
) -> ApiResult<Response> {
    let session = app
        .password_login(&Secret(req.password), &addr.ip().to_string())
        .await?;
    Ok(with_cookie(&app, session, StatusCode::OK))
}

#[derive(Deserialize)]
pub struct StartQuery {
    next: Option<String>,
}

pub async fn github_start(
    State(app): State<AppState>,
    Query(q): Query<StartQuery>,
) -> ApiResult<Redirect> {
    let url = app
        .github_sign_in_url(q.next.as_deref().unwrap_or("/"))
        .await?;
    Ok(Redirect::to(&url))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

pub async fn github_callback(
    State(app): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> ApiResult<Response> {
    let (session, next) = app.github_callback(&q.code, &q.state).await?;
    let mut response = Redirect::to(&next).into_response();
    let cookie = session_cookie(
        session.cookie.expose(),
        session.max_age_s,
        app.config.secure_cookies,
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub async fn logout(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Response> {
    app.logout(&ctx).await?;
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&clear_cookie(app.config.secure_cookies)) {
        headers.insert(header::SET_COOKIE, value);
    }
    Ok((StatusCode::NO_CONTENT, headers).into_response())
}

pub async fn me(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<Me>> {
    Ok(Json(app.me(&ctx.user, ctx.csrf_token.clone()).await?))
}

pub async fn device_start(State(app): State<AppState>) -> ApiResult<Json<DeviceStart>> {
    Ok(Json(app.device_start().await?))
}

pub async fn device_poll(
    State(app): State<AppState>,
    Json(req): Json<DevicePollRequest>,
) -> ApiResult<Json<DevicePoll>> {
    Ok(Json(app.device_poll(&req.device_code).await?))
}

pub async fn device_approve(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<DeviceApprove>,
) -> ApiResult<StatusCode> {
    app.device_approve(&ctx, &req.user_code, req.approve)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn recovery_status(State(app): State<AppState>) -> ApiResult<Json<RecoveryStatus>> {
    let state = app.recovery_state().await?;
    Ok(Json(RecoveryStatus {
        available: state.available,
        locked: state.locked,
        attempts_left: state.attempts_left,
    }))
}

pub async fn recover(
    State(app): State<AppState>,
    Json(req): Json<RecoverRequest>,
) -> ApiResult<Json<RecoveryCode>> {
    let code = app
        .recover_password(
            &req.code,
            &Secret(req.password),
            &Secret(req.password_confirm),
        )
        .await?;
    Ok(Json(RecoveryCode {
        recovery_code: code.expose().to_owned(),
    }))
}

pub async fn regenerate_recovery(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<RegenerateRecovery>,
) -> ApiResult<Json<RecoveryCode>> {
    let code = app
        .regenerate_recovery_code(&ctx, &Secret(req.password))
        .await?;
    Ok(Json(RecoveryCode {
        recovery_code: code.expose().to_owned(),
    }))
}
