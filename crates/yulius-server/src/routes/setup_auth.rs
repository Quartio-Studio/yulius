//! Setup, sign-in, current user and CLI device approval.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use yulius_app::auth::{GitHubSignInError, NewSession};
use yulius_app::model::Secret;
use yulius_contracts::api::auth::{
    DeviceApprove, DevicePoll, DevicePollRequest, DeviceStart, Me, PasswordLogin, RecoverRequest,
    RecoveryCode, RecoveryStatus, RegenerateRecovery, SetupRequest, SetupStatus,
};

use crate::AppState;
use crate::auth::{Auth, clear_cookie, session_cookie};
use crate::client::ClientAddress;
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

/// Password sign-in, limited per client: the TCP peer, or the trusted proxy's client address
/// (ACC-013).
pub async fn password_login(
    State(app): State<AppState>,
    client: ClientAddress,
    Json(req): Json<PasswordLogin>,
) -> ApiResult<Response> {
    let session = app
        .password_login(&Secret(req.password), &client.limit_key())
        .await?;
    Ok(with_cookie(&app, session, StatusCode::OK))
}

#[derive(Deserialize)]
pub struct StartQuery {
    next: Option<String>,
    /// `select_account` shows GitHub's account picker (sign in with another GitHub account).
    prompt: Option<String>,
}

pub async fn github_start(
    State(app): State<AppState>,
    Query(q): Query<StartQuery>,
) -> ApiResult<Redirect> {
    let select_account = q.prompt.as_deref() == Some("select_account");
    let url = app
        .github_sign_in_url(q.next.as_deref().unwrap_or("/"), select_account)
        .await?;
    Ok(Redirect::to(&url))
}

/// GitHub's redirect: `code` and `state` on success, `error` (e.g. `access_denied`) and `state`
/// when the user did not authorize.
#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Percent-encodes a query value (RFC 3986 unreserved characters and `/` stay as they are).
fn query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The sign-in page explaining why GitHub sign-in did not complete (ACC-004): a refusal never
/// answers with a raw error body, and the callback URL (single use) does not stay in the address
/// bar, so a reload shows the sign-in page again.
fn sign_in_refused(reason: &str, next: Option<&str>, login: Option<&str>) -> Response {
    let mut url = format!("/signin?error={reason}");
    if let Some(next) = next {
        url.push_str("&next=");
        url.push_str(&query_value(next));
    }
    if let Some(login) = login {
        url.push_str("&login=");
        url.push_str(&query_value(login));
    }
    Redirect::to(&url).into_response()
}

pub async fn github_callback(
    State(app): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let (code, state) = match (q.code, q.state) {
        (Some(code), Some(state)) if q.error.is_none() => (code, state),
        (_, state) => {
            let next = match state {
                Some(state) => app.github_sign_in_abandoned(&state).await.unwrap_or_else(|err| {
                    tracing::warn!(error = %err, "could not forget an abandoned GitHub sign-in");
                    None
                }),
                None => None,
            };
            let reason = if q.error.is_some() {
                "denied"
            } else {
                "failed"
            };
            return sign_in_refused(reason, next.as_deref(), None);
        }
    };
    let (session, next) = match app.github_callback(&code, &state).await {
        Ok(done) => done,
        Err(err) => {
            let reason = err.reason();
            return match err {
                GitHubSignInError::NotAdmitted { login, next } => {
                    sign_in_refused(reason, Some(&next), Some(&login))
                }
                GitHubSignInError::Disabled { next } => sign_in_refused(reason, Some(&next), None),
                GitHubSignInError::Expired => sign_in_refused(reason, None, None),
                GitHubSignInError::Failed(err) => {
                    tracing::warn!(error = %err, "GitHub sign-in failed");
                    sign_in_refused(reason, None, None)
                }
            };
        }
    };
    let mut response = Redirect::to(&next).into_response();
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

/// CLI device sign-in start, limited per client like password sign-in (#169).
pub async fn device_start(
    State(app): State<AppState>,
    client: ClientAddress,
) -> ApiResult<Json<DeviceStart>> {
    Ok(Json(app.device_start(&client.limit_key()).await?))
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

/// The caller's API tokens (SPEC-247 RMT-001).
pub async fn tokens(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<yulius_contracts::api::auth::ApiTokenView>>> {
    Ok(Json(app.api_tokens(&ctx).await?))
}

/// A new API token of the caller, shown once.
pub async fn create_token(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<yulius_contracts::api::auth::CreateApiToken>,
) -> ApiResult<(
    StatusCode,
    Json<yulius_contracts::api::auth::CreatedApiToken>,
)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_api_token(&ctx, &req).await?),
    ))
}

/// Revokes one of the caller's API tokens.
pub async fn revoke_token(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> ApiResult<StatusCode> {
    app.revoke_api_token(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
