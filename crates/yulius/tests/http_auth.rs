//! End-to-end HTTP tests: setup, sign-in, CSRF, admission and GitHub-mirrored visibility
//! (AC-ACC-1..5).

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{Creds, TestResult, call, harness, session_from};
use yulius_app::model::SandboxRecord;
use yulius_app::ports::store::{InstanceStore, SandboxStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::time::Timestamp;

async fn setup_superadmin(h: &support::Harness) -> Result<Creds, Box<dyn std::error::Error>> {
    let token = h.app.prepare_setup().await?.ok_or("setup token")?;
    let r = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": token.expose(), "password": "a very long secret", "password_confirm": "a very long secret", "confirm_saved": true
    })), &Creds::default()).await?;
    assert_eq!(r.status, StatusCode::CREATED);
    Ok(Creds {
        cookie: session_from(&r.set_cookie),
        csrf: r.body["csrf_token"].as_str().map(str::to_owned),
        bearer: None,
    })
}

#[tokio::test]
async fn setup_requires_token_confirmation_and_happens_once() -> TestResult {
    let h = harness()?;
    let token = h.app.prepare_setup().await?.ok_or("token")?;
    let status = call(
        &h.router,
        Method::GET,
        "/api/setup/status",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(status.body["setup_required"], true);

    let wrong = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": "nope", "password": "a very long secret", "password_confirm": "a very long secret", "confirm_saved": true
    })), &Creds::default()).await?;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);

    let unconfirmed = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": token.expose(), "password": "a very long secret", "password_confirm": "a very long secret", "confirm_saved": false
    })), &Creds::default()).await?;
    assert_eq!(unconfirmed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        unconfirmed.body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("cannot be recovered")
    );

    let short = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": token.expose(), "password": "short", "password_confirm": "short", "confirm_saved": true
    })), &Creds::default()).await?;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);

    let ok = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": token.expose(), "password": "a very long secret", "password_confirm": "a very long secret", "confirm_saved": true
    })), &Creds::default()).await?;
    assert_eq!(ok.status, StatusCode::CREATED);
    let cookie = ok.set_cookie.clone().ok_or("cookie")?;
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("SameSite=Lax")
    );
    assert_eq!(ok.body["user"]["is_superadmin"], true);

    let again = call(&h.router, Method::POST, "/api/setup", Some(json!({
        "token": token.expose(), "password": "a very long secret", "password_confirm": "a very long secret", "confirm_saved": true
    })), &Creds::default()).await?;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert!(
        h.app.prepare_setup().await?.is_none(),
        "no new token once set up"
    );
    // No recovery endpoint exists.
    let recover = call(
        &h.router,
        Method::POST,
        "/api/auth/recover",
        Some(json!({})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(recover.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn password_login_rate_limit_and_csrf() -> TestResult {
    let h = harness()?;
    let creds = setup_superadmin(&h).await?;
    let me = call(&h.router, Method::GET, "/api/me", None, &creds).await?;
    assert_eq!(me.status, StatusCode::OK);

    // State-changing call without CSRF is refused even with a valid cookie.
    let no_csrf = Creds {
        csrf: None,
        ..creds.clone()
    };
    let refused = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(json!({"github_login": "alice"})),
        &no_csrf,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    for _ in 0..5 {
        let bad = call(
            &h.router,
            Method::POST,
            "/api/auth/password",
            Some(json!({"password": "wrong"})),
            &Creds::default(),
        )
        .await?;
        assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    }
    let limited = call(
        &h.router,
        Method::POST,
        "/api/auth/password",
        Some(json!({"password": "a very long secret"})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    h.clock
        .0
        .fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    let ok = call(
        &h.router,
        Method::POST,
        "/api/auth/password",
        Some(json!({"password": "a very long secret"})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(ok.status, StatusCode::OK);

    let logout = call(&h.router, Method::POST, "/api/auth/logout", None, &creds).await?;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    let after = call(&h.router, Method::GET, "/api/me", None, &creds).await?;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

async fn configure_github(h: &support::Harness) -> TestResult {
    let app_cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c", "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    let sealed = h.app.deps.crypto.seal(app_cfg.to_string().as_bytes())?;
    h.store
        .set_github_app(&sealed, "Quartio-Studio", "org")
        .await?;
    Ok(())
}

async fn github_login(
    h: &support::Harness,
    code: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    let start = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/s/studio",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(start.status, StatusCode::SEE_OTHER);
    let location = start.location.ok_or("location")?;
    let state = location.split("state=").nth(1).ok_or("state")?.to_owned();
    call(
        &h.router,
        Method::GET,
        &format!("/api/auth/github/callback?code={code}&state={state}"),
        None,
        &Creds::default(),
    )
    .await
}

#[tokio::test]
async fn only_admitted_github_users_sign_in_and_see_permitted_sandboxes() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    configure_github(&h).await?;
    h.github.add_user("code-alice", 11, "alice");
    h.github.add_user("code-mallory", 12, "mallory");

    let admit = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(json!({"github_login": "alice"})),
        &admin,
    )
    .await?;
    assert_eq!(admit.status, StatusCode::CREATED);

    let denied = github_login(&h, "code-mallory").await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "not admitted");

    let ok = github_login(&h, "code-alice").await?;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location.as_deref(), Some("/s/studio"));
    let alice = Creds {
        cookie: session_from(&ok.set_cookie),
        csrf: None,
        bearer: None,
    };
    let me = call(&h.router, Method::GET, "/api/me", None, &alice).await?;
    assert_eq!(me.body["user"]["login"], "alice");
    assert_eq!(me.body["user"]["github_linked"], true);

    // Two sandboxes; alice can read one and sees nothing of the other.
    for (id, name, port) in [("s1", "studio", 20000u16), ("s2", "product", 20020)] {
        h.store
            .insert_sandbox(&SandboxRecord {
                id: SandboxId::parse(id)?,
                slug: name.into(),
                repo_owner: "Quartio-Studio".into(),
                repo_name: name.into(),
                github_repo_id: None,
                default_branch: "main".into(),
                profile_json: "{}".into(),
                port_base: port,
                port_count: 20,
                onboarding_state: "not_needed".into(),
                onboarding_task_id: None,
                created_by: UserId::parse("u")?,
                created_at: Timestamp(1),
                removed_at: None,
            })
            .await?;
    }
    h.github.set_perm(
        "alice",
        "Quartio-Studio/studio",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    );
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    let items = list.body.as_array().ok_or("array")?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["slug"], "studio");
    assert_eq!(items[0]["access"], "read");
    let hidden = call(&h.router, Method::GET, "/api/sandboxes/s2", None, &alice).await?;
    assert_eq!(
        hidden.status,
        StatusCode::NOT_FOUND,
        "invisible sandbox is not found"
    );

    // Upgrade on GitHub: visible after the cache TTL (AC-ACC-5).
    h.github.set_perm(
        "alice",
        "Quartio-Studio/studio",
        GitHubPermissions {
            push: true,
            pull: true,
            ..Default::default()
        },
    );
    let cached = call(&h.router, Method::GET, "/api/sandboxes/s1", None, &alice).await?;
    assert_eq!(
        cached.body["access"], "read",
        "within TTL the cache answers"
    );
    h.clock
        .0
        .fetch_add(5 * 60 * 1000 + 1, std::sync::atomic::Ordering::SeqCst);
    let fresh = call(&h.router, Method::GET, "/api/sandboxes/s1", None, &alice).await?;
    assert_eq!(fresh.body["access"], "write");

    // GitHub outage: stale-but-recent cache is used; beyond the stale bound the sandbox hides.
    h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    h.clock
        .0
        .fetch_add(5 * 60 * 1000 + 1, std::sync::atomic::Ordering::SeqCst);
    let stale = call(&h.router, Method::GET, "/api/sandboxes/s1", None, &alice).await?;
    assert_eq!(stale.body["access"], "write");
    h.clock
        .0
        .fetch_add(31 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    let gone = call(&h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    assert_eq!(gone.body.as_array().map(Vec::len), Some(0));

    // Disabling ends alice's sessions (AC-ACC-3).
    let users = call(&h.router, Method::GET, "/api/admin/users", None, &admin).await?;
    let alice_id = users.body[0]["id"].as_str().ok_or("id")?.to_owned();
    let disable = call(
        &h.router,
        Method::PATCH,
        &format!("/api/admin/users/{alice_id}"),
        Some(json!({"disabled": true})),
        &admin,
    )
    .await?;
    assert_eq!(disable.status, StatusCode::OK);
    let after = call(&h.router, Method::GET, "/api/me", None, &alice).await?;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn device_approval_issues_a_single_use_cli_token() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    let start = call(
        &h.router,
        Method::POST,
        "/api/device/start",
        None,
        &Creds::default(),
    )
    .await?;
    let device = start.body["device_code"]
        .as_str()
        .ok_or("device")?
        .to_owned();
    let user_code = start.body["user_code"]
        .as_str()
        .ok_or("user code")?
        .to_owned();
    let pending = call(
        &h.router,
        Method::POST,
        "/api/device/poll",
        Some(json!({"device_code": device})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(pending.body["status"], "pending");
    let approve = call(
        &h.router,
        Method::POST,
        "/api/device/approve",
        Some(json!({"user_code": user_code, "approve": true})),
        &admin,
    )
    .await?;
    assert_eq!(approve.status, StatusCode::NO_CONTENT);
    let got = call(
        &h.router,
        Method::POST,
        "/api/device/poll",
        Some(json!({"device_code": device})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(got.body["status"], "approved");
    let token = got.body["token"].as_str().ok_or("token")?.to_owned();
    assert!(token.starts_with("yul_"));
    let again = call(
        &h.router,
        Method::POST,
        "/api/device/poll",
        Some(json!({"device_code": device})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(
        again.body["token"],
        serde_json::Value::Null,
        "token is returned once"
    );
    let me = call(
        &h.router,
        Method::GET,
        "/api/me",
        None,
        &Creds {
            bearer: Some(token),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(me.status, StatusCode::OK);
    Ok(())
}

/// OPS-002 / AC-OPS-2: another frontend (for example a desktop app) uses the API with a bearer
/// token from an allowlisted origin; other origins get neither CORS headers nor WebSockets.
#[tokio::test]
async fn allowlisted_origins_get_cors_and_websockets() -> TestResult {
    use support::{
        HarnessOptions, harness_with, seed_github_user, serve, ws_connect, ws_refusal_status,
    };
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| {
            c.allowed_origins = vec!["app://yulius-desktop".into()]
        })),
        ..HarnessOptions::default()
    })?;
    let creds = seed_github_user(&h, "u-desk", "desk").await?;
    let preflight = |origin: &'static str| {
        axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/api/me")
            .header("origin", origin)
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "authorization")
            .body(axum::body::Body::empty())
    };
    use tower::ServiceExt;
    let allowed = h
        .router
        .clone()
        .oneshot(preflight("app://yulius-desktop")?)
        .await?;
    assert_eq!(
        allowed
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("app://yulius-desktop")
    );
    let other = h
        .router
        .clone()
        .oneshot(preflight("https://evil.test")?)
        .await?;
    assert!(other.headers().get("access-control-allow-origin").is_none());

    let served = serve(h.router.clone()).await?;
    let socket = ws_connect(served.addr, "/api/events", &creds, "app://yulius-desktop").await;
    assert!(
        socket.is_ok(),
        "allowlisted origin may open the event stream"
    );
    let refused = ws_connect(served.addr, "/api/events", &creds, "https://evil.test").await;
    let status = refused.err().as_ref().and_then(ws_refusal_status);
    assert_eq!(status, Some(403));
    Ok(())
}
