//! The secrets vault over the public API, the internal listener and the egress policy
//! (SPEC-001 VLT-001..009, SPEC-002 ADR-20): team secrets and re-authentication, encryption at
//! rest, access control, agent secrets applied to requests for their bound hosts only, the MCP
//! tools and redaction of MCP results, fingerprints and MCP `${secret:NAME}` references.

use crate::support;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, admit_and_sign_in, call, configure_github_app, mcp_tool, perms,
    seed_github_user, seed_live_attempt, seed_sandbox, seed_task, setup_superadmin_creds,
    superadmin_bearer, tool_text,
};
use yulius_app::ports::egress::{EgressDecision, EgressPolicy, EgressRequest};
use yulius_domain::task::TaskStatus;
use yulius_domain::vault::redact::{Matcher as _, redact_text};

const PASSWORD: &str = "a very long secret";
const MAIL_PASSWORD: &str = "gmail-team-Passw0rd-417";
const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
const STRIPE: &str = "sk_test_51NzQ9vaultValue8f3a";
const TOKEN_S1: &str = "vault-task-token-s1-0123456789abcdef";
const TOKEN_S2: &str = "vault-task-token-s2-0123456789abcdef";

fn minutes(n: i64) -> i64 {
    n * 60 * 1000
}

fn assert_hidden(body: &Value, values: &[&str]) {
    let text = body.to_string();
    for value in values {
        assert!(!text.contains(value), "{value} leaked in {text}");
    }
}

async fn create_team_secret(
    h: &support::Harness,
    creds: &Creds,
) -> Result<String, Box<dyn std::error::Error>> {
    let created = call(
        &h.router,
        Method::POST,
        "/api/team-secrets",
        Some(json!({"title": "Shared Gmail", "value": MAIL_PASSWORD,
            "notes": "recovery: 1111-2222"})),
        creds,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_hidden(&created.body, &[MAIL_PASSWORD, "1111-2222"]);
    Ok(created.body["id"].as_str().ok_or("id")?.to_owned())
}

/// AC-VLT-1, AC-VLT-2, SPEC-040 AC1/AC3: a title, a value and notes; shared list, reveals
/// behind a two-factor unlock, and the activity trail.
#[tokio::test]
async fn team_secrets_are_shared_and_reveals_need_a_two_factor_code() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let id = create_team_secret(&h, &admin).await?;

    let list = call(&h.router, Method::GET, "/api/team-secrets", None, &admin).await?;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body[0]["title"], "Shared Gmail");
    assert_eq!(list.body[0]["has_value"], true);
    assert_eq!(list.body[0]["has_notes"], true);
    assert!(list.body[0].get("username").is_none());
    assert_hidden(&list.body, &[MAIL_PASSWORD, "1111-2222"]);

    // Only a title, a value and notes (SPEC-040 R1).
    for legacy in ["url", "username", "password", "totp"] {
        let refused = call(
            &h.router,
            Method::POST,
            "/api/team-secrets",
            Some(json!({"title": "Old form", legacy: "x"})),
            &admin,
        )
        .await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{legacy}");
    }

    // Signed in a moment ago, even with the password confirmed: two-factor is mandatory
    // (SPEC-040 R4).
    let reveal = format!("/api/team-secrets/{id}/reveal");
    let refused = call(&h.router, Method::POST, &reveal, Some(json!({})), &admin).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.body["error"]["code"], "reauth_required");
    assert_eq!(refused.body["reauth"]["two_factor"], "missing");
    assert_hidden(&refused.body, &[MAIL_PASSWORD]);
    let no_codes = call(
        &h.router,
        Method::POST,
        &format!("/api/team-secrets/{id}/totp"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(
        no_codes.status,
        StatusCode::NOT_FOUND,
        "no per-secret codes"
    );
    // A wrong password is refused (403: the session stays); the right one confirms identity
    // but does not unlock.
    let wrong = call(
        &h.router,
        Method::POST,
        "/api/auth/reauth",
        Some(json!({"password": "not it"})),
        &admin,
    )
    .await?;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let ok = call(
        &h.router,
        Method::POST,
        "/api/auth/reauth",
        Some(json!({"password": PASSWORD})),
        &admin,
    )
    .await?;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.body["identity_fresh"], true);
    assert_eq!(ok.body["fresh"], false);
    let still = call(&h.router, Method::POST, &reveal, None, &admin).await?;
    assert_eq!(still.body["error"]["code"], "reauth_required");

    // Set up and confirmed: the code unlocks reveals for 10 minutes.
    let seed = enroll(&h, &admin).await?;
    let copy = call(
        &h.router,
        Method::POST,
        &reveal,
        Some(json!({"purpose": "copy", "fields": ["value"]})),
        &admin,
    )
    .await?;
    assert_eq!(copy.body["value"], MAIL_PASSWORD);
    assert!(copy.body["notes"].is_null(), "only the asked field");
    let full = call(&h.router, Method::POST, &reveal, None, &admin).await?;
    assert_eq!(full.body["notes"], "recovery: 1111-2222");
    let old_field = call(
        &h.router,
        Method::POST,
        &reveal,
        Some(json!({"fields": ["password"]})),
        &admin,
    )
    .await?;
    assert_eq!(old_field.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Ten minutes after the code, a new code.
    h.clock.0.fetch_add(minutes(10), Ordering::SeqCst);
    let again = call(&h.router, Method::POST, &reveal, None, &admin).await?;
    assert_eq!(again.body["error"]["code"], "reauth_required");
    assert_eq!(again.body["reauth"]["two_factor"], "on");
    let unlocked = unlock(&h, &admin, &code_at(&h, &seed, 0)?).await?;
    assert_eq!(unlocked.status, StatusCode::OK, "{}", unlocked.body);
    let shown = call(&h.router, Method::POST, &reveal, None, &admin).await?;
    assert_eq!(shown.body["value"], MAIL_PASSWORD);

    // API tokens never reveal.
    let bearer = superadmin_bearer(&h).await?;
    let token_reveal = call(&h.router, Method::POST, &reveal, None, &bearer).await?;
    assert_eq!(token_reveal.status, StatusCode::FORBIDDEN);
    assert_eq!(token_reveal.body["error"]["code"], "forbidden");

    // Every reveal, copy, code and confirmation is in the trail; no value is.
    let activity = call(
        &h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &admin,
    )
    .await?;
    let actions: Vec<&str> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    for action in [
        "vault.team.create",
        "vault.team.reveal",
        "vault.team.copy",
        "vault.reauth",
        "vault.reauth.failed",
    ] {
        assert!(
            actions.contains(&action),
            "{action} missing from {actions:?}"
        );
    }
    assert_hidden(&activity.body, &[MAIL_PASSWORD, "1111-2222"]);

    // Edit (an empty field clears it) and delete.
    let patched = call(
        &h.router,
        Method::PATCH,
        &format!("/api/team-secrets/{id}"),
        Some(json!({"notes": ""})),
        &admin,
    )
    .await?;
    assert_eq!(patched.status, StatusCode::OK);
    assert_eq!(patched.body["has_notes"], false);
    assert_eq!(patched.body["has_value"], true);
    let old_patch = call(
        &h.router,
        Method::PATCH,
        &format!("/api/team-secrets/{id}"),
        Some(json!({"totp": ""})),
        &admin,
    )
    .await?;
    assert_eq!(old_patch.status, StatusCode::UNPROCESSABLE_ENTITY);
    let deleted = call(
        &h.router,
        Method::DELETE,
        &format!("/api/team-secrets/{id}"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    Ok(())
}

/// The current code of a base32 `seed` at the test clock's time, `steps` periods away.
fn code_at(
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

/// A code that is not the current one (nor the ones a step away).
fn wrong_code(h: &support::Harness, seed: &str) -> Result<String, Box<dyn std::error::Error>> {
    let taken = [
        code_at(h, seed, -1)?,
        code_at(h, seed, 0)?,
        code_at(h, seed, 1)?,
    ];
    Ok((0..4)
        .map(|n| format!("{n}{n}{n}{n}{n}{n}"))
        .find(|c| !taken.contains(c))
        .ok_or("no wrong code")?)
}

async fn unlock(
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
async fn enroll(h: &support::Harness, creds: &Creds) -> Result<String, Box<dyn std::error::Error>> {
    let setup = call(&h.router, Method::POST, "/api/auth/two-factor", None, creds).await?;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
    let seed = setup.body["secret"].as_str().ok_or("secret")?.to_owned();
    let confirmed = unlock(h, creds, &code_at(h, &seed, 0)?).await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    Ok(seed)
}

/// AC3, AC4 (SPEC-040 R4–R6): two-factor is set up once per person, confirmed by a first code,
/// and unlocks the vault for 10 minutes; codes are single-use, checked within one step and
/// rate limited; API tokens cannot set it up or unlock.
#[tokio::test]
async fn two_factor_is_set_up_once_and_unlocks_the_vault() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let status = call(&h.router, Method::GET, "/api/auth/reauth", None, &admin).await?;
    assert_eq!(status.body["two_factor"], "missing");
    assert_eq!(status.body["fresh"], false);
    assert_eq!(status.body["identity_fresh"], true);

    let setup = call(
        &h.router,
        Method::POST,
        "/api/auth/two-factor",
        None,
        &admin,
    )
    .await?;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
    assert_eq!(setup.cache_control.as_deref(), Some("no-store"));
    let seed = setup.body["secret"].as_str().ok_or("secret")?.to_owned();
    assert_eq!(seed.len(), 32, "160 bits of base32");
    let uri = setup.body["uri"].as_str().ok_or("uri")?;
    assert!(uri.starts_with("otpauth://totp/"), "{uri}");
    assert!(uri.contains(&format!("secret={seed}")), "{uri}");
    let pending = call(&h.router, Method::GET, "/api/auth/reauth", None, &admin).await?;
    assert_eq!(pending.body["two_factor"], "pending");
    // Until confirmed, setting up again starts over with a new seed.
    let again = call(
        &h.router,
        Method::POST,
        "/api/auth/two-factor",
        None,
        &admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CREATED);
    let seed = again.body["secret"].as_str().ok_or("secret")?.to_owned();

    let wrong = unlock(&h, &admin, &wrong_code(&h, &seed)?).await?;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN, "{}", wrong.body);
    let code = code_at(&h, &seed, 0)?;
    let confirmed = unlock(&h, &admin, &code).await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    assert_eq!(confirmed.body["fresh"], true);
    assert_eq!(confirmed.body["two_factor"], "on");
    let replayed = unlock(&h, &admin, &code).await?;
    assert_eq!(replayed.status, StatusCode::FORBIDDEN, "a code works once");
    let replaced = call(
        &h.router,
        Method::POST,
        "/api/auth/two-factor",
        None,
        &admin,
    )
    .await?;
    assert_eq!(
        replaced.status,
        StatusCode::CONFLICT,
        "a confirmed seed stays"
    );

    // The next period's code unlocks; one from two periods ago does not.
    h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    let late = unlock(&h, &admin, &code_at(&h, &seed, -2)?).await?;
    assert_eq!(late.status, StatusCode::FORBIDDEN);
    let next = unlock(&h, &admin, &code_at(&h, &seed, 0)?).await?;
    assert_eq!(next.status, StatusCode::OK, "{}", next.body);
    // Ten minutes later the vault is locked again.
    h.clock.0.fetch_add(minutes(10), Ordering::SeqCst);
    let locked = call(&h.router, Method::GET, "/api/auth/reauth", None, &admin).await?;
    assert_eq!(locked.body["fresh"], false);

    // Five attempts a minute.
    for _ in 0..5 {
        let refused = unlock(&h, &admin, &wrong_code(&h, &seed)?).await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
    let limited = unlock(&h, &admin, &code_at(&h, &seed, 0)?).await?;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);

    // API tokens neither set up nor unlock.
    let bearer = superadmin_bearer(&h).await?;
    let token_setup = call(
        &h.router,
        Method::POST,
        "/api/auth/two-factor",
        None,
        &bearer,
    )
    .await?;
    assert_eq!(token_setup.status, StatusCode::FORBIDDEN);
    let token_unlock = unlock(&h, &bearer, "123456").await?;
    assert_eq!(token_unlock.status, StatusCode::FORBIDDEN);

    let activity = call(
        &h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &admin,
    )
    .await?;
    let actions: Vec<&str> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    for action in [
        "vault.2fa.setup",
        "vault.2fa.confirm",
        "vault.unlock",
        "vault.unlock.failed",
    ] {
        assert!(
            actions.contains(&action),
            "{action} missing from {actions:?}"
        );
    }
    assert_hidden(&activity.body, &[&seed, &code]);
    Ok(())
}

/// #295 (SPEC-061 AC2): seed B is set up in another tab while a code of pending seed A is being
/// checked (forced between the check and the accept). The code is refused, B stays pending, and
/// only a code of B confirms it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_code_confirms_only_the_seed_it_was_checked_against() -> TestResult {
    let crypto = Arc::new(support::HookedCrypto::new()?);
    let h = support::harness_with(support::HarnessOptions {
        crypto: Some(crypto.clone()),
        ..support::HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let first = call(
        &h.router,
        Method::POST,
        "/api/auth/two-factor",
        None,
        &admin,
    )
    .await?;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    let seed_a = first.body["secret"].as_str().ok_or("secret")?.to_owned();

    let seed_b = Arc::new(std::sync::Mutex::new(None::<String>));
    let (router, creds, out) = (h.router.clone(), admin.clone(), seed_b.clone());
    *crypto.on_seed_open.lock().map_err(|_| "hook")? = Some(Box::new(move || {
        let setup = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(call(
                &router,
                Method::POST,
                "/api/auth/two-factor",
                None,
                &creds,
            ))
        });
        if let Ok(setup) = setup
            && setup.status == StatusCode::CREATED
            && let Ok(mut out) = out.lock()
        {
            *out = setup.body["secret"].as_str().map(str::to_owned);
        }
    }));
    let refused = unlock(&h, &admin, &code_at(&h, &seed_a, 0)?).await?;
    let seed_b = seed_b
        .lock()
        .map_err(|_| "seed B")?
        .clone()
        .ok_or("seed B was not set up meanwhile")?;
    assert_ne!(seed_a, seed_b);
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(
        refused.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("set up again")),
        "{}",
        refused.body
    );
    let status = call(&h.router, Method::GET, "/api/auth/reauth", None, &admin).await?;
    assert_eq!(status.body["two_factor"], "pending", "{}", status.body);
    assert_eq!(status.body["fresh"], false);
    // Not a wrong code (it does not count toward the lockout), but audited as a failed unlock.
    {
        use yulius_app::ports::store::UserStore as _;
        use yulius_app::ports::vault::VaultStore as _;
        let me = h.store.superadmin().await?.ok_or("superadmin")?;
        let record = h
            .store
            .vault_two_factor(me.id.as_str())
            .await?
            .ok_or("seed")?;
        assert_eq!(record.failed_attempts, 0);
    }
    let activity = call(
        &h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &admin,
    )
    .await?;
    assert!(
        activity
            .body
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["action"] == "vault.unlock.failed")),
        "{}",
        activity.body
    );

    let confirmed = unlock(&h, &admin, &code_at(&h, &seed_b, 0)?).await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    assert_eq!(confirmed.body["two_factor"], "on");
    Ok(())
}

/// The id of the admitted user `login`.
async fn user_id(
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

async fn reset(
    h: &support::Harness,
    creds: &Creds,
    user: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::DELETE,
        &format!("/api/users/{user}/two-factor"),
        None,
        creds,
    )
    .await
}

/// #291 (SPEC-061 AC3, AC4): the superadmin and instance administrators reset a member's lost
/// two-factor after their own unlock; members, API tokens, locked vaults and self-resets are
/// refused. The member then sets two-factor up again, and the old seed's codes are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn administrators_reset_a_lost_two_factor() -> TestResult {
    use yulius_app::ports::vault::VaultStore as _;
    let crypto = Arc::new(support::HookedCrypto::new()?);
    let h = support::harness_with(support::HarnessOptions {
        crypto: Some(crypto.clone()),
        ..support::HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let anna = admit_and_sign_in(&h, &admin, "anna", 501, false).await?;
    let olga = admit_and_sign_in(&h, &admin, "olga", 502, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 503, false).await?;
    let ivan_token = support::user_with_token(&h, "u-ivan", "ivan", true).await?;
    let anna_id = user_id(&h, &admin, "anna").await?;
    let bob_id = user_id(&h, &admin, "bob").await?;
    let old_seed = enroll(&h, &anna).await?;
    let anna_status = |creds: Creds| {
        let router = h.router.clone();
        async move { call(&router, Method::GET, "/api/auth/reauth", None, &creds).await }
    };

    // Refusals change nothing (AC4).
    let member = reset(&h, &bob, &anna_id).await?;
    assert_eq!(member.status, StatusCode::FORBIDDEN, "{}", member.body);
    let token = reset(&h, &ivan_token, &anna_id).await?;
    assert_eq!(token.status, StatusCode::FORBIDDEN, "{}", token.body);
    assert!(
        token.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("reset only from the web UI")),
        "{}",
        token.body
    );
    let token = reset(&h, &superadmin_bearer(&h).await?, &anna_id).await?;
    assert_eq!(token.status, StatusCode::FORBIDDEN, "{}", token.body);
    let locked = reset(&h, &olga, &anna_id).await?;
    assert_eq!(locked.status, StatusCode::FORBIDDEN, "{}", locked.body);
    assert_eq!(locked.body["error"]["code"], "reauth_required");
    assert_eq!(locked.body["reauth"]["two_factor"], "missing");
    // Your own two-factor is refused before any code is asked for.
    let me = call(&h.router, Method::GET, "/api/me", None, &admin).await?;
    let own = me.body["user"]["id"].as_str().ok_or("own id")?;
    let own = reset(&h, &admin, own).await?;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.body);
    assert_eq!(own.body["error"]["code"], "forbidden", "{}", own.body);
    enroll(&h, &admin).await?;
    let unknown = reset(&h, &admin, "u-nobody").await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.body);
    let kept = anna_status(anna.clone()).await?;
    assert_eq!(
        (&kept.body["two_factor"], &kept.body["fresh"]),
        (&json!("on"), &json!(true))
    );

    // The superadmin resets it: the seed and Anna's unlock are gone (AC3).
    let done = reset(&h, &admin, &anna_id).await?;
    assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);
    let after = anna_status(anna.clone()).await?;
    assert_eq!(
        (&after.body["two_factor"], &after.body["fresh"]),
        (&json!("missing"), &json!(false))
    );
    let old = unlock(&h, &anna, &code_at(&h, &old_seed, 0)?).await?;
    assert_eq!(old.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", old.body);
    // An unlock that passed its code just before the reset and stores the unlock just after it
    // (review of SPEC-061) opens nothing: without a confirmed seed the vault stays locked.
    let anna_session = h
        .app
        .deps
        .crypto
        .sha256_hex(anna.cookie.as_deref().ok_or("cookie")?.as_bytes());
    h.store.vault_set_unlock(&anna_session, h.app.now()).await?;
    let raced = anna_status(anna.clone()).await?;
    assert_eq!(raced.body["fresh"], false, "{}", raced.body);
    let secret = create_team_secret(&h, &admin).await?;
    let reveal = call(
        &h.router,
        Method::POST,
        &format!("/api/team-secrets/{secret}/reveal"),
        None,
        &anna,
    )
    .await?;
    assert_eq!(
        reveal.body["error"]["code"], "reauth_required",
        "{}",
        reveal.body
    );
    assert_hidden(&reveal.body, &[MAIL_PASSWORD]);
    // Nor does it once a new pending seed is locked by ten wrong codes (confirmation review).
    h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    let setup = call(&h.router, Method::POST, "/api/auth/two-factor", None, &anna).await?;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
    let wrong = wrong_code(&h, setup.body["secret"].as_str().ok_or("secret")?)?;
    for _ in 0..2 {
        for _ in 0..5 {
            let refused = unlock(&h, &anna, &wrong).await?;
            assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
        }
        h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    }
    let locked_pending = anna_status(anna.clone()).await?;
    assert_eq!(
        (
            &locked_pending.body["two_factor"],
            &locked_pending.body["fresh"]
        ),
        (&json!("locked"), &json!(false)),
        "{}",
        locked_pending.body
    );
    // Nothing set up: nothing to reset, and nothing audited.
    let none = reset(&h, &admin, &bob_id).await?;
    assert_eq!(none.status, StatusCode::NO_CONTENT, "{}", none.body);

    // Anna sets up a new key; a code of the old one is refused. An instance administrator,
    // after their own unlock, resets it again.
    let new_seed = enroll(&h, &anna).await?;
    assert_ne!(new_seed, old_seed);
    h.clock.0.fetch_add(31_000, Ordering::SeqCst);
    let stale = unlock(&h, &anna, &code_at(&h, &old_seed, 0)?).await?;
    assert_eq!(stale.status, StatusCode::FORBIDDEN, "{}", stale.body);
    enroll(&h, &olga).await?;
    let by_admin = reset(&h, &olga, &anna_id).await?;
    assert_eq!(by_admin.status, StatusCode::NO_CONTENT, "{}", by_admin.body);
    let after = anna_status(anna.clone()).await?;
    assert_eq!(after.body["two_factor"], "missing");

    // Both resets are in the trail every member sees, naming who reset whom, without seeds.
    let activity = call(&h.router, Method::GET, "/api/secrets/activity", None, &bob).await?;
    let resets: Vec<(&str, &str)> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.2fa.reset")
        .filter_map(|e| Some((e["actor"].as_str()?, e["detail"].as_str()?)))
        .collect();
    assert_eq!(resets, vec![("olga", "anna"), ("superadmin", "anna")]);
    assert_hidden(&activity.body, &[&old_seed, &new_seed]);

    // A reset that lands while Anna's code is being checked: the code is refused, saying so.
    h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    let setup = call(&h.router, Method::POST, "/api/auth/two-factor", None, &anna).await?;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.body);
    let pending_seed = setup.body["secret"].as_str().ok_or("secret")?.to_owned();
    let (router, creds, target) = (h.router.clone(), olga.clone(), anna_id.clone());
    *crypto.on_seed_open.lock().map_err(|_| "hook")? = Some(Box::new(move || {
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(call(
                &router,
                Method::DELETE,
                &format!("/api/users/{target}/two-factor"),
                None,
                &creds,
            ))
        });
    }));
    let refused = unlock(&h, &anna, &code_at(&h, &pending_seed, 0)?).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(
        refused.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("was reset")),
        "{}",
        refused.body
    );
    assert_eq!(
        anna_status(anna.clone()).await?.body["two_factor"],
        "missing"
    );
    Ok(())
}

/// #291 (SPEC-061 AC5): instance administrators read the people and their two-factor state, to
/// reset it from the Users page; changing users stays the superadmin's.
#[tokio::test]
async fn instance_administrators_see_the_people_and_their_two_factor() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let anna = admit_and_sign_in(&h, &admin, "anna", 501, false).await?;
    let olga = admit_and_sign_in(&h, &admin, "olga", 502, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 503, false).await?;
    enroll(&h, &anna).await?;
    let pending = call(&h.router, Method::POST, "/api/auth/two-factor", None, &bob).await?;
    assert_eq!(pending.status, StatusCode::CREATED, "{}", pending.body);

    let listed = call(&h.router, Method::GET, "/api/admin/users", None, &olga).await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    let states: Vec<(&str, &str)> = listed
        .body
        .as_array()
        .ok_or("users")?
        .iter()
        .filter_map(|u| Some((u["github_login"].as_str()?, u["two_factor"].as_str()?)))
        .collect();
    for expected in [("anna", "on"), ("bob", "pending"), ("olga", "missing")] {
        assert!(states.contains(&expected), "{expected:?} not in {states:?}");
    }
    assert_hidden(&listed.body, &["secret", "ciphertext"]);
    let member = call(&h.router, Method::GET, "/api/admin/users", None, &bob).await?;
    assert_eq!(member.status, StatusCode::FORBIDDEN);
    let admit = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(json!({"github_login": "eve"})),
        &olga,
    )
    .await?;
    assert_eq!(admit.status, StatusCode::FORBIDDEN);
    let anna_id = user_id(&h, &admin, "anna").await?;
    let disable = call(
        &h.router,
        Method::PATCH,
        &format!("/api/admin/users/{anna_id}"),
        Some(json!({"disabled": true})),
        &olga,
    )
    .await?;
    assert_eq!(disable.status, StatusCode::FORBIDDEN);
    Ok(())
}

/// R6: ten wrong codes in a row lock two-factor (a stolen session cannot keep guessing) until
/// the person confirms it is them again; the lock is audited.
#[tokio::test]
async fn ten_wrong_codes_lock_two_factor_until_identity_is_confirmed() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let seed = enroll(&h, &admin).await?;
    for _ in 0..2 {
        h.clock.0.fetch_add(61_000, Ordering::SeqCst);
        for _ in 0..5 {
            let wrong = unlock(&h, &admin, &wrong_code(&h, &seed)?).await?;
            assert_eq!(wrong.status, StatusCode::FORBIDDEN);
        }
    }
    h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    let right = unlock(&h, &admin, &code_at(&h, &seed, 0)?).await?;
    assert_eq!(
        right.status,
        StatusCode::FORBIDDEN,
        "locked: {}",
        right.body
    );
    let status = call(&h.router, Method::GET, "/api/auth/reauth", None, &admin).await?;
    assert_eq!(status.body["two_factor"], "locked");
    // Once the setup's unlock expires, a reveal says why codes do not work.
    let id = create_team_secret(&h, &admin).await?;
    h.clock.0.fetch_add(minutes(10), Ordering::SeqCst);
    let refused = call(
        &h.router,
        Method::POST,
        &format!("/api/team-secrets/{id}/reveal"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(refused.body["error"]["code"], "reauth_required");
    let message = refused.body["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("Too many wrong codes"), "{message}");

    let confirmed = call(
        &h.router,
        Method::POST,
        "/api/auth/reauth",
        Some(json!({"password": PASSWORD})),
        &admin,
    )
    .await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    assert_eq!(confirmed.body["two_factor"], "on");
    let again = unlock(&h, &admin, &code_at(&h, &seed, 0)?).await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);

    let activity = call(
        &h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &admin,
    )
    .await?;
    assert!(
        activity
            .body
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["action"] == "vault.2fa.locked")),
        "{}",
        activity.body
    );
    Ok(())
}

/// AC4: setting up needs a recent sign-in or confirmation, so a stolen session alone cannot
/// enroll its own authenticator.
#[tokio::test]
async fn setting_up_two_factor_needs_a_recent_sign_in() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let anna = admit_and_sign_in(&h, &admin, "anna", 501, false).await?;
    h.clock.0.fetch_add(minutes(11), Ordering::SeqCst);
    let stale = call(&h.router, Method::POST, "/api/auth/two-factor", None, &anna).await?;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    assert_eq!(stale.body["error"]["code"], "reauth_required");
    assert_eq!(stale.body["reauth"]["identity_fresh"], false);
    assert_eq!(stale.body["reauth"]["methods"], json!(["github"]));
    let again = admit_again(&h, "anna", 501).await?;
    let seed = enroll(&h, &again).await?;
    assert!(!seed.is_empty());
    Ok(())
}

/// AC2 (SPEC-040 R2): a team secret stored in the old form keeps every value: the password is the
/// value, and the address, user name and seed lead the notes.
#[tokio::test]
async fn older_team_secrets_fold_into_a_value_and_notes() -> TestResult {
    use yulius_app::ports::vault::{VaultSecretRecord, VaultSection};
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let old = json!({"url": "https://mail.google.com", "username": "team@acme.test",
        "password": MAIL_PASSWORD, "notes": "recovery: 1111-2222", "totp": SEED});
    let envelope = h
        .app
        .deps
        .crypto
        .envelope_seal(b"old-gmail|team", old.to_string().as_bytes())?;
    let now = h.app.now();
    h.app
        .deps
        .store
        .vault_insert(&VaultSecretRecord {
            id: "old-gmail".into(),
            section: VaultSection::Team,
            sandbox_id: None,
            name: "Shared Gmail".into(),
            description: String::new(),
            hosts: Vec::new(),
            injection: "none".into(),
            header_name: None,
            fields: vec!["password".into(), "notes".into(), "totp".into()],
            envelope,
            created_by: "superadmin".into(),
            created_at: now,
            updated_by: "superadmin".into(),
            updated_at: now,
            value_updated_at: now,
            last_used_at: None,
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: yulius_app::ports::vault::EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
        })
        .await?;
    let list = call(&h.router, Method::GET, "/api/team-secrets", None, &admin).await?;
    assert_eq!(list.body[0]["has_value"], true);
    assert_eq!(list.body[0]["has_notes"], true);
    enroll(&h, &admin).await?;
    let shown = call(
        &h.router,
        Method::POST,
        "/api/team-secrets/old-gmail/reveal",
        None,
        &admin,
    )
    .await?;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.body);
    assert_eq!(shown.body["value"], MAIL_PASSWORD);
    assert_eq!(
        shown.body["notes"],
        format!(
            "Address: https://mail.google.com\nUser name: team@acme.test\n\
             Two-factor seed: {SEED}\n\nrecovery: 1111-2222"
        )
    );
    // A later edit seals the new form and keeps the folded notes.
    let renamed = call(
        &h.router,
        Method::PATCH,
        "/api/team-secrets/old-gmail",
        Some(json!({"title": "Team Gmail"})),
        &admin,
    )
    .await?;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);
    let again = call(
        &h.router,
        Method::POST,
        "/api/team-secrets/old-gmail/reveal",
        None,
        &admin,
    )
    .await?;
    assert_eq!(again.body["value"], MAIL_PASSWORD);
    assert_eq!(again.body["notes"], shown.body["notes"]);
    Ok(())
}

/// A GitHub member sees every team secret (flat), confirms identity by signing in with GitHub
/// again, and reveals after setting up two-factor.
#[tokio::test]
async fn members_confirm_with_a_new_github_sign_in() -> TestResult {
    let h = support::harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let id = create_team_secret(&h, &admin).await?;
    let anna = admit_and_sign_in(&h, &admin, "anna", 501, false).await?;
    let list = call(&h.router, Method::GET, "/api/team-secrets", None, &anna).await?;
    assert_eq!(list.body.as_array().map(Vec::len), Some(1));
    h.clock.0.fetch_add(minutes(30), Ordering::SeqCst);
    let reveal = format!("/api/team-secrets/{id}/reveal");
    let refused = call(&h.router, Method::POST, &reveal, None, &anna).await?;
    assert_eq!(refused.body["reauth"]["methods"], json!(["github"]));
    // No password to confirm with.
    let password = call(
        &h.router,
        Method::POST,
        "/api/auth/reauth",
        Some(json!({"password": "anything"})),
        &anna,
    )
    .await?;
    assert_eq!(password.status, StatusCode::FORBIDDEN);
    // Signing in with GitHub again confirms it is her: she sets up two-factor, whose first code
    // unlocks. The sign-in alone reveals nothing.
    let again = admit_again(&h, "anna", 501).await?;
    let signed_in = call(&h.router, Method::POST, &reveal, None, &again).await?;
    assert_eq!(signed_in.body["error"]["code"], "reauth_required");
    enroll(&h, &again).await?;
    let shown = call(&h.router, Method::POST, &reveal, None, &again).await?;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.body);
    assert_eq!(shown.body["value"], MAIL_PASSWORD);
    let activity = call(
        &h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &again,
    )
    .await?;
    assert!(activity.body.as_array().is_some_and(|a| {
        a.iter()
            .any(|e| e["actor"] == "anna" && e["action"] == "vault.team.reveal")
    }));
    Ok(())
}

/// The GitHub sign-in of an already admitted user (a new session).
async fn admit_again(
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
        &Creds::default(),
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

struct World {
    h: support::Harness,
    writer: Creds,
    reader: Creds,
    outsider: Creds,
    admin: Creds,
}

/// Sandboxes s1 (`app`) and s2 (`other`); alice writes s1, bob reads it, carol sees neither;
/// tasks t1 (s1) and t2 (s2) run with live MCP tokens.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = support::harness()?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    let outsider = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("alice", "acme/other", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN_S1).await?;
    seed_task(&h, "t2", "s2", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t2", TOKEN_S2).await?;
    setup_superadmin_creds(&h).await?;
    let admin = superadmin_bearer(&h).await?;
    Ok(World {
        h,
        writer,
        reader,
        outsider,
        admin,
    })
}

async fn add_agent_secret(
    w: &World,
    creds: &Creds,
    sandbox: &str,
    body: Value,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/agent-secrets"),
        Some(body),
        creds,
    )
    .await
}

/// A browser session for a seeded user (signed in now).
async fn browser_session(
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
    })
}

/// SPEC-040 R3/R7 (second review): an older agent secret's stored user name and TOTP seed stay
/// readable by its managers, as lines leading the notes.
#[tokio::test]
async fn older_agent_secrets_reveal_their_user_name_and_seed() -> TestResult {
    use yulius_app::ports::vault::{VaultSecretRecord, VaultSection};
    let w = world().await?;
    let old = json!({"username": "docs-admin", "value": "docs-pass-7731-xyz", "totp": SEED});
    let envelope =
        w.h.app
            .deps
            .crypto
            .envelope_seal(b"old-docs|agent", old.to_string().as_bytes())?;
    let now = w.h.app.now();
    w.h.app
        .deps
        .store
        .vault_insert(&VaultSecretRecord {
            id: "old-docs".into(),
            section: VaultSection::Agent,
            sandbox_id: Some(yulius_domain::ids::SandboxId::parse("s1")?),
            name: "DOCS_LOGIN".into(),
            description: "Docs admin login".into(),
            hosts: vec!["docs.example".into()],
            injection: "none".into(),
            header_name: None,
            fields: vec!["value".into(), "username".into(), "totp".into()],
            envelope,
            created_by: "u-alice".into(),
            created_at: now,
            updated_by: "u-alice".into(),
            updated_at: now,
            value_updated_at: now,
            last_used_at: None,
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: yulius_app::ports::vault::EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
        })
        .await?;
    let alice = browser_session(&w.h, "u-alice").await?;
    enroll(&w.h, &alice).await?;
    let shown = call(
        &w.h.router,
        Method::POST,
        "/api/agent-secrets/old-docs/reveal",
        None,
        &alice,
    )
    .await?;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.body);
    assert_eq!(shown.body["value"], "docs-pass-7731-xyz");
    assert_eq!(
        shown.body["notes"],
        format!("User name: docs-admin\nTwo-factor seed: {SEED}\n\nDocs admin login")
    );
    Ok(())
}

/// AC-VLT-5, SPEC-040 AC5: who may see, manage and read agent secrets; values come back only
/// from a reveal by someone who manages the secret, after a two-factor unlock.
#[tokio::test]
async fn agent_secrets_follow_sandbox_access_and_reveal_only_to_managers() -> TestResult {
    let w = world().await?;
    let stripe = json!({"name": "STRIPE_KEY", "description": "test-mode key",
        "hosts": ["api.stripe.test"], "injection": "bearer", "value": STRIPE});
    let created = add_agent_secret(&w, &w.writer, "s1", stripe.clone()).await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["placeholder"], "yulius-secret-STRIPE_KEY");
    assert_eq!(
        created.body["injection_text"],
        "Authorization: Bearer <value>"
    );
    assert_hidden(&created.body, &[STRIPE]);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();

    // Read access lists, cannot add or change; no access sees nothing.
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.reader,
    )
    .await?;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.body[0]["name"], "STRIPE_KEY");
    assert_eq!(listed.body[0]["can_manage"], false);
    assert_hidden(&listed.body, &[STRIPE]);
    let refused = add_agent_secret(
        &w,
        &w.reader,
        "s1",
        json!({"name": "OTHER", "hosts": ["a.test"], "value": "12345678"}),
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let patch = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"value": "replaced-value-1"})),
        &w.reader,
    )
    .await?;
    assert_eq!(patch.status, StatusCode::FORBIDDEN);
    let hidden = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.outsider,
    )
    .await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let hidden_patch = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"description": "x"})),
        &w.outsider,
    )
    .await?;
    assert_eq!(hidden_patch.status, StatusCode::NOT_FOUND);

    // Instance-wide secrets: instance administrators only.
    let global = json!({"scope": "instance", "name": "GLOBAL_KEY", "hosts": ["global.test"], "value": "global-value-123"});
    let by_writer = add_agent_secret(&w, &w.writer, "s1", global.clone()).await?;
    assert_eq!(by_writer.status, StatusCode::FORBIDDEN);
    let by_admin = add_agent_secret(&w, &w.admin, "s1", global).await?;
    assert_eq!(by_admin.status, StatusCode::CREATED, "{}", by_admin.body);
    let global_id = by_admin.body["id"].as_str().ok_or("id")?.to_owned();
    let other = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s2/agent-secrets",
        None,
        &w.writer,
    )
    .await?;
    let names: Vec<&str> = other
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["GLOBAL_KEY"],
        "instance-wide ones apply to every sandbox"
    );

    // Validation: short values, reserved names, duplicates, Basic without a user name.
    for (body, status) in [
        (
            json!({"name": "SHORT", "hosts": ["a.test"], "value": "1234567"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"name": "PATH", "hosts": ["a.test"], "value": "12345678"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"name": "LOCAL", "hosts": ["localhost"], "value": "12345678"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"name": "BASIC", "hosts": ["a.test"], "injection": "basic", "value": "12345678"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // No per-secret two-factor seed; a user name only for Basic (SPEC-040 R3).
        (
            json!({"name": "SEEDED", "hosts": ["a.test"], "value": "12345678", "totp": SEED}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"name": "LOGIN", "hosts": ["a.test"], "injection": "none", "username": "bob", "value": "12345678"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (stripe, StatusCode::CONFLICT),
        (
            json!({"name": "GLOBAL_KEY", "hosts": ["a.test"], "value": "12345678"}),
            StatusCode::CONFLICT,
        ),
    ] {
        let response = add_agent_secret(&w, &w.writer, "s1", body.clone()).await?;
        assert_eq!(response.status, status, "{body}: {}", response.body);
    }

    // People who manage it read it back after a two-factor unlock (SPEC-040 R7); readers
    // cannot, outsiders do not see it, API tokens never reveal.
    let reveal = format!("/api/agent-secrets/{id}/reveal");
    let writer = browser_session(&w.h, "u-alice").await?;
    let reader = browser_session(&w.h, "u-bob").await?;
    let outsider = browser_session(&w.h, "u-carol").await?;
    let locked = call(&w.h.router, Method::POST, &reveal, None, &writer).await?;
    assert_eq!(locked.status, StatusCode::FORBIDDEN);
    assert_eq!(locked.body["error"]["code"], "reauth_required");
    assert_hidden(&locked.body, &[STRIPE]);
    enroll(&w.h, &writer).await?;
    let shown = call(&w.h.router, Method::POST, &reveal, None, &writer).await?;
    assert_eq!(shown.status, StatusCode::OK, "{}", shown.body);
    assert_eq!(shown.cache_control.as_deref(), Some("no-store"));
    assert_eq!(shown.body["value"], STRIPE);
    assert_eq!(shown.body["notes"], "test-mode key");
    // An instance-wide secret is read only by instance administrators.
    let global_reveal = call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{global_id}/reveal"),
        None,
        &writer,
    )
    .await?;
    assert_eq!(global_reveal.status, StatusCode::FORBIDDEN);
    assert_hidden(&global_reveal.body, &["global-value-123"]);
    enroll(&w.h, &reader).await?;
    let by_reader = call(&w.h.router, Method::POST, &reveal, None, &reader).await?;
    assert_eq!(by_reader.status, StatusCode::FORBIDDEN);
    assert_hidden(&by_reader.body, &[STRIPE]);
    let by_outsider = call(&w.h.router, Method::POST, &reveal, None, &outsider).await?;
    assert_eq!(by_outsider.status, StatusCode::NOT_FOUND);
    let by_token = call(&w.h.router, Method::POST, &reveal, None, &w.admin).await?;
    assert_eq!(by_token.status, StatusCode::FORBIDDEN);
    assert_hidden(&by_token.body, &[STRIPE]);
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &writer,
    )
    .await?;
    assert!(
        activity.body.as_array().is_some_and(|a| a
            .iter()
            .any(|e| e["actor"] == "alice" && e["action"] == "vault.agent.reveal")),
        "{}",
        activity.body
    );
    assert_hidden(&activity.body, &[STRIPE]);

    // A replaced value is never returned either.
    let replaced = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"value": "sk_test_replaced_999"})),
        &w.writer,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK);
    assert_hidden(&replaced.body, &["sk_test_replaced_999"]);
    let removed = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/agent-secrets/{id}"),
        None,
        &w.writer,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    Ok(())
}

fn request(host: &str, path: &str, headers: &[(&str, &str)]) -> EgressRequest {
    EgressRequest {
        host: host.into(),
        port: 443,
        path_and_query: path.into(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), v.as_bytes().to_vec()))
            .collect(),
    }
}

fn header<'a>(decision: &'a EgressDecision, name: &str) -> Option<&'a str> {
    match decision {
        EgressDecision::Forward { headers, .. } => headers
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| std::str::from_utf8(v).ok()),
        EgressDecision::Refuse { .. } => None,
    }
}

/// AC-VLT-3: the application's egress policy — credentials only for bound hosts of the run's
/// sandbox, placeholders, Basic credentials, a named header, conflicts and choices.
#[tokio::test]
async fn the_egress_policy_adds_secrets_for_bound_hosts_only() -> TestResult {
    let w = world().await?;
    for body in [
        json!({"name": "STRIPE_KEY", "hosts": ["api.stripe.test"], "injection": "bearer", "value": STRIPE}),
        json!({"name": "SVC_PASS", "hosts": ["svc.test"], "injection": "basic", "username": "bob", "value": "svc-password-42"}),
        json!({"name": "HDR_KEY", "hosts": ["*.hdr.test"], "injection": "header", "header_name": "X-Api-Key", "value": "hdr-value-12345"}),
        json!({"name": "TWIN_A", "hosts": ["twin.test"], "injection": "bearer", "value": "twin-a-value-1"}),
        json!({"name": "TWIN_B", "hosts": ["twin.test"], "injection": "bearer", "value": "twin-b-value-2"}),
    ] {
        let created = add_agent_secret(&w, &w.writer, "s1", body).await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    }
    // Revealing Basic credentials gives the whole credential: the user name leads the notes.
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.writer,
    )
    .await?;
    let svc = listed
        .body
        .as_array()
        .and_then(|a| a.iter().find(|s| s["name"] == "SVC_PASS"))
        .and_then(|s| s["id"].as_str())
        .ok_or("SVC_PASS")?
        .to_owned();
    let alice = browser_session(&w.h, "u-alice").await?;
    enroll(&w.h, &alice).await?;
    let basic = call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{svc}/reveal"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(basic.body["value"], "svc-password-42");
    assert_eq!(basic.body["notes"], "User name: bob");
    let app: Arc<dyn EgressPolicy> = w.h.app.clone();
    assert!(app.egress_client("unknown-token").await.is_none());
    let s1 = app.egress_client(TOKEN_S1).await.ok_or("s1 client")?;
    let s2 = app.egress_client(TOKEN_S2).await.ok_or("s2 client")?;
    assert_eq!(s1.sandbox_id.as_str(), "s1");
    assert!(app.egress_intercepts(&s1, "api.stripe.test", 443).await);
    assert!(app.egress_intercepts(&s1, "eu.hdr.test", 443).await);
    assert!(
        !app.egress_intercepts(&s1, "hdr.test", 443).await,
        "*. needs a subdomain"
    );
    assert!(!app.egress_intercepts(&s1, "api.stripe.test", 8443).await);
    assert!(!app.egress_intercepts(&s1, "example.com", 443).await);
    assert!(
        !app.egress_intercepts(&s2, "api.stripe.test", 443).await,
        "another sandbox's secret"
    );

    // Injected header; the agent's proxy headers removed; identity encoding requested.
    let plain = app
        .egress_apply(
            &s1,
            request(
                "api.stripe.test",
                "/v1/charges",
                &[
                    ("proxy-authorization", "Basic x"),
                    ("accept-encoding", "gzip"),
                ],
            ),
        )
        .await;
    assert_eq!(
        header(&plain, "authorization"),
        Some(format!("Bearer {STRIPE}").as_str())
    );
    assert_eq!(header(&plain, "proxy-authorization"), None);
    assert_eq!(header(&plain, "accept-encoding"), Some("identity"));

    // Placeholders in a header and the query are replaced; one for an unbound secret is not.
    let placeholder = app
        .egress_apply(
            &s1,
            request(
                "api.stripe.test",
                "/v1/x?key=yulius-secret-STRIPE_KEY&other=yulius-secret-SVC_PASS",
                &[("authorization", "Bearer yulius-secret-STRIPE_KEY")],
            ),
        )
        .await;
    match &placeholder {
        EgressDecision::Forward {
            path_and_query,
            used,
            ..
        } => {
            assert_eq!(
                path_and_query,
                &format!("/v1/x?key={STRIPE}&other=yulius-secret-SVC_PASS")
            );
            assert_eq!(used, &vec!["STRIPE_KEY".to_owned()]);
        }
        EgressDecision::Refuse { .. } => return Err("refused".into()),
    }
    assert_eq!(
        header(&placeholder, "authorization"),
        Some(format!("Bearer {STRIPE}").as_str())
    );

    // Basic credentials and a named header.
    let basic = app.egress_apply(&s1, request("svc.test", "/", &[])).await;
    assert_eq!(
        header(&basic, "authorization"),
        Some("Basic Ym9iOnN2Yy1wYXNzd29yZC00Mg==")
    );
    let named = app
        .egress_apply(&s1, request("eu.hdr.test", "/", &[]))
        .await;
    assert_eq!(header(&named, "x-api-key"), Some("hdr-value-12345"));
    // The agent's own header stays (no injection over it).
    let own = app
        .egress_apply(
            &s1,
            request("api.stripe.test", "/", &[("authorization", "Bearer mine")]),
        )
        .await;
    assert_eq!(header(&own, "authorization"), Some("Bearer mine"));

    // Two secrets for one header: refused until one is chosen.
    let twins = app.egress_apply(&s1, request("twin.test", "/", &[])).await;
    match twins {
        EgressDecision::Refuse { status, message } => {
            assert_eq!(status, 409);
            assert!(
                message.contains("TWIN_A, TWIN_B") && message.contains("X-Yulius-Secret"),
                "{message}"
            );
        }
        EgressDecision::Forward { .. } => return Err("ambiguous injection was forwarded".into()),
    }
    let chosen = app
        .egress_apply(
            &s1,
            request("twin.test", "/", &[("x-yulius-secret", "TWIN_B")]),
        )
        .await;
    assert_eq!(
        header(&chosen, "authorization"),
        Some("Bearer twin-b-value-2")
    );
    assert_eq!(header(&chosen, "x-yulius-secret"), None);
    let unknown = app
        .egress_apply(
            &s1,
            request("twin.test", "/", &[("x-yulius-secret", "STRIPE_KEY")]),
        )
        .await;
    assert!(matches!(
        unknown,
        EgressDecision::Refuse { status: 403, .. }
    ));

    // Responses are redacted of every in-scope value and encoding.
    let matcher = app.egress_redactor(&s1).await;
    let (redacted, names) = redact_text(
        matcher.as_ref(),
        &format!(
            "echo: Bearer {STRIPE} hex {}",
            yulius_domain::vault::encode::hex(STRIPE.as_bytes(), false)
        ),
    );
    assert_eq!(
        redacted,
        "echo: Bearer [secret:STRIPE_KEY] hex [secret:STRIPE_KEY]"
    );
    assert_eq!(names, vec!["STRIPE_KEY".to_owned()]);
    assert!(app.egress_redactor(&s2).await.is_empty());

    // Uses are audited by name.
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    assert!(activity.body.as_array().is_some_and(|a| {
        a.iter().any(|e| {
            e["action"] == "vault.agent.use"
                && e["actor"].as_str().is_some_and(|a| a.starts_with("task:"))
        })
    }));
    assert_hidden(&activity.body, &[STRIPE, "svc-password-42"]);
    let unseen = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.outsider,
    )
    .await?;
    assert!(
        unseen
            .body
            .as_array()
            .is_some_and(|a| a.iter().all(|e| e["action"] != "vault.agent.use")),
        "another sandbox's activity stays hidden"
    );
    Ok(())
}

/// SPEC-142 CSE-004, CSE-005: a CI job's proxy client (`ci:<run>`) uses only the secrets its job
/// was granted, only toward their bound hosts, audited under the run; its credential ends with the
/// job.
#[tokio::test]
async fn a_ci_job_uses_only_its_granted_secrets_toward_their_hosts() -> TestResult {
    let w = world().await?;
    for body in [
        json!({"name": "STRIPE_KEY", "hosts": ["api.stripe.test"], "injection": "bearer", "value": STRIPE}),
        json!({"name": "HDR_KEY", "hosts": ["*.hdr.test", "api.stripe.test"], "injection": "header", "header_name": "X-Api-Key", "value": "hdr-value-12345"}),
        json!({"name": "PROD_KEY", "hosts": ["prod.test"], "injection": "bearer", "value": "prod-value-12345"}),
    ] {
        let created = add_agent_secret(&w, &w.writer, "s1", body).await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        if created.body["name"] == "PROD_KEY" {
            use yulius_app::ports::vault::VaultStore as _;
            let id = created.body["id"].as_str().ok_or("id")?;
            assert!(w.h.store.vault_set_cd_prod(id, false, true).await?);
        }
    }
    let s1 = yulius_domain::ids::SandboxId::parse("s1")?;
    let granted = ["STRIPE_KEY".to_owned(), "PROD_KEY".to_owned()];
    let token =
        w.h.app
            .ci_egress_open(&s1, "run-7", "job-7a", &granted)
            .ok_or("no CI credential")?;
    let app: Arc<dyn EgressPolicy> = w.h.app.clone();
    let ci = app.egress_client(&token).await.ok_or("ci client")?;
    assert_eq!(ci.actor(), "ci:run-7");
    assert_eq!(ci.sandbox_id.as_str(), "s1");
    assert!(app.egress_intercepts(&ci, "api.stripe.test", 443).await);
    assert!(
        !app.egress_intercepts(&ci, "eu.hdr.test", 443).await,
        "a host bound only to a secret the job was not granted is a plain tunnel"
    );
    // A production-deploy secret is never a CI job's, even when its name is granted.
    assert!(!app.egress_intercepts(&ci, "prod.test", 443).await);
    let named = app
        .egress_apply(
            &ci,
            request("prod.test", "/", &[("x-yulius-secret", "PROD_KEY")]),
        )
        .await;
    assert!(matches!(named, EgressDecision::Refuse { status: 403, .. }));
    let prod = app
        .egress_apply(&ci, request("prod.test", "/?k=yulius-secret-PROD_KEY", &[]))
        .await;
    match &prod {
        EgressDecision::Forward {
            path_and_query,
            used,
            ..
        } => {
            assert_eq!(path_and_query, "/?k=yulius-secret-PROD_KEY");
            assert!(used.is_empty(), "{used:?}");
        }
        EgressDecision::Refuse { .. } => return Err("refused".into()),
    }
    assert_eq!(header(&prod, "authorization"), None);

    // The granted secret is injected and substituted; the other one, bound to the same host,
    // is neither injected nor substituted, and naming it is refused.
    let applied = app
        .egress_apply(
            &ci,
            request(
                "api.stripe.test",
                "/v1/x?key=yulius-secret-STRIPE_KEY&other=yulius-secret-HDR_KEY",
                &[],
            ),
        )
        .await;
    match &applied {
        EgressDecision::Forward {
            path_and_query,
            used,
            ..
        } => {
            assert_eq!(
                path_and_query,
                &format!("/v1/x?key={STRIPE}&other=yulius-secret-HDR_KEY")
            );
            assert_eq!(used, &vec!["STRIPE_KEY".to_owned()]);
        }
        EgressDecision::Refuse { .. } => return Err("refused".into()),
    }
    assert_eq!(
        header(&applied, "authorization"),
        Some(format!("Bearer {STRIPE}").as_str())
    );
    assert_eq!(header(&applied, "x-api-key"), None);
    let named = app
        .egress_apply(
            &ci,
            request("api.stripe.test", "/", &[("x-yulius-secret", "HDR_KEY")]),
        )
        .await;
    assert!(matches!(named, EgressDecision::Refuse { status: 403, .. }));
    let elsewhere = app
        .egress_apply(
            &ci,
            request("eu.hdr.test", "/?k=yulius-secret-HDR_KEY", &[]),
        )
        .await;
    assert_eq!(header(&elsewhere, "x-api-key"), None);
    match &elsewhere {
        EgressDecision::Forward { used, .. } => assert!(used.is_empty(), "{used:?}"),
        EgressDecision::Refuse { .. } => return Err("refused".into()),
    }

    // The use is audited under the run, never with the value.
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    assert!(
        activity.body.as_array().is_some_and(|a| a.iter().any(|e| {
            e["action"] == "vault.agent.use"
                && e["actor"] == "ci:run-7"
                && e["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("STRIPE_KEY") && d.contains("api.stripe.test"))
        })),
        "{}",
        activity.body
    );
    assert!(
        activity.body.as_array().is_some_and(|a| a
            .iter()
            .all(|e| !(e["actor"] == "ci:run-7" && e["detail"].to_string().contains("HDR_KEY")))),
        "{}",
        activity.body
    );
    assert_hidden(&activity.body, &[STRIPE, "hdr-value-12345"]);

    // The job ended: its credential names no client any more, and it never was a task's.
    w.h.app.ci_egress_close(&token);
    assert!(app.egress_client(&token).await.is_none());
    let empty =
        w.h.app
            .ci_egress_open(&s1, "run-8", "job-8a", &[])
            .ok_or("no CI credential")?;
    let none = app.egress_client(&empty).await.ok_or("ci client")?;
    assert!(!app.egress_intercepts(&none, "api.stripe.test", 443).await);
    w.h.app.ci_egress_close(&empty);
    Ok(())
}

/// VLT-005, VLT-007: `secrets_list` gives names only, every MCP result is redacted, the
/// fingerprints endpoint gives no value but finds one.
#[tokio::test]
async fn agents_list_names_and_read_redacted_results() -> TestResult {
    let w = world().await?;
    let created = add_agent_secret(&w, &w.writer, "s1", json!({"name": "STRIPE_KEY", "description": "Stripe test key", "hosts": ["api.stripe.test"], "value": STRIPE})).await?;
    assert_eq!(created.status, StatusCode::CREATED);

    let listed = mcp_tool(&w.h, TOKEN_S1, "secrets_list", json!({})).await?;
    let text = tool_text(&listed);
    assert_eq!(text["secrets"][0]["name"], "STRIPE_KEY");
    assert_eq!(text["secrets"][0]["hosts"], json!(["api.stripe.test"]));
    assert_eq!(
        text["secrets"][0]["placeholder"],
        "yulius-secret-STRIPE_KEY"
    );
    assert_hidden(&listed, &[STRIPE]);
    let other = tool_text(&mcp_tool(&w.h, TOKEN_S2, "secrets_list", json!({})).await?);
    assert_eq!(
        other["secrets"],
        json!([]),
        "another sandbox's agent sees nothing"
    );

    // A task prompt (or any text a tool returns) holding the value comes back redacted, in
    // every encoding.
    let task = seed_task(&w.h, "t3", "s1", "u-alice", TaskStatus::Running, None).await?;
    {
        use yulius_app::ports::store::TaskStore as _;
        let mut record = task.clone();
        record.prompt = format!(
            "use {STRIPE} and {}",
            yulius_domain::vault::encode::base64(format!("k={STRIPE}").as_bytes(), false, true)
        );
        w.h.store
            .insert_task(&yulius_app::model::TaskRecord {
                id: yulius_domain::ids::TaskId::parse("t4")?,
                ..record
            })
            .await?;
    }
    let got = mcp_tool(&w.h, TOKEN_S1, "task_get", json!({"task_id": "t4"})).await?;
    let raw = got.to_string();
    assert!(!raw.contains(STRIPE), "{raw}");
    assert!(raw.contains("[secret:STRIPE_KEY]"), "{raw}");

    // Fingerprints: no value, but enough to find one.
    let fingerprints = call(
        &w.h.internal,
        Method::POST,
        "/internal/scrub/fingerprints",
        Some(json!({})),
        &Creds {
            bearer: Some(TOKEN_S1.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(fingerprints.status, StatusCode::OK);
    assert_hidden(&fingerprints.body, &[STRIPE]);
    let set: yulius_contracts::scrub::FingerprintSet = serde_json::from_value(fingerprints.body)?;
    let matcher = yulius_harness::scrub_client::matcher(&set).ok_or("matcher")?;
    let (out, _) = redact_text(&matcher, &format!("STRIPE={STRIPE}\n"));
    assert_eq!(out, "STRIPE=[secret:STRIPE_KEY]\n");
    let refused = call(
        &w.h.internal,
        Method::POST,
        "/internal/scrub/fingerprints",
        Some(json!({})),
        &Creds {
            bearer: Some("nope".into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    let report = call(
        &w.h.internal,
        Method::POST,
        "/internal/scrub/report",
        Some(json!({"channel": "bash", "names": ["STRIPE_KEY"]})),
        &Creds {
            bearer: Some(TOKEN_S1.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(report.status, StatusCode::NO_CONTENT);
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    assert!(
        activity
            .body
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["action"] == "vault.agent.redacted"))
    );
    Ok(())
}

/// VLT-009, AC-VLT-8: MCP servers' `${secret:NAME}` references resolve to agent secrets.
#[tokio::test]
async fn mcp_secret_references_resolve_to_agent_secrets() -> TestResult {
    let w = world().await?;
    add_agent_secret(&w, &w.writer, "s1", json!({"name": "APIFY_TOKEN", "hosts": ["api.apify.test"], "injection": "none", "value": "apify-token-value-77"})).await?;
    let server = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(
            json!({"name": "apify", "transport": "stdio", "command": "npx", "args": ["apify-mcp"],
            "env": {"APIFY_TOKEN": "${secret:APIFY_TOKEN}", "MISSING": "${secret:NOT_THERE}"}}),
        ),
        &w.writer,
    )
    .await?;
    assert_eq!(server.status, StatusCode::CREATED, "{}", server.body);
    use yulius_app::ports::store::SandboxStore as _;
    let sandbox =
        w.h.store
            .sandbox(&yulius_domain::ids::SandboxId::parse("s1")?)
            .await?
            .ok_or("sandbox")?;
    let attachments = w.h.app.mcp_attachments(&sandbox).await?;
    let env = attachments
        .iter()
        .find_map(|a| match a {
            yulius_app::ports::harness::McpAttachment::Stdio { name, env, .. }
                if name == "apify" =>
            {
                Some(env.clone())
            }
            _ => None,
        })
        .ok_or("apify attachment")?;
    assert_eq!(
        env,
        vec![("APIFY_TOKEN".to_owned(), "apify-token-value-77".to_owned())]
    );
    // Another sandbox's servers cannot reach s1's secret.
    let other =
        w.h.store
            .sandbox(&yulius_domain::ids::SandboxId::parse("s2")?)
            .await?
            .ok_or("sandbox")?;
    assert_eq!(
        w.h.app.resolve_secret_ref(&other, "APIFY_TOKEN").await?,
        None
    );
    Ok(())
}

/// AC-VLT-7: no value, user name or seed in the database file, its WAL or a backup; the API
/// still reads them back.
#[tokio::test]
async fn nothing_is_stored_in_clear() -> TestResult {
    let dir = tempfile::tempdir()?;
    let data = dir.path().canonicalize()?;
    let store = yulius_store::SqliteStore::open(&data.join("yulius.db"))?;
    let h = support::harness_with(support::HarnessOptions {
        store: Some(store.clone()),
        ..support::HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let id = create_team_secret(&h, &admin).await?;
    let w_user = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    let agent = call(&h.router, Method::POST, "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "STRIPE_KEY", "hosts": ["api.stripe.test"], "injection": "basic", "username": "stripe-user-x", "value": STRIPE})), &w_user).await?;
    assert_eq!(agent.status, StatusCode::CREATED, "{}", agent.body);
    let seed = enroll(&h, &admin).await?;
    let backup = data.join("backup.db");
    store.backup_to(&backup).await?;
    let mut files = vec![data.join("yulius.db"), backup];
    let wal = data.join("yulius.db-wal");
    if wal.exists() {
        files.push(wal);
    }
    for path in &files {
        let bytes = std::fs::read(path)?;
        for needle in [
            MAIL_PASSWORD,
            "1111-2222",
            SEED,
            "team@acme.test",
            STRIPE,
            "stripe-user-x",
            "mail.google.com",
            &seed,
        ] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} in clear in {}",
                path.display()
            );
        }
    }
    // Encrypted, not lost.
    let shown = call(
        &h.router,
        Method::POST,
        &format!("/api/team-secrets/{id}/reveal"),
        None,
        &admin,
    )
    .await?;
    assert_eq!(shown.body["value"], MAIL_PASSWORD);
    Ok(())
}

/// The egress proxy is served on the internal listener (SPEC-002 ADR-20): a `CONNECT` with the
/// run's task token opens a tunnel, without it 407; the internal API keeps working beside it.
#[tokio::test]
async fn the_proxy_shares_the_internal_listener() -> TestResult {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let w = world().await?;
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let echo_addr = echo.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    let proxy = yulius::wiring::egress::egress_proxy(
        &w.h.app,
        yulius_egress::EgressConfig {
            resolve: std::collections::HashMap::from([("tunnel.test".to_owned(), echo_addr)]),
            ..yulius_egress::EgressConfig::default()
        },
    )
    .await?;
    let router = yulius::wiring::egress::with_egress(
        yulius_server::internal_router(w.h.app.clone()),
        proxy,
        &w.h.app.config.internal_url,
    )
    .layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from(([127, 0, 0, 1], 5001)),
    ));
    let served = support::serve(router).await?;

    async fn exchange(
        addr: std::net::SocketAddr,
        head: &str,
    ) -> Result<(tokio::net::TcpStream, String), Box<dyn std::error::Error>> {
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        stream.write_all(head.as_bytes()).await?;
        let mut buffer = vec![0u8; 1024];
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buffer))
            .await??;
        Ok((stream, String::from_utf8_lossy(&buffer[..n]).into_owned()))
    }
    let (_, refused) = exchange(
        served.addr,
        "CONNECT tunnel.test:443 HTTP/1.1\r\nHost: tunnel.test:443\r\n\r\n",
    )
    .await?;
    assert!(refused.starts_with("HTTP/1.1 407"), "{refused}");
    use base64::Engine as _;
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("yulius:{TOKEN_S1}"));
    let (mut tunnel, opened) = exchange(
        served.addr,
        &format!("CONNECT tunnel.test:443 HTTP/1.1\r\nHost: tunnel.test:443\r\nProxy-Authorization: Basic {credentials}\r\n\r\n"),
    )
    .await?;
    assert!(opened.starts_with("HTTP/1.1 200"), "{opened}");
    tunnel.write_all(b"ping through the tunnel\n").await?;
    let mut buffer = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), tunnel.read(&mut buffer))
        .await??;
    assert_eq!(&buffer[..n], b"ping through the tunnel\n");
    // A local destination is refused even with credentials.
    let (_, local) = exchange(
        served.addr,
        &format!("CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: x\r\nProxy-Authorization: Basic {credentials}\r\n\r\n", echo_addr.port()),
    )
    .await?;
    assert!(local.starts_with("HTTP/1.1 403"), "{local}");
    // The internal API is unchanged.
    let (_, health) = exchange(
        served.addr,
        "GET /internal/healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .await?;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    Ok(())
}

/// SPEC-121 ASH-002, AC-ASH-2: a secret added with a name, a value and a description only is
/// bound to no host and has no injection: the proxy sends it nowhere until hosts are allowed.
#[tokio::test]
async fn a_secret_without_hosts_is_sent_nowhere() -> TestResult {
    let w = world().await?;
    let created = add_agent_secret(
        &w,
        &w.writer,
        "s1",
        json!({"name": "VIDOC_API_KEY", "description": "API key for Vidoc", "value": "vidoc-key-123456"}),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["hosts"], json!([]));
    assert_eq!(created.body["injection"], "none");
    let app: Arc<dyn EgressPolicy> = w.h.app.clone();
    let s1 = app.egress_client(TOKEN_S1).await.ok_or("s1 client")?;
    for host in ["api.vidoc.test", "vidoc.test", "example.com"] {
        assert!(!app.egress_intercepts(&s1, host, 443).await, "{host}");
    }
    let decision = app
        .egress_apply(
            &s1,
            request(
                "api.vidoc.test",
                "/v1?key=yulius-secret-VIDOC_API_KEY",
                &[("Authorization", "Bearer yulius-secret-VIDOC_API_KEY")],
            ),
        )
        .await;
    assert_eq!(
        header(&decision, "Authorization"),
        Some("Bearer yulius-secret-VIDOC_API_KEY"),
        "the placeholder stays: the host is not bound"
    );
    // People may still bind (and unbind) hosts directly.
    let id = created.body["id"].as_str().ok_or("id")?;
    let bound = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"hosts": ["api.vidoc.test"]})),
        &w.writer,
    )
    .await?;
    assert_eq!(bound.status, StatusCode::OK, "{}", bound.body);
    assert!(app.egress_intercepts(&s1, "api.vidoc.test", 443).await);
    let unbound = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"hosts": []})),
        &w.writer,
    )
    .await?;
    assert_eq!(unbound.status, StatusCode::OK, "{}", unbound.body);
    assert!(!app.egress_intercepts(&s1, "api.vidoc.test", 443).await);
    Ok(())
}

async fn agent_secret_of(w: &World, name: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.writer,
    )
    .await?;
    Ok(listed
        .body
        .as_array()
        .and_then(|a| a.iter().find(|s| s["name"] == name))
        .cloned()
        .ok_or(name.to_owned())?)
}

async fn answer_request(
    w: &World,
    creds: &Creds,
    id: &str,
    allow: bool,
    hosts: &[&str],
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{id}/request"),
        Some(json!({"allow": allow, "hosts": hosts})),
        creds,
    )
    .await
}

/// SPEC-121 ASH-003/004/006, AC-ASH-3/4: an agent proposes the hosts of a secret, which binds
/// nothing; a person who manages it allows them (bound) or dismisses them; others cannot.
#[tokio::test]
async fn agents_request_hosts_and_people_allow_them() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let w = world().await?;
    let created = add_agent_secret(
        &w,
        &w.writer,
        "s1",
        json!({"name": "VIDOC_API_KEY", "description": "API key for Vidoc", "value": "vidoc-key-123456"}),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["requested_hosts"], json!([]));
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();

    // Refusals: an unknown secret, another sandbox's agent, a bad host, too many hosts, none.
    let many: Vec<String> = (0..17).map(|i| format!("h{i}.vidoc.test")).collect();
    for (token, args) in [
        (
            TOKEN_S1,
            json!({"secret": "NOPE", "hosts": ["api.vidoc.test"]}),
        ),
        (
            TOKEN_S2,
            json!({"secret": "VIDOC_API_KEY", "hosts": ["api.vidoc.test"]}),
        ),
        (
            TOKEN_S1,
            json!({"secret": "VIDOC_API_KEY", "hosts": ["localhost"]}),
        ),
        (TOKEN_S1, json!({"secret": "VIDOC_API_KEY", "hosts": many})),
        (TOKEN_S1, json!({"secret": "VIDOC_API_KEY", "hosts": []})),
    ] {
        let refused = mcp_tool(&w.h, token, "secret_request_hosts", args.clone()).await?;
        assert_eq!(refused["isError"], true, "{args}: {refused}");
    }
    assert_eq!(
        agent_secret_of(&w, "VIDOC_API_KEY").await?["requested_hosts"],
        json!([])
    );

    let asked = mcp_tool(
        &w.h,
        TOKEN_S1,
        "secret_request_hosts",
        json!({"secret": "VIDOC_API_KEY", "hosts": ["https://API.vidoc.test/"]}),
    )
    .await?;
    assert_eq!(asked["isError"], false, "{asked}");
    let body = tool_text(&asked);
    assert_eq!(body["requested"], json!(["api.vidoc.test"]));
    assert!(
        body["link"]
            .as_str()
            .is_some_and(|l| l.ends_with("/secrets?section=agents")),
        "{body}"
    );
    // Nothing bound: the request waits for a person; the task needs one.
    let secret = agent_secret_of(&w, "VIDOC_API_KEY").await?;
    assert_eq!(secret["hosts"], json!([]));
    assert_eq!(secret["requested_hosts"], json!(["api.vidoc.test"]));
    assert!(
        secret["requested_by"]
            .as_str()
            .is_some_and(|b| b.starts_with('#')),
        "{secret}"
    );
    let app: Arc<dyn EgressPolicy> = w.h.app.clone();
    let s1 = app.egress_client(TOKEN_S1).await.ok_or("s1 client")?;
    assert!(!app.egress_intercepts(&s1, "api.vidoc.test", 443).await);
    let task =
        w.h.store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("task")?;
    assert_eq!(
        task.activity,
        Some(yulius_domain::task::Activity::NeedsAttention)
    );
    let listed = tool_text(&mcp_tool(&w.h, TOKEN_S1, "secrets_list", json!({})).await?);
    assert_eq!(
        listed["secrets"][0]["requested_hosts"],
        json!(["api.vidoc.test"])
    );
    assert!(
        listed["how_to_use"]
            .to_string()
            .contains("secret_request_hosts"),
        "{listed}"
    );

    // Only its managers answer: a reader is refused, someone who cannot see it finds nothing.
    let reader = answer_request(&w, &w.reader, &id, true, &["api.vidoc.test"]).await?;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.body);
    let outsider = answer_request(&w, &w.outsider, &id, true, &["api.vidoc.test"]).await?;
    assert_eq!(outsider.status, StatusCode::NOT_FOUND, "{}", outsider.body);
    let allowed = answer_request(&w, &w.writer, &id, true, &["api.vidoc.test"]).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["hosts"], json!(["api.vidoc.test"]));
    assert_eq!(allowed.body["requested_hosts"], json!([]));
    assert_eq!(allowed.body["requested_by"], Value::Null);
    assert!(app.egress_intercepts(&s1, "api.vidoc.test", 443).await);
    let none = answer_request(&w, &w.writer, &id, true, &["api.vidoc.test"]).await?;
    assert_eq!(none.status, StatusCode::CONFLICT, "nothing to answer");

    // A bound host is reported as bound; a new one dismissed binds nothing.
    let again = tool_text(
        &mcp_tool(
            &w.h,
            TOKEN_S1,
            "secret_request_hosts",
            json!({"secret": "VIDOC_API_KEY", "hosts": ["api.vidoc.test"]}),
        )
        .await?,
    );
    assert_eq!(again["requested"], json!([]));
    assert_eq!(again["already_bound"], json!(["api.vidoc.test"]));
    assert_eq!(
        agent_secret_of(&w, "VIDOC_API_KEY").await?["requested_hosts"],
        json!([])
    );
    mcp_tool(
        &w.h,
        TOKEN_S1,
        "secret_request_hosts",
        json!({"secret": "VIDOC_API_KEY", "hosts": ["evil.test"], "reason": "upload"}),
    )
    .await?;
    let dismissed = answer_request(&w, &w.writer, &id, false, &["evil.test"]).await?;
    assert_eq!(dismissed.status, StatusCode::OK, "{}", dismissed.body);
    assert_eq!(dismissed.body["hosts"], json!(["api.vidoc.test"]));
    assert_eq!(dismissed.body["requested_hosts"], json!([]));
    assert!(!app.egress_intercepts(&s1, "evil.test", 443).await);

    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    for action in [
        "vault.agent.request",
        "vault.agent.allow",
        "vault.agent.dismiss",
    ] {
        assert!(
            activity
                .body
                .as_array()
                .is_some_and(|a| a.iter().any(|e| e["action"] == action)),
            "{action}: {}",
            activity.body
        );
    }
    Ok(())
}

/// SPEC-121 review round 1: an answer covers exactly the hosts the person saw. A host an agent
/// added while the page was open is never allowed unseen; the answer changes only the hosts and
/// the request; limits and instance-wide secrets hold.
#[tokio::test]
async fn an_answer_covers_only_the_hosts_the_person_saw() -> TestResult {
    let w = world().await?;
    let created = add_agent_secret(
        &w,
        &w.writer,
        "s1",
        json!({"name": "VIDOC_API_KEY", "description": "API key for Vidoc", "value": "vidoc-key-123456"}),
    )
    .await?;
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let ask = |hosts: Value| {
        mcp_tool(
            &w.h,
            TOKEN_S1,
            "secret_request_hosts",
            json!({"secret": "VIDOC_API_KEY", "hosts": hosts}),
        )
    };
    ask(json!(["api.vidoc.test"])).await?;
    // The person sees api.vidoc.test; meanwhile the agent adds evil.test.
    ask(json!(["evil.test"])).await?;
    for allow in [true, false] {
        let stale = answer_request(&w, &w.writer, &id, allow, &["api.vidoc.test"]).await?;
        assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    }
    let secret = agent_secret_of(&w, "VIDOC_API_KEY").await?;
    assert_eq!(secret["hosts"], json!([]));
    assert_eq!(
        secret["requested_hosts"],
        json!(["api.vidoc.test", "evil.test"])
    );
    let app: Arc<dyn EgressPolicy> = w.h.app.clone();
    let s1 = app.egress_client(TOKEN_S1).await.ok_or("s1 client")?;
    assert!(!app.egress_intercepts(&s1, "evil.test", 443).await);
    // What the page shows after a reload answers it (any order, any case).
    let allowed =
        answer_request(&w, &w.writer, &id, true, &["EVIL.test", "api.vidoc.test"]).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(
        allowed.body["hosts"],
        json!(["api.vidoc.test", "evil.test"])
    );

    // Bound and requested together stay within 16: the answer is refused, the request kept.
    let many: Vec<String> = (0..16).map(|i| format!("h{i}.vidoc.test")).collect();
    let patched = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"hosts": many[..15]})),
        &w.writer,
    )
    .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    ask(json!(["new.vidoc.test"])).await?;
    let full = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"hosts": many})),
        &w.writer,
    )
    .await?;
    assert_eq!(full.status, StatusCode::OK, "{}", full.body);
    let over = answer_request(&w, &w.writer, &id, true, &["new.vidoc.test"]).await?;
    assert_eq!(
        over.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        over.body
    );
    assert_eq!(
        agent_secret_of(&w, "VIDOC_API_KEY").await?["requested_hosts"],
        json!(["new.vidoc.test"])
    );

    // An instance-wide secret's request names the sandbox and is answered by administrators.
    let global = add_agent_secret(
        &w,
        &w.admin,
        "s1",
        json!({"scope": "instance", "name": "GLOBAL_KEY", "value": "global-value-123"}),
    )
    .await?;
    assert_eq!(global.status, StatusCode::CREATED, "{}", global.body);
    let global_id = global.body["id"].as_str().ok_or("id")?.to_owned();
    mcp_tool(
        &w.h,
        TOKEN_S1,
        "secret_request_hosts",
        json!({"secret": "GLOBAL_KEY", "hosts": ["global.test"]}),
    )
    .await?;
    let listed = agent_secret_of(&w, "GLOBAL_KEY").await?;
    assert!(
        listed["requested_by"]
            .as_str()
            .is_some_and(|b| b.ends_with(" in app")),
        "{listed}"
    );
    let by_writer = answer_request(&w, &w.writer, &global_id, true, &["global.test"]).await?;
    assert_eq!(
        by_writer.status,
        StatusCode::FORBIDDEN,
        "{}",
        by_writer.body
    );
    let by_admin = answer_request(&w, &w.admin, &global_id, true, &["global.test"]).await?;
    assert_eq!(by_admin.status, StatusCode::OK, "{}", by_admin.body);
    Ok(())
}

/// SPEC-178 VAU-002: a task's agent verifies a value of its own project's agent secrets by its
/// SHA-256, never another project's, and never a production secret's.
#[tokio::test]
async fn agents_verify_a_value_of_their_projects_secrets_by_its_hash() -> TestResult {
    let w = world().await?;
    for (sandbox, body) in [
        (
            "s1",
            json!({"name": "STRIPE_KEY", "hosts": ["api.stripe.test"], "value": STRIPE}),
        ),
        (
            "s1",
            json!({"name": "PROD_KEY", "value": "prod-value-12345"}),
        ),
        (
            "s2",
            json!({"name": "OTHER_KEY", "value": "other-value-12345"}),
        ),
    ] {
        let created = add_agent_secret(&w, &w.writer, sandbox, body).await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        if created.body["name"] == "PROD_KEY" {
            use yulius_app::ports::vault::VaultStore as _;
            let id = created.body["id"].as_str().ok_or("id")?;
            assert!(w.h.store.vault_set_cd_prod(id, false, true).await?);
        }
    }
    let sha = |text: &str| w.h.app.deps.crypto.sha256_hex(text.as_bytes());

    let verified = tool_text(
        &mcp_tool(
            &w.h,
            TOKEN_S1,
            "secret_verify",
            json!({"secret": "STRIPE_KEY", "sha256": sha(STRIPE)}),
        )
        .await?,
    );
    assert_eq!(verified["match"], "exact", "{verified}");
    assert_eq!(verified["lines"], 1, "{verified}");
    let wrong = tool_text(
        &mcp_tool(
            &w.h,
            TOKEN_S1,
            "secret_verify",
            json!({"secret": "STRIPE_KEY", "sha256": sha("not-the-key")}),
        )
        .await?,
    );
    assert_eq!(wrong["match"], "different", "{wrong}");
    assert_hidden(&verified, &[STRIPE, &sha(STRIPE)]);

    let other = mcp_tool(
        &w.h,
        TOKEN_S1,
        "secret_verify",
        json!({"secret": "OTHER_KEY", "sha256": sha("other-value-12345")}),
    )
    .await?;
    assert_eq!(other["isError"], true, "{other}");
    assert!(
        tool_text(&other)
            .to_string()
            .contains("no agent secret named"),
        "{other}"
    );
    let production = mcp_tool(
        &w.h,
        TOKEN_S1,
        "secret_verify",
        json!({"secret": "PROD_KEY", "sha256": sha("prod-value-12345")}),
    )
    .await?;
    assert_eq!(production["isError"], true, "{production}");
    assert!(
        tool_text(&production)
            .to_string()
            .contains("production secret"),
        "{production}"
    );

    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    assert!(
        activity.body.as_array().is_some_and(|a| a
            .iter()
            .any(|e| e["action"] == "vault.agent.verify"
                && e["actor"] == "task:1"
                && e["detail"] == "STRIPE_KEY → exact")),
        "{}",
        activity.body
    );
    Ok(())
}

/// SPEC-178 VAU-002 (review round 1): a check repeated after the value was rotated is audited
/// again, so the activity shows the answer that changed.
#[tokio::test]
async fn a_verify_after_a_rotation_is_audited_again() -> TestResult {
    let w = world().await?;
    let created = add_agent_secret(
        &w,
        &w.writer,
        "s1",
        json!({"name": "STRIPE_KEY", "hosts": ["api.stripe.test"], "value": STRIPE}),
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?;
    let args = json!({"secret": "STRIPE_KEY", "sha256": w.h.app.deps.crypto.sha256_hex(STRIPE.as_bytes())});
    let verify = || mcp_tool(&w.h, TOKEN_S1, "secret_verify", args.clone());
    assert_eq!(tool_text(&verify().await?)["match"], "exact");
    let rotated = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"value": "rotated-stripe-value-1"})),
        &w.writer,
    )
    .await?;
    assert!(rotated.status.is_success(), "{}", rotated.body);
    let after = tool_text(&verify().await?);
    assert_eq!(after["match"], "different", "{after}");
    // Lines are told only for a match: a guess learns nothing of the value's shape.
    assert!(after.get("lines").is_none(), "{after}");

    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    let details: Vec<&str> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.agent.verify")
        .filter_map(|e| e["detail"].as_str())
        .collect();
    assert_eq!(
        details,
        ["STRIPE_KEY → different", "STRIPE_KEY → exact"],
        "newest first"
    );
    Ok(())
}
