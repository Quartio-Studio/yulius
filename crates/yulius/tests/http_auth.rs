//! End-to-end HTTP tests: setup, sign-in, CSRF, admission, GitHub-mirrored visibility and
//! access on every sandbox and task route (AC-ACC-1..5).

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, perms, seed_github_user, seed_sandbox, seed_task,
    session_from,
};
use yulius_app::model::SandboxRecord;
use yulius_app::ports::store::{InstanceStore, SandboxStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::TaskStatus;
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
            .contains("stored the password safely")
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
    // Recovery exists only with the code issued at setup (ACC-011; tests/recovery.rs).
    let recover = call(
        &h.router,
        Method::POST,
        "/api/auth/recover",
        Some(json!({})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(
        recover.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a code is required"
    );
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

/// Every sandbox- and task-scoped route with a body its extractor accepts. `{sid}`/`{tid}` are
/// replaced; websocket routes are called without an upgrade.
fn scoped_routes() -> Vec<(Method, &'static str, Option<Value>)> {
    let empty = || Some(json!({}));
    vec![
        (Method::GET, "/api/sandboxes/{sid}", None),
        (Method::PATCH, "/api/sandboxes/{sid}", empty()),
        (Method::DELETE, "/api/sandboxes/{sid}", None),
        (Method::GET, "/api/sandboxes/{sid}/tasks", None),
        (
            Method::POST,
            "/api/sandboxes/{sid}/tasks",
            Some(json!({"prompt": "x"})),
        ),
        (Method::GET, "/api/sandboxes/{sid}/workflows", None),
        (Method::GET, "/api/sandboxes/{sid}/automations", None),
        (Method::GET, "/api/sandboxes/{sid}/skills", None),
        (Method::GET, "/api/sandboxes/{sid}/skills/x", None),
        (Method::GET, "/api/sandboxes/{sid}/mcp", None),
        (Method::GET, "/api/sandboxes/{sid}/git", None),
        (Method::GET, "/api/sandboxes/{sid}/delivery/runs", None),
        (Method::GET, "/api/sandboxes/{sid}/delivery/runs/r1", None),
        (
            Method::POST,
            "/api/sandboxes/{sid}/delivery/promote",
            empty(),
        ),
        (
            Method::POST,
            "/api/sandboxes/{sid}/delivery/environments/production/runs",
            empty(),
        ),
        (
            Method::POST,
            "/api/sandboxes/{sid}/onboarding/skip",
            empty(),
        ),
        (Method::GET, "/api/sandboxes/{sid}/files?path=", None),
        (
            Method::GET,
            "/api/sandboxes/{sid}/files/content?path=README.md",
            None,
        ),
        (Method::POST, "/api/sandboxes/{sid}/shells", empty()),
        (Method::GET, "/api/sandboxes/{sid}/browser", None),
        (Method::GET, "/api/tasks/{tid}", None),
        (Method::DELETE, "/api/tasks/{tid}", None),
        (Method::GET, "/api/tasks/{tid}/messages", None),
        (
            Method::POST,
            "/api/tasks/{tid}/messages",
            Some(json!({"body": "hi"})),
        ),
        (Method::POST, "/api/tasks/{tid}/stop", empty()),
        (Method::POST, "/api/tasks/{tid}/continue", empty()),
        (Method::POST, "/api/tasks/{tid}/switch", empty()),
        (Method::POST, "/api/tasks/{tid}/archive", empty()),
        (Method::POST, "/api/tasks/{tid}/restore", empty()),
        (
            Method::POST,
            "/api/tasks/{tid}/feedback",
            Some(json!({"verdict": "correct"})),
        ),
        (Method::GET, "/api/tasks/{tid}/delivery", None),
        (Method::GET, "/api/tasks/{tid}/changes", None),
        (Method::GET, "/api/tasks/{tid}/commits", None),
        (Method::POST, "/api/tasks/{tid}/delivery/pr", empty()),
        (Method::POST, "/api/tasks/{tid}/delivery/merge", empty()),
        (Method::GET, "/api/tasks/{tid}/terminal", None),
    ]
}

fn fill(path: &str, sandbox: &str, task: &str) -> String {
    path.replace("{sid}", sandbox).replace("{tid}", task)
}

/// AC-ACC-4 (#52): a sandbox the user cannot see answers exactly like one that does not exist
/// on every sandbox and task route, and a read user is refused every change to a task.
#[tokio::test]
async fn invisible_sandboxes_answer_like_missing_ones_and_read_users_cannot_change_tasks()
-> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "studio", "u-alice").await?;
    seed_sandbox(&h, "s2", "product", "u-alice").await?;
    h.github.set_perm("alice", "acme/studio", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s2", "u-alice", TaskStatus::Closed, None).await?;

    // Alice is signed in and reads her sandbox and its task.
    for path in [
        "/api/sandboxes/s1",
        "/api/tasks/t1",
        "/api/tasks/t1/messages",
    ] {
        let seen = call(&h.router, Method::GET, path, None, &alice).await?;
        assert_eq!(seen.status, StatusCode::OK, "{path}");
    }

    // The sandbox she cannot see (and its task) answers exactly like a missing one.
    for (method, path, body) in scoped_routes() {
        let hidden = fill(path, "s2", "t2");
        let missing = fill(path, "s9", "t9");
        let a = call(&h.router, method.clone(), &hidden, body.clone(), &alice).await?;
        let b = call(&h.router, method.clone(), &missing, body, &alice).await?;
        assert_eq!(
            (a.status, &a.body),
            (b.status, &b.body),
            "{method} {hidden} reveals the invisible sandbox"
        );
        assert!(
            a.status.is_client_error(),
            "{method} {hidden}: {}",
            a.status
        );
        if !path.ends_with("/terminal") && !path.ends_with("/browser") {
            assert_eq!(a.status, StatusCode::NOT_FOUND, "{method} {hidden}");
        }
    }

    // A read user cannot change a task of a sandbox she can read.
    for (method, path, body) in [
        (Method::POST, "/api/tasks/t1/continue", Some(json!({}))),
        (Method::POST, "/api/tasks/t1/switch", Some(json!({}))),
        (Method::POST, "/api/tasks/t1/stop", Some(json!({}))),
        (Method::POST, "/api/tasks/t1/archive", Some(json!({}))),
        (
            Method::POST,
            "/api/tasks/t1/messages",
            Some(json!({"body": "steer"})),
        ),
        (Method::DELETE, "/api/tasks/t1", None),
        (Method::POST, "/api/tasks/t1/delivery/pr", Some(json!({}))),
        (
            Method::POST,
            "/api/sandboxes/s1/tasks",
            Some(json!({"prompt": "x"})),
        ),
    ] {
        let refused = call(&h.router, method.clone(), path, body, &alice).await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{method} {path}");
    }
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

/// Starts GitHub sign-in (`start_query` after `/api/auth/github/start?`) and returns the state
/// GitHub would echo back, checking the redirect to GitHub.
async fn github_state(
    h: &support::Harness,
    start_query: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let start = call(
        &h.router,
        Method::GET,
        &format!("/api/auth/github/start?{start_query}"),
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(start.status, StatusCode::SEE_OTHER);
    let location = start.location.ok_or("location")?;
    let state = location.split("state=").nth(1).ok_or("state")?;
    Ok(state.split('&').next().unwrap_or_default().to_owned())
}

async fn github_callback(
    h: &support::Harness,
    query: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::GET,
        &format!("/api/auth/github/callback?{query}"),
        None,
        &Creds::default(),
    )
    .await
}

async fn github_login(
    h: &support::Harness,
    code: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    let state = github_state(h, "next=/s/studio").await?;
    github_callback(h, &format!("code={code}&state={state}")).await
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

    // A refusal returns to the sign-in page naming the refused account (ACC-004), not an error.
    let denied = github_login(&h, "code-mallory").await?;
    assert_eq!(denied.status, StatusCode::SEE_OTHER, "not admitted");
    assert_eq!(
        denied.location.as_deref(),
        Some("/signin?error=not_admitted&next=/s/studio&login=mallory")
    );
    assert!(denied.set_cookie.is_none(), "no session for a refused user");

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
    let again = github_login(&h, "code-alice").await?;
    assert_eq!(
        again.location.as_deref(),
        Some("/signin?error=disabled&next=/s/studio"),
        "a disabled user is told so on the sign-in page"
    );
    assert!(again.set_cookie.is_none());
    Ok(())
}

/// The same-window case (F 2026-10-09): the browser's GitHub account is refused, GitHub's
/// account picker is offered, and a reload or a denial lands on the sign-in page, never on a raw
/// error (ACC-004, AC-ACC-3).
#[tokio::test]
async fn refused_github_sign_ins_return_to_the_sign_in_page_with_their_reason() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    configure_github(&h).await?;
    h.github.add_user("code-other", 21, "other-account");
    h.github.add_user("code-admitted", 22, "Admitted-Account");
    let admit = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(json!({"github_login": "admitted-account"})),
        &admin,
    )
    .await?;
    assert_eq!(admit.status, StatusCode::CREATED);

    // GitHub signs in the browser's current account, which was not admitted.
    let state = github_state(&h, "next=/s/a%20b").await?;
    let refused = github_callback(&h, &format!("code=code-other&state={state}")).await?;
    assert_eq!(refused.status, StatusCode::SEE_OTHER);
    assert_eq!(
        refused.location.as_deref(),
        Some("/signin?error=not_admitted&next=/s/a%20b&login=other-account")
    );
    // Reloading the used callback URL: the sign-in page again, not "sign-in expired" as JSON.
    let reload = github_callback(&h, &format!("code=code-other&state={state}")).await?;
    assert_eq!(reload.status, StatusCode::SEE_OTHER);
    assert_eq!(reload.location.as_deref(), Some("/signin?error=expired"));

    // "Use another GitHub account" asks GitHub for its account picker; the default does not.
    let plain = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/",
        None,
        &Creds::default(),
    )
    .await?;
    assert!(
        !plain
            .location
            .as_deref()
            .unwrap_or_default()
            .contains("prompt=")
    );
    let state = github_state(&h, "next=/&prompt=select_account").await?;
    let picker = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/&prompt=select_account",
        None,
        &Creds::default(),
    )
    .await?;
    assert!(
        picker
            .location
            .as_deref()
            .unwrap_or_default()
            .ends_with("&prompt=select_account")
    );
    // In the picker the person chooses the admitted account (logins match case-insensitively).
    let ok = github_callback(&h, &format!("code=code-admitted&state={state}")).await?;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location.as_deref(), Some("/"));
    let creds = Creds {
        cookie: session_from(&ok.set_cookie),
        csrf: None,
        bearer: None,
    };
    let me = call(&h.router, Method::GET, "/api/me", None, &creds).await?;
    assert_eq!(me.body["user"]["login"], "Admitted-Account");

    // Denying access on GitHub forgets the state and keeps the destination.
    let state = github_state(&h, "next=/tasks").await?;
    let denied = github_callback(
        &h,
        &format!("error=access_denied&error_description=The+user+has+denied&state={state}"),
    )
    .await?;
    assert_eq!(
        denied.location.as_deref(),
        Some("/signin?error=denied&next=/tasks")
    );
    let replay = github_callback(&h, &format!("code=code-admitted&state={state}")).await?;
    assert_eq!(replay.location.as_deref(), Some("/signin?error=expired"));
    assert!(
        replay.set_cookie.is_none(),
        "a denied state cannot be replayed"
    );

    // A malformed callback (no code, no error) and a GitHub failure (bad code) say "failed".
    let bare = github_callback(&h, "state=whatever").await?;
    assert_eq!(bare.location.as_deref(), Some("/signin?error=failed"));
    let state = github_state(&h, "next=/").await?;
    let failed = github_callback(&h, &format!("code=code-unknown&state={state}")).await?;
    assert_eq!(failed.location.as_deref(), Some("/signin?error=failed"));
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

/// #56: every public response carries the baseline security headers (no framing by other
/// sites, no MIME sniffing, no referrer to other origins).
#[tokio::test]
async fn public_responses_carry_security_headers() -> TestResult {
    use tower::ServiceExt;
    let h = harness()?;
    for path in [
        "/",
        "/s/studio/tasks",
        "/api/me",
        "/api/setup/status",
        "/healthz",
    ] {
        let request = axum::http::Request::builder()
            .uri(path)
            .body(axum::body::Body::empty())?;
        let response = h.router.clone().oneshot(request).await?;
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        assert_eq!(
            header("x-frame-options").as_deref(),
            Some("SAMEORIGIN"),
            "{path}"
        );
        assert_eq!(
            header("content-security-policy").as_deref(),
            Some("frame-ancestors 'self'"),
            "{path}"
        );
        assert_eq!(
            header("x-content-type-options").as_deref(),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            header("referrer-policy").as_deref(),
            Some("same-origin"),
            "{path}"
        );
    }
    Ok(())
}

/// #60: after GitHub sign-in the server returns only to a path on this site.
#[tokio::test]
async fn github_sign_in_returns_only_to_local_paths() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    configure_github(&h).await?;
    h.github.add_user("code-alice", 11, "alice");
    let admit = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(json!({"github_login": "alice"})),
        &admin,
    )
    .await?;
    assert_eq!(admit.status, StatusCode::CREATED);
    for (next, expected) in [
        ("/s/studio/tasks", "/s/studio/tasks"),
        ("/s/a%20b", "/s/a b"),
        ("%2F%5Cexample.org", "/"),
        ("%2F%09%2Fexample.org", "/"),
        ("%2F%0Aexample", "/"),
        ("%2F%2Fexample.org", "/"),
        ("https%3A%2F%2Fexample.org", "/"),
    ] {
        let state = github_state(&h, &format!("next={next}")).await?;
        let done = github_callback(&h, &format!("code=code-alice&state={state}")).await?;
        assert_eq!(done.status, StatusCode::SEE_OTHER, "{next}");
        assert_eq!(done.location.as_deref(), Some(expected), "{next}");
    }
    Ok(())
}
