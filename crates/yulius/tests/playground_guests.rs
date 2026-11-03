//! A playground's guests (SPEC-410, ADR-0260): a playground's admin creates an expiring link, a
//! teammate without GitHub joins with a name and works in that playground only, and revoking the
//! link or its expiry ends it. The fake GitHub, a real store and the harness clock.

use crate::support;

use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, Resp, TestResult, call, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task, session_from,
};
use yulius_app::model::{SandboxRecord, UserRecord, WebSessionRecord};
use yulius_app::ports::egress::{EgressDecision, EgressPolicy as _, EgressRequest};
use yulius_app::ports::store::{
    AuditStore as _, SandboxStore as _, SessionStore as _, UserStore as _,
};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::{Action, TokenScope};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const FENCE: &str = "a guest reaches only the playground they were let into";
const ALICE_TASK: &str = "guests-task-token-alice-0123456789abcdef";
const GUEST_TASK: &str = "guests-task-token-guest-0123456789abcdef";

struct World {
    h: Harness,
    /// Made the playground `p1` (`alices-lab`); admin on the GitHub sandbox `s1` (`app`).
    alice: Creds,
    /// Made the playground `p2` (`bobs-lab`).
    bob: Creds,
    owner: Creds,
}

/// A playground as the server records one (SPEC-099): no GitHub owner, its name as the repository.
async fn seed_playground(h: &Harness, id: &str, slug: &str, by: &str) -> BoxResult<SandboxRecord> {
    let sandbox = SandboxRecord {
        id: SandboxId::parse(id)?,
        slug: slug.into(),
        repo_owner: String::new(),
        repo_name: slug.into(),
        github_repo_id: None,
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: h.store.next_port_base(43000, 20).await?,
        port_count: 20,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: UserId::parse(by)?,
        created_at: Timestamp(1),
        removed_at: None,
    };
    h.store.insert_sandbox(&sandbox).await?;
    Ok(sandbox)
}

/// A browser session for a seeded user, as the web UI has: a person lets guests in.
async fn browser(h: &Harness, id: &str) -> BoxResult<Creds> {
    let cookie = format!("browser-{id}");
    let csrf = format!("csrf-{id}");
    let now = now(h);
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: UserId::parse(id)?,
            csrf_token: csrf.clone(),
            created_at: now,
            expires_at: now.plus_millis(60 * DAY_MS),
            revoked_at: None,
        })
        .await?;
    Ok(Creds {
        cookie: Some(cookie),
        csrf: Some(csrf),
        bearer: None,
    })
}

async fn world() -> BoxResult<World> {
    // Open sockets re-check who is signed in often, so a revocation shows within the test.
    let h = support::harness_with(support::HarnessOptions {
        configure: Some(Box::new(|c| c.viewer_recheck_ms = 200)),
        ..support::HarnessOptions::default()
    })?;
    let owner = support::setup_superadmin_creds(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_github_user(&h, "u-bob", "bob").await?;
    let alice = browser(&h, "u-alice").await?;
    let bob = browser(&h, "u-bob").await?;
    seed_playground(&h, "p1", "alices-lab", "u-alice").await?;
    seed_playground(&h, "p2", "bobs-lab", "u-bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("admin"));
    Ok(World {
        h,
        alice,
        bob,
        owner,
    })
}

async fn invite(w: &World, creds: &Creds, sid: &str, body: Value) -> BoxResult<Resp> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/guest-invites"),
        Some(body),
        creds,
    )
    .await
}

/// A new link to `p1` from alice: its code.
async fn link(w: &World, days: u32) -> BoxResult<String> {
    let made = invite(w, &w.alice, "p1", json!({ "days": days })).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    code_of(&made.body)
}

fn code_of(created: &Value) -> BoxResult<String> {
    let url = created["url"].as_str().ok_or("url")?;
    Ok(url.split_once("?code=").ok_or("code")?.1.to_owned())
}

async fn peek(w: &World, code: &str) -> BoxResult<Resp> {
    call(
        &w.h.router,
        Method::GET,
        &format!("/api/guest-invites/{code}"),
        None,
        &Creds::default(),
    )
    .await
}

async fn join_raw(w: &World, code: &str, name: &str) -> BoxResult<Resp> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/guest-invites/{code}/join"),
        Some(json!({ "name": name })),
        &Creds::default(),
    )
    .await
}

/// Joins through `code` as `name`: the guest's browser credentials and account.
async fn join(w: &World, code: &str, name: &str) -> BoxResult<(Creds, UserRecord)> {
    let joined = join_raw(w, code, name).await?;
    assert_eq!(joined.status, StatusCode::CREATED, "{}", joined.body);
    let creds = Creds {
        cookie: Some(session_from(&joined.set_cookie).ok_or("no session cookie")?),
        csrf: Some(joined.body["csrf_token"].as_str().ok_or("csrf")?.to_owned()),
        bearer: None,
    };
    let id = UserId::parse(joined.body["user"]["id"].as_str().ok_or("id")?)?;
    let user = w.h.store.user(&id).await?.ok_or("guest account")?;
    Ok((creds, user))
}

fn now(h: &Harness) -> Timestamp {
    Timestamp(h.clock.0.load(Ordering::SeqCst))
}

fn advance(h: &Harness, ms: i64) {
    h.clock.0.fetch_add(ms, Ordering::SeqCst);
}

async fn get(w: &World, path: &str, creds: &Creds) -> BoxResult<Resp> {
    call(&w.h.router, Method::GET, path, None, creds).await
}

fn slugs(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|s| s["slug"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_maker_creates_an_expiring_link_shown_once() -> TestResult {
    let w = world().await?;
    let now = now(&w.h);
    let made = invite(&w, &w.alice, "p1", json!({"days": 1})).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let code = code_of(&made.body)?;
    let url = made.body["url"].as_str().ok_or("url")?;
    assert!(
        url.starts_with(&format!("{}/join?code=", w.h.app.config.public_url)),
        "{url}"
    );
    assert_eq!(code.len(), 64, "a 256-bit code");
    assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(made.body["invite"]["created_by"], "alice");
    assert_eq!(
        made.body["invite"]["expires_at"],
        yulius_app::auth::rfc3339(now.plus_millis(DAY_MS))
    );

    // The list never shows the code again.
    let listed = get(&w, "/api/sandboxes/p1/guest-invites", &w.alice).await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    assert_eq!(listed.body["invites"].as_array().map(Vec::len), Some(1));
    assert!(!listed.body.to_string().contains(&code), "{}", listed.body);
    assert_eq!(listed.body["max_live"], 5);
    assert_eq!(listed.body["max_guests"], 20);

    // 7 days by default; only 1, 7 or 30.
    let default = invite(&w, &w.alice, "p1", json!({})).await?;
    assert_eq!(default.status, StatusCode::CREATED);
    assert_eq!(
        default.body["invite"]["expires_at"],
        yulius_app::auth::rfc3339(now.plus_millis(7 * DAY_MS))
    );
    for days in [0, 2, 31, 365] {
        let refused = invite(&w, &w.alice, "p1", json!({ "days": days })).await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{days}");
    }

    // The superadmin owns every sandbox; at most 5 live links per playground.
    for _ in 0..3 {
        let more = invite(&w, &w.owner, "p1", json!({"days": 30})).await?;
        assert_eq!(more.status, StatusCode::CREATED, "{}", more.body);
    }
    let sixth = invite(&w, &w.alice, "p1", json!({"days": 30})).await?;
    assert_eq!(sixth.status, StatusCode::CONFLICT, "{}", sixth.body);
    // An expired link no longer counts.
    advance(&w.h, DAY_MS);
    let again = invite(&w, &w.alice, "p1", json!({"days": 30})).await?;
    assert_eq!(again.status, StatusCode::CREATED, "{}", again.body);

    let audited =
        w.h.store
            .audit_details("p1", "playground.invite.create", 10)
            .await?;
    assert_eq!(audited.len(), 6, "{audited:?}");
    Ok(())
}

#[tokio::test]
async fn only_a_playgrounds_admin_creates_links() -> TestResult {
    let w = world().await?;
    // Another person does not see alice's playground.
    let bobs = invite(&w, &w.bob, "p1", json!({})).await?;
    assert_eq!(bobs.status, StatusCode::NOT_FOUND);
    let listed = get(&w, "/api/sandboxes/p1/guest-invites", &w.bob).await?;
    assert_eq!(listed.status, StatusCode::NOT_FOUND);
    // A GitHub sandbox's access mirrors GitHub: no guests.
    let github = invite(&w, &w.alice, "s1", json!({})).await?;
    assert_eq!(github.status, StatusCode::CONFLICT, "{}", github.body);
    // A person lets guests in, not a `yulius login` token.
    let user =
        w.h.store
            .user(&UserId::parse("u-alice")?)
            .await?
            .ok_or("alice")?;
    let cli =
        w.h.app
            .issue_api_token(&user, "cli", TokenScope::Cli)
            .await?;
    let by_token = Creds {
        bearer: Some(cli.expose().to_owned()),
        ..Creds::default()
    };
    let refused = invite(&w, &by_token, "p1", json!({})).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    // A guest never makes one.
    let code = link(&w, 7).await?;
    let (guest, _) = join(&w, &code, "Carol").await?;
    let theirs = invite(&w, &guest, "p1", json!({})).await?;
    assert_eq!(theirs.status, StatusCode::FORBIDDEN, "{}", theirs.body);
    Ok(())
}

#[tokio::test]
async fn a_guest_joins_with_a_name_and_gets_a_session() -> TestResult {
    let w = world().await?;
    let code = link(&w, 1).await?;
    let seen = peek(&w, &code).await?;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(seen.body["playground"], "alices-lab");
    assert_eq!(seen.body["invited_by"], "alice");

    let joined = join_raw(&w, &code, "  Carol  ").await?;
    assert_eq!(joined.status, StatusCode::CREATED, "{}", joined.body);
    assert_eq!(joined.body["slug"], "alices-lab");
    assert_eq!(joined.body["user"]["display_name"], "Carol");
    assert_eq!(joined.body["user"]["login"], "guest");
    assert_eq!(joined.body["user"]["is_superadmin"], false);
    assert_eq!(joined.body["user"]["is_instance_admin"], false);
    assert_eq!(joined.body["user"]["github_linked"], false);
    assert_eq!(joined.body["user"]["guest"]["slug"], "alices-lab");
    assert_eq!(joined.body["user"]["guest"]["sandbox_id"], "p1");
    // The session ends with the link.
    let cookie = joined.set_cookie.clone().ok_or("cookie")?;
    assert!(cookie.contains("Max-Age=86400"), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");

    let (guest, user) = join(&w, &code, "Dana").await?;
    assert!(user.github_login.is_none() && user.password_hash.is_none());
    assert!(!user.is_superadmin && !user.is_instance_admin);
    let me = get(&w, "/api/me", &guest).await?;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.body["user"]["guest"]["slug"], "alices-lab");
    let mine = get(&w, "/api/sandboxes", &guest).await?;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.body);
    assert_eq!(slugs(&mine.body), ["alices-lab"]);
    assert_eq!(mine.body[0]["access"], "write");

    // The admins see who came in.
    let listed = get(&w, "/api/sandboxes/p1/guest-invites", &w.alice).await?;
    let names: Vec<&str> = listed.body["invites"][0]["guests"]
        .as_array()
        .ok_or("guests")?
        .iter()
        .filter_map(|g| g["name"].as_str())
        .collect();
    assert_eq!(names, ["Carol", "Dana"]);

    for bad in ["", "   ", &"x".repeat(61), "a\u{202e}b", "tab\there"] {
        let refused = join_raw(&w, &code, bad).await?;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad:?}");
    }
    let audited =
        w.h.store
            .last_audit("p1", "playground.guest.join", now(&w.h))
            .await?
            .ok_or("join audited")?;
    assert_eq!(audited.actor, "Dana (guest)");
    Ok(())
}

#[tokio::test]
async fn a_dead_link_lets_nobody_in() -> TestResult {
    let w = world().await?;
    let unknown = "0".repeat(64);
    assert_eq!(peek(&w, &unknown).await?.status, StatusCode::NOT_FOUND);
    assert_eq!(
        join_raw(&w, &unknown, "Eve").await?.status,
        StatusCode::NOT_FOUND
    );

    // Revoked.
    let made = invite(&w, &w.alice, "p1", json!({"days": 7})).await?;
    let code = code_of(&made.body)?;
    let id = made.body["invite"]["id"].as_str().ok_or("id")?;
    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/p1/guest-invites/{id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(peek(&w, &code).await?.status, StatusCode::NOT_FOUND);
    assert_eq!(
        join_raw(&w, &code, "Eve").await?.status,
        StatusCode::NOT_FOUND
    );

    // Expired.
    let code = link(&w, 1).await?;
    advance(&w.h, DAY_MS);
    assert_eq!(peek(&w, &code).await?.status, StatusCode::NOT_FOUND);
    assert_eq!(
        join_raw(&w, &code, "Eve").await?.status,
        StatusCode::NOT_FOUND
    );

    // Its playground moved to GitHub, or removed.
    let code = link(&w, 7).await?;
    w.h.store
        .move_to_github(&SandboxId::parse("p1")?, "acme", "lab", Some(9))
        .await?;
    assert_eq!(peek(&w, &code).await?.status, StatusCode::NOT_FOUND);
    let made = invite(&w, &w.bob, "p2", json!({})).await?;
    let code = code_of(&made.body)?;
    w.h.store
        .remove_sandbox(&SandboxId::parse("p2")?, now(&w.h))
        .await?;
    assert_eq!(
        join_raw(&w, &code, "Eve").await?.status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn a_link_lets_in_at_most_twenty_guests() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    for n in 0..20 {
        if n % 10 == 0 {
            advance(&w.h, 61_000); // past the per-client limit's minute
        }
        join(&w, &code, &format!("Guest {n}")).await?;
    }
    advance(&w.h, 61_000);
    let full = join_raw(&w, &code, "One more").await?;
    assert_eq!(full.status, StatusCode::CONFLICT, "{}", full.body);
    // Another link of the same playground still admits.
    let other = link(&w, 7).await?;
    join(&w, &other, "One more").await?;
    Ok(())
}

#[tokio::test]
async fn joining_is_rate_limited_per_client() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    for _ in 0..5 {
        assert_eq!(peek(&w, &code).await?.status, StatusCode::OK);
    }
    for _ in 0..5 {
        assert_eq!(
            peek(&w, &"0".repeat(64)).await?.status,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        join_raw(&w, &code, "Frank").await?.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(peek(&w, &code).await?.status, StatusCode::TOO_MANY_REQUESTS);
    advance(&w.h, 60_000);
    join(&w, &code, "Frank").await?;
    Ok(())
}

#[tokio::test]
async fn a_guest_works_only_in_their_playground() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    let (guest, user) = join(&w, &code, "Carol").await?;
    let app = &w.h.app;
    let p1 = SandboxId::parse("p1")?;
    assert!(app.can_work_in(&user, &p1).await);
    for other in ["p2", "s1"] {
        let sid = SandboxId::parse(other)?;
        assert!(!app.can_view(&user, &sid).await, "{other}");
        let direct = get(&w, &format!("/api/sandboxes/{other}"), &guest).await?;
        assert_eq!(direct.status, StatusCode::NOT_FOUND, "{other}");
        let tasks = get(&w, &format!("/api/sandboxes/{other}/tasks"), &guest).await?;
        assert_eq!(tasks.status, StatusCode::NOT_FOUND, "{other}");
    }
    let opened = get(&w, "/api/sandboxes/p1", &guest).await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["access"], "write");
    let tasks = get(&w, "/api/sandboxes/p1/tasks", &guest).await?;
    assert_eq!(tasks.status, StatusCode::OK, "{}", tasks.body);
    // A guest of bob's playground is not one of alice's.
    let made = invite(&w, &w.bob, "p2", json!({})).await?;
    let (_, bobs_guest) = join(&w, &code_of(&made.body)?, "Gil").await?;
    assert!(!app.can_view(&bobs_guest, &p1).await);
    Ok(())
}

#[tokio::test]
async fn a_guest_never_configures_the_playground() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    let (guest, user) = join(&w, &code, "Carol").await?;
    let p1 = SandboxId::parse("p1")?;
    for action in [Action::Configure, Action::Administer] {
        let refused = w.h.app.require(&user, &p1, action).await;
        assert!(
            matches!(refused, Err(yulius_app::error::AppError::Forbidden(_))),
            "{action:?}: {refused:?}"
        );
    }
    let patched = call(
        &w.h.router,
        Method::PATCH,
        "/api/sandboxes/p1",
        Some(json!({"vidoc": false})),
        &guest,
    )
    .await?;
    assert_eq!(patched.status, StatusCode::FORBIDDEN, "{}", patched.body);
    let removed = call(
        &w.h.router,
        Method::DELETE,
        "/api/sandboxes/p1",
        None,
        &guest,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::FORBIDDEN, "{}", removed.body);
    // The maker still configures it.
    assert!(
        w.h.app
            .require(&user_of(&w, "u-alice").await?, &p1, Action::Administer)
            .await
            .is_ok()
    );
    Ok(())
}

async fn user_of(w: &World, id: &str) -> BoxResult<UserRecord> {
    Ok(w.h.store.user(&UserId::parse(id)?).await?.ok_or("user")?)
}

#[tokio::test]
async fn a_guest_reaches_no_secret_or_setting() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    let (guest, _) = join(&w, &code, "Carol").await?;
    let refused: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/sandboxes/p1/agent-secrets", None),
        (
            Method::POST,
            "/api/sandboxes/p1/agent-secrets",
            Some(json!({})),
        ),
        (Method::GET, "/api/sandboxes/p1/variables", None),
        (Method::GET, "/api/sandboxes/p1/team-secrets", None),
        (Method::GET, "/api/sandboxes/p1/mcp", None),
        (Method::GET, "/api/sandboxes/p1/skills", None),
        (Method::GET, "/api/sandboxes/p1/automations", None),
        (Method::GET, "/api/sandboxes/p1/workflows", None),
        (Method::GET, "/api/sandboxes/p1/agent-states", None),
        (Method::POST, "/api/sandboxes/p1/cockpit", Some(json!({}))),
        (Method::GET, "/api/sandboxes/p1/cockpit/link", None),
        (Method::GET, "/api/sandboxes/p1/guest-invites", None),
        (
            Method::POST,
            "/api/sandboxes/p1/guest-invites",
            Some(json!({})),
        ),
        (Method::GET, "/api/team-secrets", None),
        (Method::POST, "/api/team-secrets", Some(json!({}))),
        (Method::GET, "/api/secrets/activity", None),
        (
            Method::PATCH,
            "/api/agent-secrets/x",
            Some(json!({"description": "x"})),
        ),
        (Method::GET, "/api/me/tokens", None),
        (
            Method::POST,
            "/api/me/tokens",
            Some(json!({"label": "x", "scope": "operate"})),
        ),
        (
            Method::POST,
            "/api/device/approve",
            Some(json!({"user_code": "ABCD-EFGH", "approve": true})),
        ),
        (
            Method::POST,
            "/api/sandboxes",
            Some(json!({"mode": "playground", "name": "mine", "template": "empty"})),
        ),
        (Method::POST, "/api/playgrounds?name=mine", Some(json!({}))),
        (Method::POST, "/api/import", Some(json!({}))),
        (Method::POST, "/api/onboard/plan", Some(json!({}))),
        (Method::GET, "/api/github/repositories", None),
        (Method::GET, "/api/status", None),
        (Method::GET, "/api/disk", None),
        (Method::GET, "/api/capacity", None),
        (Method::GET, "/api/connections", None),
        (Method::GET, "/api/update", None),
        (Method::GET, "/api/notes", None),
        (Method::GET, "/api/analytics/summary", None),
        (Method::GET, "/api/admin/users", None),
        (Method::GET, "/api/admin/accounts", None),
        (Method::DELETE, "/api/mcp/x", None),
        (Method::POST, "/api/mcp/x/test", Some(json!({}))),
        (Method::DELETE, "/api/workflows/x", None),
        (Method::DELETE, "/api/automations/x", None),
        (Method::POST, "/api/auth/reauth", Some(json!({}))),
    ];
    for (method, path, body) in refused {
        let resp = call(&w.h.router, method.clone(), path, body.clone(), &guest).await?;
        assert_eq!(
            resp.status,
            StatusCode::FORBIDDEN,
            "{method} {path}: {}",
            resp.body
        );
        assert!(
            resp.body.to_string().contains(FENCE),
            "{method} {path}: {}",
            resp.body
        );
    }
    for path in [
        "/api/me",
        "/api/sandboxes",
        "/api/sandboxes/p1/tasks",
        "/api/archived-tasks",
        "/api/me/notes",
    ] {
        let resp = get(&w, path, &guest).await?;
        assert_eq!(resp.status, StatusCode::OK, "{path}: {}", resp.body);
    }
    // The maker reaches the same routes as before.
    let theirs = get(&w, "/api/sandboxes/p1/agent-secrets", &w.alice).await?;
    assert_eq!(theirs.status, StatusCode::OK, "{}", theirs.body);
    Ok(())
}

#[tokio::test]
async fn a_guests_task_gets_no_agent_secret() -> TestResult {
    let w = world().await?;
    support::exhausted_subscription(&w.h).await?;
    let host = "api.lab.test";
    let value = "lab-project-token-5150a1ce";
    let added = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/p1/agent-secrets",
        Some(json!({"scope": "sandbox", "name": "LAB_TOKEN", "value": value, "hosts": [host], "injection": "bearer"})),
        &w.alice,
    )
    .await?;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    let code = link(&w, 7).await?;
    let (_, guest) = join(&w, &code, "Carol").await?;
    seed_task(&w.h, "t1", "p1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&w.h, "t1", ALICE_TASK).await?;
    seed_task(
        &w.h,
        "t2",
        "p1",
        guest.id.as_str(),
        TaskStatus::Running,
        None,
    )
    .await?;
    seed_live_attempt(&w.h, "t2", GUEST_TASK).await?;

    let sent = |token: &'static str| {
        let app = w.h.app.clone();
        async move {
            let client = app.egress_client(token).await?;
            let request = EgressRequest {
                host: host.into(),
                port: 443,
                path_and_query: "/v1".into(),
                headers: vec![(
                    "authorization".into(),
                    b"Bearer yulius-secret-LAB_TOKEN".to_vec(),
                )],
            };
            match app.egress_apply(&client, request).await {
                EgressDecision::Forward { headers, .. } => Some(
                    headers
                        .iter()
                        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                EgressDecision::Refuse { .. } => None,
            }
        }
    };
    let makers = sent(ALICE_TASK)
        .await
        .ok_or("the maker's run reaches the host")?;
    assert!(
        makers.contains(value),
        "the maker's task gets the project secret"
    );
    let guests = sent(GUEST_TASK).await.unwrap_or_default();
    assert!(
        !guests.contains(value),
        "a guest's task gets no agent secret: {guests}"
    );
    Ok(())
}

#[tokio::test]
async fn revoking_a_link_ends_its_guests_at_once() -> TestResult {
    let w = world().await?;
    let made = invite(&w, &w.alice, "p1", json!({"days": 7})).await?;
    let id = made.body["invite"]["id"].as_str().ok_or("id")?.to_owned();
    let (gone, gone_user) = join(&w, &code_of(&made.body)?, "Carol").await?;
    let kept_code = link(&w, 7).await?;
    let (kept, kept_user) = join(&w, &kept_code, "Dana").await?;
    assert_eq!(get(&w, "/api/me", &gone).await?.status, StatusCode::OK);

    // Only an admin revokes.
    let path = format!("/api/sandboxes/p1/guest-invites/{id}");
    for creds in [&w.bob, &kept] {
        let refused = call(&w.h.router, Method::DELETE, &path, None, creds).await?;
        assert!(
            matches!(
                refused.status,
                StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
            ),
            "{}",
            refused.status
        );
    }
    let revoked = call(&w.h.router, Method::DELETE, &path, None, &w.alice).await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.body);
    assert_eq!(
        get(&w, "/api/me", &gone).await?.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(!w.h.app.can_view(&gone_user, &SandboxId::parse("p1")?).await);
    // Revoking it again finds no live link.
    let again = call(&w.h.router, Method::DELETE, &path, None, &w.alice).await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // The other link's guest stays in.
    assert_eq!(get(&w, "/api/me", &kept).await?.status, StatusCode::OK);
    assert!(
        w.h.app
            .can_work_in(&kept_user, &SandboxId::parse("p1")?)
            .await
    );
    let listed = get(&w, "/api/sandboxes/p1/guest-invites", &w.alice).await?;
    assert_eq!(listed.body["invites"].as_array().map(Vec::len), Some(1));
    let audited =
        w.h.store
            .audit_details("p1", "playground.invite.revoke", 5)
            .await?;
    assert_eq!(audited, [id]);
    Ok(())
}

#[tokio::test]
async fn an_expired_link_ends_its_guests() -> TestResult {
    let w = world().await?;
    let code = link(&w, 1).await?;
    let (guest, user) = join(&w, &code, "Carol").await?;
    advance(&w.h, DAY_MS - 1);
    assert_eq!(get(&w, "/api/me", &guest).await?.status, StatusCode::OK);
    assert!(w.h.app.can_work_in(&user, &SandboxId::parse("p1")?).await);
    advance(&w.h, 1);
    assert_eq!(
        get(&w, "/api/me", &guest).await?.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(!w.h.app.can_view(&user, &SandboxId::parse("p1")?).await);
    Ok(())
}

#[tokio::test]
async fn moving_or_removing_the_playground_ends_guest_access() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    let (_, user) = join(&w, &code, "Carol").await?;
    let p1 = SandboxId::parse("p1")?;
    assert!(w.h.app.can_work_in(&user, &p1).await);
    w.h.store
        .move_to_github(&p1, "acme", "lab", Some(9))
        .await?;
    assert!(
        !w.h.app.can_view(&user, &p1).await,
        "access mirrors GitHub now"
    );

    let made = invite(&w, &w.bob, "p2", json!({})).await?;
    let (_, bobs) = join(&w, &code_of(&made.body)?, "Dana").await?;
    let p2 = SandboxId::parse("p2")?;
    assert!(w.h.app.can_work_in(&bobs, &p2).await);
    w.h.store.remove_sandbox(&p2, now(&w.h)).await?;
    assert!(!w.h.app.can_view(&bobs, &p2).await);
    Ok(())
}

#[tokio::test]
async fn a_guest_is_named_as_a_guest() -> TestResult {
    let w = world().await?;
    let code = link(&w, 7).await?;
    let (guest, user) = join(&w, &code, "Carol").await?;
    assert_eq!(user.label(), "Carol (guest)");
    seed_task(
        &w.h,
        "t1",
        "p1",
        user.id.as_str(),
        TaskStatus::Running,
        None,
    )
    .await?;
    let tasks = get(&w, "/api/sandboxes/p1/tasks", &guest).await?;
    assert_eq!(tasks.status, StatusCode::OK, "{}", tasks.body);
    let text = tasks.body.to_string();
    assert!(text.contains("Carol (guest)"), "{text}");
    assert!(!text.contains("superadmin"), "{text}");
    Ok(())
}

#[tokio::test]
async fn a_guests_event_stream_carries_only_their_playground_and_ends_with_the_link() -> TestResult
{
    use futures::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;
    use yulius_contracts::events::UiEvent;
    let w = world().await?;
    let made = invite(&w, &w.alice, "p1", json!({"days": 7})).await?;
    let id = made.body["invite"]["id"].as_str().ok_or("id")?.to_owned();
    let (guest, _) = join(&w, &code_of(&made.body)?, "Carol").await?;
    let served = support::serve(w.h.router.clone()).await?;
    let origin = w.h.app.config.public_url.clone();
    let mut socket = support::ws_connect(served.addr, "/api/events", &guest, &origin).await?;
    let mark = |sandbox: Option<&str>, task: &str| -> TestResult {
        w.h.app.events.publish(
            sandbox.map(SandboxId::parse).transpose()?,
            false,
            UiEvent::TaskRemoved {
                task_id: task.into(),
            },
        );
        Ok(())
    };
    // The server subscribes once the upgrade completes: mark until the stream has it.
    let mut ready = false;
    for _ in 0..40 {
        mark(Some("p1"), "t-ready")?;
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(std::time::Duration::from_millis(250), socket.next()).await
            && text.contains("t-ready")
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "the guest's stream never subscribed");

    mark(None, "t-instance-wide")?;
    mark(Some("p2"), "t-bobs-lab")?;
    mark(Some("p1"), "t-after")?;
    let mut frames = Vec::new();
    loop {
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
            .await
            .map_err(|_| format!("no t-after within 10 s: {frames:?}"))?;
        if let Some(Ok(Message::Text(text))) = next {
            let text = text.to_string();
            let done = text.contains("t-after");
            frames.push(text);
            if done {
                break;
            }
        }
    }
    assert!(
        !frames
            .iter()
            .any(|f| f.contains("t-instance-wide") || f.contains("t-bobs-lab")),
        "{frames:?}"
    );

    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/p1/guest-invites/{id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    // The stream closes at its next re-check.
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the revoked guest's stream stays open");
    Ok(())
}
