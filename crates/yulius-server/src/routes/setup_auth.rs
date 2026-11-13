//! Setup, sign-in, current user and CLI device approval.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use yulius_app::auth::{GitHubSignInError, NewSession, local_return_path};
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
    let github_configured = app.github_configured().await?;
    Ok(Json(SetupStatus {
        setup_required: app.setup_required().await?,
        github_configured,
        github_callback_missing: github_configured && app.github_callback_missing(),
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

/// Starts GitHub sign-in: 303 to GitHub. Like the callback it never answers with an error body
/// (ACC-004): without a GitHub App it returns to the sign-in page saying so (SPEC-422).
pub async fn github_start(
    State(app): State<AppState>,
    client: ClientAddress,
    Query(q): Query<StartQuery>,
) -> Response {
    let select_account = q.prompt.as_deref() == Some("select_account");
    let next = q.next.as_deref().and_then(local_return_path);
    match app.github_configured().await {
        Ok(true) => {}
        Ok(false) => return sign_in_refused("github_unconfigured", next, None),
        Err(err) => {
            tracing::warn!(error = %err, "could not start GitHub sign-in");
            return sign_in_refused("failed", next, None);
        }
    }
    // Never stopped by what the instance knows of its callback, and no question to GitHub:
    // GitHub decides, and a sign-in that completes proves the callback (SPEC-466 MVC-013).
    match app
        .github_sign_in_url(next.unwrap_or("/"), select_account, &client.limit_key())
        .await
    {
        Ok((url, state)) => {
            // The state stays with the browser that started: the callback is honoured only
            // where this cookie is (SPEC-483 A2).
            let mut response = Redirect::to(&url).into_response();
            let max_age = yulius_app::auth::OAUTH_STATE_TTL_MS / 1000;
            set_cookie(
                &mut response,
                &oauth_cookie(&state, max_age, app.config.secure_cookies),
            );
            response
        }
        Err(yulius_app::error::AppError::Capacity(_)) => sign_in_refused("busy", next, None),
        Err(err) => {
            tracing::warn!(error = %err, "could not start GitHub sign-in");
            sign_in_refused("failed", next, None)
        }
    }
}

/// The cookie that keeps a started GitHub sign-in's state in the browser that started it.
const OAUTH_COOKIE: &str = "yulius_oauth";
/// Its name on HTTPS: `__Host-`, so no other host of the domain can set it.
const OAUTH_COOKIE_SECURE: &str = "__Host-yulius_oauth";

fn oauth_cookie_name(secure: bool) -> &'static str {
    if secure {
        OAUTH_COOKIE_SECURE
    } else {
        OAUTH_COOKIE
    }
}

fn oauth_cookie(state: &str, max_age_s: i64, secure: bool) -> String {
    let name = oauth_cookie_name(secure);
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={state}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_s}{secure}")
}

fn set_cookie(response: &mut Response, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// The state the starting browser kept, from its `Cookie` header.
fn started_state(headers: &HeaderMap, secure: bool) -> Option<&str> {
    let name = oauth_cookie_name(secure);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
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
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let secure = app.config.secure_cookies;
    let mut response = github_callback_answer(&app, &headers, q).await;
    // Used or not, the started sign-in's cookie goes.
    set_cookie(&mut response, &oauth_cookie("", 0, secure));
    response
}

async fn github_callback_answer(app: &AppState, headers: &HeaderMap, q: CallbackQuery) -> Response {
    // Only the browser that started this sign-in completes it: a callback link handed to
    // someone else would sign them in as the one who made it (SPEC-483 A2). The state is not
    // spent, so the starting browser can still finish.
    let started_here = |state: &str| {
        started_state(headers, app.config.secure_cookies)
            .is_some_and(|kept| app.same_secret(kept, state))
    };
    if !q.state.as_deref().is_some_and(started_here) {
        return sign_in_refused("expired", None, None);
    }
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
    set_cookie(&mut response, &cookie);
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
