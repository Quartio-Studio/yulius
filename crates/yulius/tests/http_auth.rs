//! End-to-end HTTP tests: setup, sign-in, CSRF, admission, GitHub-mirrored visibility and
//! access on every sandbox and task route (AC-ACC-1..5).

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, perms, seed_github_user, seed_sandbox, seed_task,
    session_from,
};
use yulius_app::auth::{DEVICE_STARTS_PER_MINUTE, MAX_DEVICE_GRANTS, MAX_DEVICE_GRANTS_PER_CLIENT};
use yulius_app::error::AppError;
use yulius_app::model::SandboxRecord;
use yulius_app::ports::store::{InstanceStore, SandboxStore, SessionStore};
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
        (Method::GET, "/api/sandboxes/{sid}/onboarding/plan", None),
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
    for path in ["/api/sandboxes/s1", "/api/tasks/t1"] {
        let seen = call(&h.router, Method::GET, path, None, &alice).await?;
        assert_eq!(seen.status, StatusCode::OK, "{path}");
    }
    // MSR-005 (SPEC-028): a task has no messages route.
    for (method, body) in [
        (Method::GET, None),
        (Method::POST, Some(json!({"body": "hi"}))),
    ] {
        let gone = call(&h.router, method, "/api/tasks/t1/messages", body, &alice).await?;
        assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.body);
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

/// #169: live device grants are bounded per client and per instance, whatever the per-minute
/// limit lets through; past either bound a start is refused with `Capacity` (429).
#[tokio::test]
async fn device_grants_are_bounded_per_client_and_instance() -> TestResult {
    let h = harness()?;
    let minute = || {
        h.clock
            .0
            .fetch_add(61_000, std::sync::atomic::Ordering::SeqCst)
    };
    for n in 0..MAX_DEVICE_GRANTS_PER_CLIENT {
        if n > 0 && n % DEVICE_STARTS_PER_MINUTE == 0 {
            minute();
        }
        h.app.device_start("one").await?;
    }
    minute();
    let waiting = |refused: Result<_, AppError>| matches!(refused, Err(AppError::Capacity(m)) if m.contains("sign-ins waiting"));
    assert!(
        waiting(h.app.device_start("one").await),
        "the client's bound"
    );
    for n in MAX_DEVICE_GRANTS_PER_CLIENT..MAX_DEVICE_GRANTS {
        h.app.device_start(&format!("client-{n}")).await?;
    }
    assert!(
        waiting(h.app.device_start("newcomer").await),
        "the instance's bound"
    );
    Ok(())
}

/// #169: the background purge deletes consumed grants at once and the rest once they expired;
/// polling a purged grant reads "expired", like an unknown one.
#[tokio::test]
async fn device_grants_disappear_within_one_purge() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    let consumed = h.app.device_start("cli").await?;
    let pending = h.app.device_start("cli").await?;
    let approve = call(
        &h.router,
        Method::POST,
        "/api/device/approve",
        Some(json!({"user_code": consumed.user_code, "approve": true})),
        &admin,
    )
    .await?;
    assert_eq!(approve.status, StatusCode::NO_CONTENT);
    let got = h.app.device_poll(&consumed.device_code).await?;
    assert_eq!(got.status, "approved");

    h.app.purge_device_grants().await?;
    assert!(
        h.store
            .device_grant_by_user_code(&consumed.user_code)
            .await?
            .is_none()
    );
    assert_eq!(
        h.app.device_poll(&pending.device_code).await?.status,
        "pending"
    );

    h.clock.0.fetch_add(
        i64::from(pending.expires_in_s) * 1000,
        std::sync::atomic::Ordering::SeqCst,
    );
    h.app.purge_device_grants().await?;
    assert!(
        h.store
            .device_grant_by_user_code(&pending.user_code)
            .await?
            .is_none()
    );
    assert_eq!(
        h.app.device_poll(&pending.device_code).await?.status,
        "expired"
    );
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

/// #56: every public response carries the baseline security headers (no framing at all, not
/// even by Yulius itself — BRW-008 —, no MIME sniffing, no referrer to other origins) and the
/// server's tag, which the preview proxy uses to refuse showing Yulius inside itself.
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
        assert_eq!(header("x-frame-options").as_deref(), Some("DENY"), "{path}");
        assert_eq!(
            header("content-security-policy").as_deref(),
            Some("frame-ancestors 'none'"),
            "{path}"
        );
        assert_eq!(
            header("x-yulius").as_deref(),
            Some(h.app.instance_tag.as_str()),
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

/// #66: an open event stream follows revoked access and closes for a disabled user within the
/// re-check interval, as new requests would.
#[tokio::test]
async fn event_streams_follow_revoked_access_and_close_for_disabled_users() -> TestResult {
    use futures::StreamExt as _;
    use support::{HarnessOptions, harness_with, serve, ws_connect};
    use tokio_tungstenite::tungstenite::Message;
    use yulius_app::ports::store::UserStore;
    use yulius_contracts::events::UiEvent;
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.viewer_recheck_ms = 200)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "studio", "u-alice").await?;
    h.github.set_perm("alice", "acme/studio", perms("read"));
    let served = serve(h.router.clone()).await?;
    let origin = h.app.config.public_url.clone();
    let mut socket = ws_connect(served.addr, "/api/events", &alice, &origin).await?;
    let sandbox = SandboxId::parse("s1")?;
    let publish = |task: &str| {
        h.app.events.publish(
            Some(sandbox.clone()),
            false,
            UiEvent::TaskRemoved {
                task_id: task.into(),
            },
        );
    };
    // The next text frame within `wait`, if any.
    async fn next_text(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        wait: std::time::Duration,
    ) -> Option<Result<String, Option<u16>>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(deadline, socket.next()).await {
                Err(_) => return None,
                Ok(Some(Ok(Message::Text(text)))) => return Some(Ok(text.to_string())),
                Ok(Some(Ok(Message::Close(frame)))) => {
                    return Some(Err(frame.map(|f| u16::from(f.code))));
                }
                Ok(None | Some(Err(_))) => return Some(Err(None)),
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    // The server subscribes once the upgrade completes: publish until the stream has it.
    let mut seen = None;
    for _ in 0..20 {
        publish("t-visible");
        seen = next_text(&mut socket, std::time::Duration::from_millis(250)).await;
        if seen.is_some() {
            break;
        }
    }
    assert!(
        matches!(&seen, Some(Ok(text)) if text.contains("t-visible")),
        "{seen:?}"
    );
    // Drain duplicates of the first event.
    while let Some(Ok(_)) = next_text(&mut socket, std::time::Duration::from_millis(300)).await {}

    // Access removed on GitHub: after the permission cache expires and a re-check, this
    // sandbox's events no longer reach the open stream.
    if let Ok(mut map) = h.github.perms.lock() {
        map.remove(&("alice".to_owned(), "acme/studio".to_owned()));
    }
    h.clock
        .0
        .fetch_add(6 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    publish("t-hidden");
    let hidden = next_text(&mut socket, std::time::Duration::from_secs(1)).await;
    assert!(
        hidden.is_none(),
        "an event of a revoked sandbox arrived: {hidden:?}"
    );

    // A disabled user's stream closes with 4003.
    h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(Timestamp(5))), None)
        .await?;
    let closed = next_text(&mut socket, std::time::Duration::from_secs(5)).await;
    assert_eq!(closed, Some(Err(Some(4003))), "{closed:?}");
    Ok(())
}

/// SPEC-191 LIV-001: a quiet event stream still says it is alive, every heartbeat interval, so a
/// page can tell a dead connection from a quiet one.
#[tokio::test]
async fn a_quiet_event_stream_sends_heartbeats() -> TestResult {
    use support::{HarnessOptions, harness_with, serve, ws_connect};
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.events_heartbeat_ms = 150)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let served = serve(h.router.clone()).await?;
    let origin = h.app.config.public_url.clone();
    let mut socket = ws_connect(served.addr, "/api/events", &alice, &origin).await?;
    for beat in 0..2 {
        let text = next_ws_text(&mut socket, std::time::Duration::from_secs(2)).await;
        let Some(Ok(text)) = text else {
            return Err(format!("heartbeat {beat} did not arrive: {text:?}").into());
        };
        let event: Value = serde_json::from_str(&text)?;
        assert_eq!(event, json!({"type": "heartbeat"}), "{text}");
    }
    Ok(())
}

/// The next text frame within `wait`: `Ok(text)`, `Err(close code)` when the socket closed, or
/// `None` when nothing arrived.
async fn next_ws_text(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    wait: std::time::Duration,
) -> Option<Result<String, Option<u16>>> {
    use futures::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(Message::Text(text)))) => return Some(Ok(text.to_string())),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Some(Err(frame.map(|f| u16::from(f.code))));
            }
            Ok(None | Some(Err(_))) => return Some(Err(None)),
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// ACC-012 (F 2026-10-09, regression): the password superadmin has no GitHub link, and every
/// access decision used to require one, so it saw no sandbox at all. It now lists and opens every
/// sandbox with admin access — tasks, messages, files, tools, analytics — without asking GitHub
/// (here GitHub is down) and without caching a mirrored permission.
#[tokio::test]
async fn the_superadmin_sees_and_opens_every_sandbox_without_github() -> TestResult {
    use yulius_app::ports::store::{PermissionStore, UserStore};
    let h = harness()?;
    let admin = setup_superadmin(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "studio", "u-alice").await?;
    seed_sandbox(&h, "s2", "product", "u-alice").await?;
    h.github.set_perm("alice", "acme/studio", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s2", "u-alice", TaskStatus::Closed, None).await?;
    let repo = h.app.config.data_dir.join("sandboxes/s2/repo");
    std::fs::create_dir_all(&repo)?;
    std::fs::write(repo.join("README.md"), "# product\n")?;
    h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let me = call(&h.router, Method::GET, "/api/me", None, &admin).await?;
    assert_eq!(me.body["user"]["is_superadmin"], true);
    assert_eq!(me.body["user"]["github_linked"], false);
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &admin).await?;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    let mut seen: Vec<(String, String)> = list
        .body
        .as_array()
        .ok_or("array")?
        .iter()
        .map(|s| {
            (
                s["slug"].as_str().unwrap_or_default().to_owned(),
                s["access"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("product".to_owned(), "admin".to_owned()),
            ("studio".to_owned(), "admin".to_owned())
        ],
        "every sandbox, with admin access"
    );

    for path in [
        "/api/sandboxes/s2",
        "/api/sandboxes/s2/tasks",
        "/api/tasks/t2",
        "/api/sandboxes/s2/files?path=",
        "/api/sandboxes/s2/files/content?path=README.md",
        "/api/sandboxes/s2/workflows",
        "/api/sandboxes/s2/automations",
        "/api/sandboxes/s2/mcp",
        "/api/analytics/summary?sandbox=s2",
        "/api/tasks/t1",
    ] {
        let r = call(&h.router, Method::GET, path, None, &admin).await?;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
    }
    let detail = call(&h.router, Method::GET, "/api/sandboxes/s2", None, &admin).await?;
    assert_eq!(detail.body["access"], "admin");
    let content = call(
        &h.router,
        Method::GET,
        "/api/sandboxes/s2/files/content?path=README.md",
        None,
        &admin,
    )
    .await?;
    assert!(
        content.body.to_string().contains("# product"),
        "{}",
        content.body
    );
    // Changing someone's task is theirs alone, also for the superadmin (SPEC-070): it acts on
    // GitHub as them.
    let archived = call(
        &h.router,
        Method::POST,
        "/api/tasks/t2/archive",
        Some(json!({})),
        &admin,
    )
    .await?;
    assert_eq!(archived.status, StatusCode::FORBIDDEN, "{}", archived.body);
    assert!(
        archived.body.to_string().contains("Ask them for control"),
        "{}",
        archived.body
    );

    // Nothing was mirrored from GitHub for the superadmin.
    let superadmin = h.store.superadmin().await?.ok_or("superadmin")?;
    for sid in ["s1", "s2"] {
        assert!(
            h.store
                .cached_access(&superadmin.id, &SandboxId::parse(sid)?)
                .await?
                .is_none(),
            "no permission cached for {sid}"
        );
    }
    Ok(())
}

/// ACC-012 is the superadmin's alone: an instance administrator or any other account without a
/// GitHub identity still sees nothing (404 like a missing sandbox), and GitHub users see only
/// what GitHub grants them.
#[tokio::test]
async fn only_the_superadmin_sees_sandboxes_without_github() -> TestResult {
    use yulius_app::model::UserRecord;
    use yulius_app::ports::store::UserStore;
    let h = harness()?;
    setup_superadmin(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "studio", "u-alice").await?;
    seed_sandbox(&h, "s2", "product", "u-alice").await?;
    h.github.set_perm("alice", "acme/studio", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    seed_task(&h, "t2", "s2", "u-alice", TaskStatus::Closed, None).await?;
    // A password account that is not the superadmin (even an instance administrator).
    let other = UserRecord {
        id: UserId::parse("u-pw")?,
        github_login: None,
        github_id: None,
        display_name: "Password user".into(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: true,
        password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$synthetic$synthetic".into()),
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&other).await?;
    let token = h.app.issue_api_token(&other, "tests").await?;
    let other = Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    };

    let none = call(&h.router, Method::GET, "/api/sandboxes", None, &other).await?;
    assert_eq!(none.status, StatusCode::OK);
    assert_eq!(none.body, json!([]));
    for (method, path, body) in scoped_routes() {
        let hidden = fill(path, "s1", "t1");
        let missing = fill(path, "s9", "t9");
        let a = call(&h.router, method.clone(), &hidden, body.clone(), &other).await?;
        let b = call(&h.router, method.clone(), &missing, body, &other).await?;
        assert_eq!(
            (a.status, &a.body),
            (b.status, &b.body),
            "{method} {hidden} reveals a sandbox to an account without GitHub"
        );
    }

    // GitHub users keep mirrored permissions exactly.
    let list = call(&h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    let items = list.body.as_array().ok_or("array")?;
    assert_eq!(items.len(), 1, "{}", list.body);
    assert_eq!(
        (&items[0]["slug"], &items[0]["access"]),
        (&json!("studio"), &json!("read"))
    );
    let hidden = call(&h.router, Method::GET, "/api/tasks/t2", None, &alice).await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// ACC-012 with #66: the superadmin's open sockets — the event stream and a task terminal — are
/// not closed or filtered by the periodic visibility re-check, also after the permission cache's
/// lifetime and while GitHub is down; a password account that is not the superadmin cannot open
/// the terminal at all.
#[tokio::test]
async fn the_superadmins_sockets_survive_the_visibility_recheck() -> TestResult {
    use futures::SinkExt as _;
    use support::{HarnessOptions, harness_with, serve, superadmin_bearer, ws_connect};
    use tokio_tungstenite::tungstenite::Message;
    use yulius_contracts::events::UiEvent;
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.viewer_recheck_ms = 150)),
        ..HarnessOptions::default()
    })?;
    setup_superadmin(&h).await?;
    let admin = superadmin_bearer(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "studio", "u-alice").await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let served = serve(h.router.clone()).await?;
    let origin = h.app.config.public_url.clone();

    // Alice has no access to s1: her terminal is refused like a missing task.
    let refused = ws_connect(served.addr, "/api/tasks/t1/terminal", &alice, &origin).await;
    assert_eq!(
        refused.as_ref().err().and_then(support::ws_refusal_status),
        Some(404)
    );

    let mut terminal = ws_connect(served.addr, "/api/tasks/t1/terminal", &admin, &origin).await?;
    let state = next_ws_text(&mut terminal, std::time::Duration::from_secs(5))
        .await
        .ok_or("no state frame")?
        .map_err(|code| format!("closed: {code:?}"))?;
    let state: Value = serde_json::from_str(&state)?;
    assert_eq!(state["type"], "state");
    // Alice owns the task: the superadmin watches it (SPEC-070).
    assert_eq!(state["can_write"], false, "only the owner types");
    let mut events = ws_connect(served.addr, "/api/events", &admin, &origin).await?;
    let sandbox = SandboxId::parse("s1")?;
    let publish = |task: &str| {
        h.app.events.publish(
            Some(sandbox.clone()),
            false,
            UiEvent::TaskRemoved {
                task_id: task.into(),
            },
        );
    };
    // The server subscribes once the upgrade completes: publish until the stream has it.
    let mut seen = None;
    for _ in 0..20 {
        publish("t-first");
        seen = next_ws_text(&mut events, std::time::Duration::from_millis(250)).await;
        if seen.is_some() {
            break;
        }
    }
    assert!(
        matches!(&seen, Some(Ok(text)) if text.contains("t-first")),
        "{seen:?}"
    );
    while let Some(Ok(_)) = next_ws_text(&mut events, std::time::Duration::from_millis(300)).await {
    }

    // Past the permission cache and with GitHub down, several re-checks run.
    h.github
        .fail_permissions
        .store(true, std::sync::atomic::Ordering::SeqCst);
    h.clock
        .0
        .fetch_add(40 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let quiet = next_ws_text(&mut terminal, std::time::Duration::from_millis(300)).await;
    assert!(quiet.is_none(), "the terminal changed: {quiet:?}");
    terminal
        .send(Message::Text("not a frame".to_owned().into()))
        .await?;
    let answer = next_ws_text(&mut terminal, std::time::Duration::from_secs(5)).await;
    assert!(
        matches!(&answer, Some(Ok(text)) if text.contains("unknown frame")),
        "the terminal is still open: {answer:?}"
    );
    publish("t-later");
    let later = next_ws_text(&mut events, std::time::Duration::from_secs(5)).await;
    assert!(
        matches!(&later, Some(Ok(text)) if text.contains("t-later")),
        "the event stream still carries the sandbox: {later:?}"
    );
    Ok(())
}
