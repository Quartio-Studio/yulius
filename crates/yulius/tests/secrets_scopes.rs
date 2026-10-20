//! Agent secrets of three scopes (SPEC-245, ADR-0186): a project's, global ones an administrator
//! switches on per project, and personal ones only their owner's tasks get. Resolution goes
//! through the egress proxy as a run sees it.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, perms, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task,
};
use yulius_app::ports::egress::{EgressClient, EgressDecision, EgressPolicy as _, EgressRequest};
use yulius_app::ports::store::{SandboxStore as _, TaskStore as _};
use yulius_app::vault::Holder;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;

/// Alice's task in `app`.
const ALICE_TASK: &str = "scopes-task-token-alice-0123456789abcdef";
/// Bob's task in `app`.
const BOB_TASK: &str = "scopes-task-token-bob-0123456789abcdefgh";
/// A task an automation of alice's started in `app`.
const AUTOMATED_TASK: &str = "scopes-task-token-automation-0123456789";
/// Bob's task in `web`.
const BOB_WEB_TASK: &str = "scopes-task-token-bob-web-0123456789abcd";
const MINE: &str = "alice-personal-token-5150a1ce";
const HOST: &str = "api.mine.test";

struct World {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
    admin: Creds,
}

/// Sandboxes s1 (`app`) and s2 (`web`), both written by alice and bob; the superadmin
/// administers the instance. Live tasks: alice's and bob's in s1, an automation's (owned by
/// alice) in s1, bob's in s2.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    seed_sandbox(&h, "s2", "web", "u-alice").await?;
    for repo in ["acme/app", "acme/web"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("write"));
    }
    support::setup_superadmin_creds(&h).await?;
    let admin = support::superadmin_bearer(&h).await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", ALICE_TASK).await?;
    seed_task(&h, "t2", "s1", "u-bob", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t2", BOB_TASK).await?;
    let mut automated = seed_task(&h, "t0", "s1", "u-alice", TaskStatus::Running, None).await?;
    automated.id = TaskId::parse("t3")?;
    automated.origin = yulius_app::model::TaskOrigin::Automation;
    h.store.insert_task(&automated).await?;
    seed_live_attempt(&h, "t3", AUTOMATED_TASK).await?;
    seed_task(&h, "t4", "s2", "u-bob", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t4", BOB_WEB_TASK).await?;
    Ok(World {
        h,
        alice,
        bob,
        admin,
    })
}

async fn add(
    w: &World,
    creds: &Creds,
    sandbox: &str,
    body: Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let created = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/agent-secrets"),
        Some(body),
        creds,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(created.body["id"].as_str().ok_or("id")?.to_owned())
}

/// A bearer secret bound to `host`.
fn secret(scope: &str, name: &str, value: &str, host: &str) -> Value {
    json!({"scope": scope, "name": name, "value": value, "hosts": [host], "injection": "bearer"})
}

async fn client(w: &World, token: &str) -> Result<EgressClient, Box<dyn std::error::Error>> {
    Ok(w.h.app.egress_client(token).await.ok_or("proxy client")?)
}

/// The `Authorization` header the proxy forwards for a request to `host` that carries none.
async fn sent(w: &World, client: &EgressClient, host: &str) -> Option<String> {
    let request = EgressRequest {
        host: host.into(),
        port: 443,
        path_and_query: "/v1/items".into(),
        headers: vec![("x-trace".into(), b"Bearer yulius-secret-MY_TOKEN".to_vec())],
    };
    match w.h.app.egress_apply(client, request).await {
        EgressDecision::Forward { headers, .. } => headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned()),
        EgressDecision::Refuse { .. } => None,
    }
}

async fn listed(
    w: &World,
    creds: &Creds,
    sandbox: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let list = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/agent-secrets"),
        None,
        creds,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    Ok(list.body.as_array().ok_or("list")?.clone())
}

fn names(list: &[Value]) -> Vec<&str> {
    list.iter().filter_map(|s| s["name"].as_str()).collect()
}

async fn switch(
    w: &World,
    creds: &Creds,
    sandbox: &str,
    id: &str,
    on: bool,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/{sandbox}/agent-secrets/{id}/enabled"),
        Some(json!({"enabled": on})),
        creds,
    )
    .await
}

/// SPEC-245 RES-002..004, MAN-003: alice's personal secret reaches her own task's requests and no
/// one else's — not bob's task in the same project, not a task her automation started, not a CI
/// job granted its name; bob, and administrators, never see it.
#[tokio::test]
async fn a_personal_secret_reaches_only_its_owners_tasks() -> TestResult {
    let w = world().await?;
    let id = add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;

    let mine = client(&w, ALICE_TASK).await?;
    assert!(w.h.app.egress_intercepts(&mine, HOST, 443).await);
    assert_eq!(sent(&w, &mine, HOST).await, Some(format!("Bearer {MINE}")));

    let s1 = SandboxId::parse("s1")?;
    let ci =
        w.h.app
            .ci_egress_open(&s1, "run-1", "job-1", &["MY_TOKEN".to_owned()])
            .ok_or("ci credential")?;
    for (who, token) in [
        ("bob's task", BOB_TASK),
        ("the automation's task", AUTOMATED_TASK),
        ("a CI job", ci.as_str()),
    ] {
        let other = client(&w, token).await?;
        assert!(!w.h.app.egress_intercepts(&other, HOST, 443).await, "{who}");
        let header = sent(&w, &other, HOST).await;
        assert!(
            !header.unwrap_or_default().contains(MINE),
            "{who} never gets it"
        );
    }
    w.h.app.ci_egress_close(&ci);

    // What agents are told.
    let alice_list = support::mcp_tool(&w.h, ALICE_TASK, "secrets_list", json!({})).await?;
    assert!(alice_list.to_string().contains("MY_TOKEN"), "{alice_list}");
    for token in [BOB_TASK, AUTOMATED_TASK] {
        let other = support::mcp_tool(&w.h, token, "secrets_list", json!({})).await?;
        assert!(!other.to_string().contains("MY_TOKEN"), "{other}");
    }

    // People: only alice sees it; to bob and the administrator it does not exist.
    assert_eq!(names(&listed(&w, &w.alice, "s1").await?), ["MY_TOKEN"]);
    for creds in [&w.bob, &w.admin] {
        assert!(names(&listed(&w, creds, "s1").await?).is_empty());
        for (method, path, body) in [
            (
                Method::PATCH,
                format!("/api/agent-secrets/{id}"),
                Some(json!({"description": "x"})),
            ),
            (Method::DELETE, format!("/api/agent-secrets/{id}"), None),
            (
                Method::POST,
                format!("/api/agent-secrets/{id}/reveal"),
                Some(json!({"purpose": "show"})),
            ),
            (
                Method::PUT,
                format!("/api/sandboxes/s1/agent-secrets/{id}/enabled"),
                Some(json!({"enabled": false})),
            ),
        ] {
            let refused = call(&w.h.router, method, &path, body, creds).await?;
            assert_eq!(
                refused.status,
                StatusCode::NOT_FOUND,
                "{path}: {}",
                refused.body
            );
        }
    }
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.bob,
    )
    .await?;
    assert!(
        !activity.body.to_string().contains("MY_TOKEN"),
        "{}",
        activity.body
    );

    // Bob's run holds no hashes of it; Yulius' own redaction of the project covers it.
    let bob_id = UserId::parse("u-bob")?;
    let alice_id = UserId::parse("u-alice")?;
    let bobs =
        w.h.app
            .scrub_fingerprints_for(&s1, Holder::Person(&bob_id))
            .await?;
    assert!(
        !bobs.names.iter().any(|n| n == "MY_TOKEN"),
        "{:?}",
        bobs.names
    );
    let alices =
        w.h.app
            .scrub_fingerprints_for(&s1, Holder::Person(&alice_id))
            .await?;
    assert!(
        alices.names.iter().any(|n| n == "MY_TOKEN"),
        "{:?}",
        alices.names
    );
    let redacted =
        w.h.app
            .redact_for_sandbox(&s1, &format!("echo {MINE}"))
            .await;
    assert_eq!(redacted, "echo [secret:MY_TOKEN]");

    // Switched off for the project, her own task no longer gets it either.
    let off = switch(&w, &w.alice, "s1", &id, false).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["enabled"], false);
    assert!(!w.h.app.egress_intercepts(&mine, HOST, 443).await);
    Ok(())
}

/// SPEC-245 SCP-003, MAN-002, MAN-004: a global secret reaches the projects an administrator
/// switched it on for; a project writer cannot switch it.
#[tokio::test]
async fn global_secrets_reach_the_projects_an_administrator_switched_them_on_for() -> TestResult {
    let w = world().await?;
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(secret(
            "instance",
            "ARGO_TOKEN",
            "argo-shared-token-77",
            "argo.test",
        )),
        &w.alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let id = add(
        &w,
        &w.admin,
        "s1",
        secret(
            "instance",
            "ARGO_TOKEN",
            "argo-shared-token-77",
            "argo.test",
        ),
    )
    .await?;

    let in_app = client(&w, BOB_TASK).await?;
    let in_web = client(&w, BOB_WEB_TASK).await?;
    assert!(
        w.h.app.egress_intercepts(&in_app, "argo.test", 443).await,
        "on where it was added"
    );
    assert!(
        !w.h.app.egress_intercepts(&in_web, "argo.test", 443).await,
        "off elsewhere"
    );
    let web = listed(&w, &w.bob, "s2").await?;
    assert_eq!(web.len(), 1);
    assert_eq!(
        (
            web[0]["scope"].as_str(),
            web[0]["enabled"].as_bool(),
            web[0]["can_manage"].as_bool()
        ),
        (Some("instance"), Some(false), Some(false))
    );

    let by_writer = switch(&w, &w.alice, "s2", &id, true).await?;
    assert_eq!(
        by_writer.status,
        StatusCode::FORBIDDEN,
        "{}",
        by_writer.body
    );
    let project = add(
        &w,
        &w.alice,
        "s2",
        secret("sandbox", "WEB_KEY", "web-project-key-123", "web.test"),
    )
    .await?;
    let not_switchable = switch(&w, &w.alice, "s2", &project, false).await?;
    assert_eq!(
        not_switchable.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        not_switchable.body
    );

    let on = switch(&w, &w.admin, "s2", &id, true).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.body["enabled"], true);
    assert_eq!(
        sent(&w, &in_web, "argo.test").await,
        Some("Bearer argo-shared-token-77".to_owned())
    );
    let audit = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.admin,
    )
    .await?;
    assert!(
        audit.body.to_string().contains("vault.agent.enable"),
        "{}",
        audit.body
    );
    Ok(())
}

/// SPEC-245 RES-001, MAN-005: a name resolves to the project's secret, else the run's person's,
/// else the global one; the list says which wins.
#[tokio::test]
async fn a_name_resolves_project_then_personal_then_global() -> TestResult {
    let w = world().await?;
    let global = add(
        &w,
        &w.admin,
        "s1",
        secret("instance", "KEY", "global-value-0001", "api.key.test"),
    )
    .await?;
    let personal = add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "KEY", "alice-value-0002", "api.key.test"),
    )
    .await?;
    let project = add(
        &w,
        &w.alice,
        "s1",
        secret("sandbox", "KEY", "project-value-0003", "api.key.test"),
    )
    .await?;
    let (alice, bob) = (client(&w, ALICE_TASK).await?, client(&w, BOB_TASK).await?);

    assert_eq!(
        sent(&w, &alice, "api.key.test").await,
        Some("Bearer project-value-0003".into())
    );
    assert_eq!(
        sent(&w, &bob, "api.key.test").await,
        Some("Bearer project-value-0003".into())
    );
    let shadow = |list: &[Value], id: &str| {
        list.iter()
            .find(|s| s["id"] == id)
            .map(|s| s["shadowed_by"].clone())
    };
    let list = listed(&w, &w.alice, "s1").await?;
    assert_eq!(shadow(&list, &global), Some(json!("sandbox")));
    assert_eq!(shadow(&list, &personal), Some(json!("sandbox")));
    assert_eq!(shadow(&list, &project), Some(Value::Null));

    let gone = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/agent-secrets/{project}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.body);
    assert_eq!(
        sent(&w, &alice, "api.key.test").await,
        Some("Bearer alice-value-0002".into())
    );
    assert_eq!(
        sent(&w, &bob, "api.key.test").await,
        Some("Bearer global-value-0001".into())
    );
    let list = listed(&w, &w.alice, "s1").await?;
    assert_eq!(shadow(&list, &global), Some(json!("personal")));
    let bobs = listed(&w, &w.bob, "s1").await?;
    assert_eq!(
        shadow(&bobs, &global),
        Some(Value::Null),
        "alice's secret shadows nothing for bob"
    );
    Ok(())
}

/// SPEC-245 RES-005: a personal secret is used through the proxy only: no `.env` copy, no
/// browser filling, no MCP reference, no production deploy.
#[tokio::test]
async fn personal_secrets_stay_in_the_proxy() -> TestResult {
    let w = world().await?;
    let id = add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    let copy = support::mcp_tool(
        &w.h,
        ALICE_TASK,
        "secret_copy_to_env",
        json!({"secrets": ["MY_TOKEN"]}),
    )
    .await?;
    assert_eq!(copy["isError"], true, "{copy}");
    assert!(
        copy.to_string().contains("never copied into .env files"),
        "{copy}"
    );
    let fill = support::mcp_tool(
        &w.h,
        ALICE_TASK,
        "browser_fill_secret",
        json!({"tab_id": "tab-1", "secret": "MY_TOKEN"}),
    )
    .await?;
    assert_eq!(fill["isError"], true, "{fill}");
    assert!(
        fill.to_string()
            .contains("never typed into the project browser"),
        "{fill}"
    );
    let sandbox =
        w.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    assert_eq!(
        w.h.app.resolve_secret_ref(&sandbox, "MY_TOKEN").await?,
        None
    );
    let prod = call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{id}/cd-prod"),
        Some(json!({"allow": true})),
        &w.alice,
    )
    .await?;
    assert_eq!(prod.status, StatusCode::CONFLICT, "{}", prod.body);
    Ok(())
}

/// SPEC-245 MAN-003: a person adds a personal secret only in a project they may work in.
#[tokio::test]
async fn a_reader_cannot_switch_a_personal_secret_on() -> TestResult {
    let w = world().await?;
    w.h.github.set_perm("bob", "acme/web", perms("read"));
    let id = add(
        &w,
        &w.bob,
        "s1",
        secret("personal", "BOB_KEY", "bob-personal-9876", "bob.test"),
    )
    .await?;
    let refused = switch(&w, &w.bob, "s2", &id, true).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s2/agent-secrets",
        Some(secret("personal", "OTHER", "bob-personal-5432", "bob.test")),
        &w.bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    Ok(())
}
