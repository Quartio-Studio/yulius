//! Superadmin recovery code (SPEC-001 ACC-002/011, AC-ACC-7): shown once at setup, three wrong
//! codes lock recovery for good (also under parallel guessing), a recovery resets the password,
//! ends sessions and issues a new code, and a signed-in superadmin can regenerate the code.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, call, harness};

const PASSWORD: &str = "a very long secret";
const NEW_PASSWORD: &str = "another long secret!";

async fn recover(
    h: &support::Harness,
    code: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    recover_with(h, code, NEW_PASSWORD).await
}

async fn recover_with(
    h: &support::Harness,
    code: &str,
    password: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        "/api/auth/recover",
        Some(json!({"code": code, "password": password, "password_confirm": password})),
        &Creds::default(),
    )
    .await
}

async fn status(h: &support::Harness) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(call(
        &h.router,
        Method::GET,
        "/api/auth/recovery",
        None,
        &Creds::default(),
    )
    .await?
    .body)
}

fn wrong_code(code: &str) -> String {
    let first = code
        .chars()
        .next()
        .and_then(|c| c.to_digit(10))
        .unwrap_or(0);
    format!("{}{}", (first + 1) % 10, &code[1..])
}

/// SPEC-242 PWM-001: recovery takes an 8-character password; 7 are refused with the minimum,
/// before the code is checked, so the refusal spends no attempt.
#[tokio::test]
async fn recovery_accepts_eight_characters_and_refuses_seven_without_spending_an_attempt()
-> TestResult {
    let h = harness()?;
    let token = h.app.prepare_setup().await?.ok_or("setup token")?;
    let setup = call(
        &h.router,
        Method::POST,
        "/api/setup",
        Some(json!({"token": token.expose(), "password": PASSWORD, "password_confirm": PASSWORD, "confirm_saved": true})),
        &Creds::default(),
    )
    .await?;
    let code = setup.body["recovery_code"]
        .as_str()
        .ok_or("code at setup")?
        .to_owned();
    let seven = recover_with(&h, &code, "zaq1@WS").await?;
    assert_eq!(seven.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        seven.body["error"]["message"],
        "the password needs at least 8 characters"
    );
    assert_eq!(
        status(&h).await?["attempts_left"],
        json!(3),
        "no attempt spent"
    );

    let eight = recover_with(&h, &code, "zaq1@WSX").await?;
    assert_eq!(eight.status, StatusCode::OK, "{:?}", eight.body);
    let login = call(
        &h.router,
        Method::POST,
        "/api/auth/password",
        Some(json!({"password": "zaq1@WSX"})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(
        login.status,
        StatusCode::OK,
        "the new 8-character password works"
    );
    Ok(())
}

#[tokio::test]
async fn the_recovery_code_resets_the_password_and_three_wrong_codes_lock_it() -> TestResult {
    let h = harness()?;
    let token = h.app.prepare_setup().await?.ok_or("setup token")?;
    let setup = call(
        &h.router,
        Method::POST,
        "/api/setup",
        Some(json!({"token": token.expose(), "password": PASSWORD, "password_confirm": PASSWORD, "confirm_saved": true})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(setup.status, StatusCode::CREATED);
    let code = setup.body["recovery_code"]
        .as_str()
        .ok_or("code at setup")?
        .to_owned();
    assert!(
        code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()),
        "{code}"
    );
    let session = Creds {
        cookie: support::session_from(&setup.set_cookie),
        csrf: setup.body["csrf_token"].as_str().map(str::to_owned),
        bearer: None,
    };
    let s = status(&h).await?;
    assert_eq!(
        (s["available"].clone(), s["attempts_left"].clone()),
        (json!(true), json!(3))
    );

    // A wrong code counts; the right one resets the password, ends sessions, issues a new code.
    let wrong = recover(&h, &wrong_code(&code)).await?;
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        wrong.body.to_string().contains("2 attempts left"),
        "{:?}",
        wrong.body
    );
    let ok = recover(&h, &code).await?;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
    let fresh = ok.body["recovery_code"]
        .as_str()
        .ok_or("new code")?
        .to_owned();
    let me = call(&h.router, Method::GET, "/api/me", None, &session).await?;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED, "old sessions end");
    let login = call(
        &h.router,
        Method::POST,
        "/api/auth/password",
        Some(json!({"password": NEW_PASSWORD})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(login.status, StatusCode::OK, "the new password works");
    let admin = Creds {
        cookie: support::session_from(&login.set_cookie),
        csrf: login.body["csrf_token"].as_str().map(str::to_owned),
        bearer: None,
    };
    let used = recover(&h, &code).await?;
    assert_eq!(
        used.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a used code is gone"
    );
    assert_eq!(status(&h).await?["attempts_left"], json!(2));

    // Parallel guesses cannot exceed the limit: recovery locks after three failures in total.
    let guess = wrong_code(&fresh);
    let guesses = (0..10).map(|_| recover(&h, &guess));
    let results = futures::future::join_all(guesses).await;
    let forbidden = results
        .iter()
        .filter(|r| r.as_ref().is_ok_and(|r| r.status == StatusCode::FORBIDDEN))
        .count();
    assert!(forbidden >= 8, "locked after the limit: {forbidden}");
    let locked = status(&h).await?;
    assert_eq!(
        (locked["available"].clone(), locked["locked"].clone()),
        (json!(false), json!(true))
    );
    let after = recover(&h, &fresh).await?;
    assert_eq!(
        after.status,
        StatusCode::FORBIDDEN,
        "even the right code no longer works"
    );
    assert!(
        h.app
            .deps
            .store
            .instance()
            .await?
            .ok_or("instance")?
            .recovery_hash
            .is_none(),
        "the code is erased"
    );

    // Only a signed-in superadmin with the password re-enables recovery.
    let anonymous = call(
        &h.router,
        Method::POST,
        "/api/admin/recovery-code",
        Some(json!({"password": NEW_PASSWORD})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    let bad = call(
        &h.router,
        Method::POST,
        "/api/admin/recovery-code",
        Some(json!({"password": "wrong password!!"})),
        &admin,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::FORBIDDEN);
    let regenerated = call(
        &h.router,
        Method::POST,
        "/api/admin/recovery-code",
        Some(json!({"password": NEW_PASSWORD})),
        &admin,
    )
    .await?;
    assert_eq!(regenerated.status, StatusCode::OK, "{:?}", regenerated.body);
    let s = status(&h).await?;
    assert_eq!(
        (s["available"].clone(), s["attempts_left"].clone()),
        (json!(true), json!(3))
    );
    Ok(())
}
