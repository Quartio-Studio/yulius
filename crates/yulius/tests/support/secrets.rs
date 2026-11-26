//! A person enrolled for secrets and their unlocked browser session.

use crate::support;
use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{Creds, call};

pub(crate) fn minutes(n: i64) -> i64 {
    n * 60 * 1000
}

/// The current code of a base32 `seed` at the test clock's time, `steps` periods away.
pub(crate) fn code_at(
    h: &support::Harness,
    seed: &str,
    steps: i64,
) -> Result<String, Box<dyn std::error::Error>> {
    use yulius_domain::vault::totp::{code_from_hmac, counter, parse_otp};
    let config = parse_otp(seed)?;
    let now = h.app.now().0 + steps * 30_000;
    let hmac = h
        .app
        .deps
        .crypto
        .totp_hmac(config.algorithm, &config.secret, counter(now, 30));
    Ok(code_from_hmac(&hmac, 6))
}

pub(crate) async fn unlock(
    h: &support::Harness,
    creds: &Creds,
    code: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        "/api/auth/unlock",
        Some(json!({"code": code})),
        creds,
    )
    .await
}

/// Sets up and confirms two-factor for `creds`; returns the seed.
pub(crate) async fn enroll(
    h: &support::Harness,
    creds: &Creds,
) -> Result<String, Box<dyn std::error::Error>> {
    let setup = call(&h.router, Method::POST, "/api/auth/two-factor", None, creds).await?;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
    let seed = setup.body["secret"].as_str().ok_or("secret")?.to_owned();
    let confirmed = unlock(h, creds, &code_at(h, &seed, 0)?).await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    Ok(seed)
}

/// The id of the admitted user `login`.
pub(crate) async fn user_id(
    h: &support::Harness,
    superadmin: &Creds,
    login: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let users = call(&h.router, Method::GET, "/api/admin/users", None, superadmin).await?;
    Ok(users
        .body
        .as_array()
        .ok_or("users")?
        .iter()
        .find(|u| u["github_login"] == login)
        .and_then(|u| u["id"].as_str())
        .ok_or("no such user")?
        .to_owned())
}

/// The GitHub sign-in of an already admitted user (a new session).
pub(crate) async fn admit_again(
    h: &support::Harness,
    login: &str,
    id: u64,
) -> Result<Creds, Box<dyn std::error::Error>> {
    let code = format!("code-{login}");
    h.github.add_user(&code, id, login);
    let start = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/secrets",
        None,
        &Creds::default(),
    )
    .await?;
    let location = start.location.ok_or("location")?;
    let state = location.split("state=").nth(1).ok_or("state")?.to_owned();
    let callback = call(
        &h.router,
        Method::GET,
        &format!("/api/auth/github/callback?code={code}&state={state}"),
        None,
        &support::started(&state),
    )
    .await?;
    let cookie = support::session_from(&callback.set_cookie).ok_or("cookie")?;
    let creds = Creds {
        cookie: Some(cookie),
        ..Creds::default()
    };
    let me = call(&h.router, Method::GET, "/api/me", None, &creds).await?;
    Ok(Creds {
        csrf: me.body["csrf_token"].as_str().map(str::to_owned),
        ..creds
    })
}

/// A browser session of `user` with two-factor set up and the vault unlocked: what a person
/// needs to let the value of a secret allowed use by use go somewhere new (ADR-0317).
pub(crate) async fn unlocked(
    h: &support::Harness,
    user: &str,
) -> Result<Creds, Box<dyn std::error::Error>> {
    let session = browser_session(h, user).await?;
    enroll(h, &session).await?;
    Ok(session)
}

/// A browser session for a seeded user (signed in now).
pub(crate) async fn browser_session(
    h: &support::Harness,
    user: &str,
) -> Result<Creds, Box<dyn std::error::Error>> {
    use yulius_app::model::WebSessionRecord;
    use yulius_app::ports::store::SessionStore as _;
    let cookie = format!("session-cookie-of-{user}");
    let now = h.app.now();
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: yulius_domain::ids::UserId::parse(user)?,
            csrf_token: format!("csrf-of-{user}"),
            created_at: now,
            expires_at: now.plus_millis(minutes(600)),
            revoked_at: None,
        })
        .await?;
    Ok(Creds {
        cookie: Some(cookie),
        csrf: Some(format!("csrf-of-{user}")),
        bearer: None,
        oauth: None,
    })
}
