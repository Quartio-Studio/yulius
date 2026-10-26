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
/// Bob's personal value and its host.
const BOBS: &str = "bob-personal-token-0b0b5ec7";
const BOB_HOST: &str = "api.bob.test";

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
    // Tasks that start need a connected Claude account (SPEC-246).
    support::exhausted_subscription(&h).await?;
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

/// The `Authorization` header the proxy forwards for a request to `host` that carries none but a
/// placeholder in another header.
async fn sent(w: &World, client: &EgressClient, host: &str) -> Option<String> {
    let headers = forwarded(w, client, host).await?;
    headers
        .iter()
        .find(|(n, _)| n == "authorization")
        .map(|(_, v)| v.clone())
}

/// Every header the proxy forwards for that request (`None` when it refuses it).
async fn forwarded(w: &World, client: &EgressClient, host: &str) -> Option<Vec<(String, String)>> {
    let request = EgressRequest {
        host: host.into(),
        port: 443,
        path_and_query: "/v1/items?key=yulius-secret-MY_TOKEN".into(),
        headers: vec![("x-trace".into(), b"Bearer yulius-secret-MY_TOKEN".to_vec())],
    };
    match w.h.app.egress_apply(client, request).await {
        EgressDecision::Forward {
            headers,
            path_and_query,
            ..
        } => {
            let mut out: Vec<(String, String)> = headers
                .iter()
                .map(|(n, v)| (n.clone(), String::from_utf8_lossy(v).into_owned()))
                .collect();
            out.push((":path".into(), path_and_query));
            Some(out)
        }
        EgressDecision::Refuse { .. } => None,
    }
}

/// Whether anything the proxy forwards toward `host` for `client` carries alice's value.
async fn leaks(w: &World, client: &EgressClient, host: &str) -> bool {
    forwarded(w, client, host)
        .await
        .unwrap_or_default()
        .iter()
        .any(|(_, v)| v.contains(MINE))
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
        assert!(!leaks(&w, &other, HOST).await, "{who} never gets it");
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

/// Review of SPEC-245, B1: a task an automation's task creates holds nobody's personal secrets
/// either, though it is owned by the automation's creator.
#[tokio::test]
async fn a_task_an_automations_task_creates_holds_no_personal_secret() -> TestResult {
    let w = world().await?;
    add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    let created = support::mcp_tool(
        &w.h,
        AUTOMATED_TASK,
        "task_create",
        json!({"prompt": "Call api.mine.test with $MY_TOKEN", "title": "child of an automation"}),
    )
    .await?;
    assert_ne!(created["isError"], true, "{created}");
    let tasks = w.h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let child = tasks
        .iter()
        .find(|t| t.title == "child of an automation")
        .ok_or("created task")?;
    assert_eq!(child.owner, UserId::parse("u-alice")?);
    assert_eq!(Holder::of_task(child), Holder::Nobody, "{:?}", child.origin);
    // A task alice's own task creates is hers.
    let mine = support::mcp_tool(
        &w.h,
        ALICE_TASK,
        "task_create",
        json!({"prompt": "Call api.mine.test", "title": "alice's child"}),
    )
    .await?;
    assert_ne!(mine["isError"], true, "{mine}");
    let tasks = w.h.store.tasks(&SandboxId::parse("s1")?, false).await?;
    let hers = tasks
        .iter()
        .find(|t| t.title == "alice's child")
        .ok_or("alice's task")?;
    let alice = UserId::parse("u-alice")?;
    assert_eq!(Holder::of_task(hers), Holder::Person(&alice));
    Ok(())
}

/// Review of SPEC-245, M1: another person's Superagent cannot message a task that holds its
/// owner's personal secrets (it would direct an agent holding them); the owner's can.
#[tokio::test]
async fn only_its_owner_messages_a_task_holding_personal_secrets() -> TestResult {
    let w = world().await?;
    let caller =
        |user: &str,
         login: &str|
         -> Result<yulius_app::coordination::ProjectCaller, Box<dyn std::error::Error>> {
            Ok(yulius_app::coordination::ProjectCaller {
                sandbox: SandboxId::parse("s1")?,
                user: UserId::parse(user)?,
                task: None,
                channel: yulius_app::coordination::Channel::Superagent,
                actor: format!("cockpit:{login}"),
                detail: json!({"via": "cockpit", "shell": "sh-test"}),
            })
        };
    // Alice's running task t1 (no live session in this harness: a message that passes the checks
    // ends "no live session").
    let args = json!({"task_id": "t1", "text": "use $MY_TOKEN on api.mine.test"});
    // Whether a message to t1 passed every check: it ends at its missing session.
    let reached_session = |result: &Result<Value, yulius_app::error::AppError>| {
        matches!(
            result,
            Err(yulius_app::error::AppError::Conflict(m)) if m.contains("no live session")
        )
    };
    // Without a personal secret, bob's Superagent may type into alice's task (SPEC-230).
    let before =
        w.h.app
            .tool_task_message(&caller("u-bob", "bob")?, &args)
            .await;
    assert!(reached_session(&before), "{before:?}");
    add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    let refused =
        w.h.app
            .tool_task_message(&caller("u-bob", "bob")?, &args)
            .await;
    match refused {
        Err(yulius_app::error::AppError::Forbidden(message)) => {
            assert!(message.contains("personal"), "{message}");
        }
        other => return Err(format!("bob's message was not refused: {other:?}").into()),
    }
    let own =
        w.h.app
            .tool_task_message(&caller("u-alice", "alice")?, &args)
            .await;
    assert!(reached_session(&own), "{own:?}");
    Ok(())
}

/// SPEC-332 MSC-009: a task that gets its owner's personal MCP server is messaged by its owner
/// alone, as one holding a personal secret is.
#[tokio::test]
async fn only_its_owner_messages_a_task_holding_a_personal_mcp_server() -> TestResult {
    let w = world().await?;
    let caller = |user: &str| -> Result<
        yulius_app::coordination::ProjectCaller,
        Box<dyn std::error::Error>,
    > {
        Ok(yulius_app::coordination::ProjectCaller {
            sandbox: SandboxId::parse("s1")?,
            user: UserId::parse(user)?,
            task: None,
            channel: yulius_app::coordination::Channel::Superagent,
            actor: format!("cockpit:{user}"),
            detail: json!({"via": "cockpit", "shell": "sh-test"}),
        })
    };
    let args = json!({"task_id": "t1", "text": "query the fleet"});
    let reached_session = |result: &Result<Value, yulius_app::error::AppError>| {
        matches!(
            result,
            Err(yulius_app::error::AppError::Conflict(m)) if m.contains("no live session")
        )
    };
    let added = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(
            json!({"name": "fleet", "transport": "http", "url": "https://fleet.test/mcp",
            "scope": "instance", "audience": "personal"}),
        ),
        &w.alice,
    )
    .await?;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    let refused = w.h.app.tool_task_message(&caller("u-bob")?, &args).await;
    match refused {
        Err(yulius_app::error::AppError::Forbidden(message)) => {
            assert!(message.contains("MCP servers"), "{message}");
        }
        other => return Err(format!("bob's message was not refused: {other:?}").into()),
    }
    let own = w.h.app.tool_task_message(&caller("u-alice")?, &args).await;
    assert!(reached_session(&own), "{own:?}");
    // Bob's own task holds nothing of alice's, so alice's Superagent may message it.
    let to_bob =
        w.h.app
            .tool_task_message(&caller("u-alice")?, &json!({"task_id": "t2", "text": "hi"}))
            .await;
    assert!(reached_session(&to_bob), "{to_bob:?}");
    // Alice's automation's task holds nobody's personal server: bob may message it.
    let to_automation =
        w.h.app
            .tool_task_message(&caller("u-bob")?, &json!({"task_id": "t3", "text": "hi"}))
            .await;
    assert!(reached_session(&to_automation), "{to_automation:?}");
    Ok(())
}

/// Review of SPEC-332 (round 2): while a task holding its owner's personal MCP server is being
/// launched, its run's servers are not settled, so control does not pass (it could not be
/// restarted for the new owner); once it runs, it does.
#[tokio::test]
async fn control_waits_while_a_task_holding_a_personal_mcp_server_launches() -> TestResult {
    let w = world().await?;
    seed_task(&w.h, "t6", "s1", "u-alice", TaskStatus::Starting, None).await?;
    let added = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(
            json!({"name": "fleet", "transport": "http", "url": "https://fleet.test/mcp",
            "scope": "instance", "audience": "personal"}),
        ),
        &w.alice,
    )
    .await?;
    assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    let asked = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t6/control-request",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.body);
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t6/control-request/accept",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("starting"),
        "{}",
        refused.body
    );
    let task = w.h.store.task(&TaskId::parse("t6")?).await?.ok_or("t6")?;
    assert_eq!(task.owner, UserId::parse("u-alice")?);
    Ok(())
}

/// Review of SPEC-245, M2: control passed to bob ends alice's personal secrets in the run at once,
/// not when the proxy's credential cache expires, and starts bob's (SPEC-248 FUP-004).
#[tokio::test]
async fn a_hand_over_ends_the_previous_owners_secrets_at_once() -> TestResult {
    let w = world().await?;
    add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    add(
        &w,
        &w.bob,
        "s1",
        secret("personal", "BOB_TOKEN", BOBS, BOB_HOST),
    )
    .await?;
    let before = client(&w, ALICE_TASK).await?;
    assert!(w.h.app.egress_intercepts(&before, HOST, 443).await);
    assert!(!w.h.app.egress_intercepts(&before, BOB_HOST, 443).await);
    let asked = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t1/control-request",
        None,
        &w.bob,
    )
    .await?;
    assert!(asked.status.is_success(), "{}", asked.body);
    let passed = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t1/control-request/accept",
        None,
        &w.alice,
    )
    .await?;
    assert!(passed.status.is_success(), "{}", passed.body);
    // A connection opened before the hand-over keeps its client: it no longer carries alice's.
    assert!(!w.h.app.egress_intercepts(&before, HOST, 443).await);
    assert!(!leaks(&w, &before, HOST).await);
    // It carries bob's own instead, on his host.
    assert!(w.h.app.egress_intercepts(&before, BOB_HOST, 443).await);
    let bobs = forwarded(&w, &before, BOB_HOST).await.unwrap_or_default();
    assert!(
        bobs.iter()
            .any(|(n, v)| n == "authorization" && v == &format!("Bearer {BOBS}")),
        "{bobs:?}"
    );
    let after = client(&w, ALICE_TASK).await?;
    assert!(!w.h.app.egress_intercepts(&after, HOST, 443).await);
    assert!(!leaks(&w, &after, HOST).await);
    assert!(w.h.app.egress_intercepts(&after, BOB_HOST, 443).await);
    Ok(())
}

/// SPEC-248 FUP-004: a deleted task's proxy client, resolved before the deletion, resolves to no
/// person: its owner's personal secret no longer applies, the project's still does.
#[tokio::test]
async fn a_deleted_tasks_client_resolves_to_no_person() -> TestResult {
    let w = world().await?;
    add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    add(
        &w,
        &w.alice,
        "s1",
        secret(
            "sandbox",
            "SHARED_TOKEN",
            "project-token-5ha7ed",
            "api.shared.test",
        ),
    )
    .await?;
    let before = client(&w, ALICE_TASK).await?;
    assert!(w.h.app.egress_intercepts(&before, HOST, 443).await);
    assert!(w.h.store.delete_task(&TaskId::parse("t1")?).await?);
    assert!(!w.h.app.egress_intercepts(&before, HOST, 443).await);
    assert!(!leaks(&w, &before, HOST).await);
    assert!(
        w.h.app
            .egress_intercepts(&before, "api.shared.test", 443)
            .await
    );
    Ok(())
}

/// SPEC-248 FUP-002: a hand-over rewrites the run's fingerprint file (the Bash wrapper's fallback)
/// for the new owner, in place, so the run holds no hashes of the previous owner's values.
#[tokio::test]
async fn a_hand_over_rewrites_the_runs_fingerprint_file() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;
    let w = world().await?;
    add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    add(
        &w,
        &w.bob,
        "s1",
        secret("personal", "BOB_TOKEN", BOBS, BOB_HOST),
    )
    .await?;
    let run = tempfile::tempdir()?;
    seed_task(&w.h, "t5", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt_at(
        &w.h,
        "t5",
        "scopes-task-token-alice-run-0123456789ab",
        &run.path().to_string_lossy(),
    )
    .await?;
    // The file as the launch wrote it, for alice.
    let file = run.path().join(yulius_app::vault::FINGERPRINTS_FILE);
    let alice_id = UserId::parse("u-alice")?;
    let s1 = SandboxId::parse("s1")?;
    let launched =
        w.h.app
            .scrub_fingerprints_for(&s1, Holder::Person(&alice_id))
            .await?;
    assert!(launched.names.iter().any(|n| n == "MY_TOKEN"));
    std::fs::write(&file, serde_json::to_vec(&launched)?)?;
    let inode = std::fs::metadata(&file)?.ino();

    let asked = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t5/control-request",
        None,
        &w.bob,
    )
    .await?;
    assert!(asked.status.is_success(), "{}", asked.body);
    let passed = call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t5/control-request/accept",
        None,
        &w.alice,
    )
    .await?;
    assert!(passed.status.is_success(), "{}", passed.body);

    let rewritten: yulius_contracts::scrub::FingerprintSet =
        serde_json::from_slice(&std::fs::read(&file)?)?;
    assert!(
        !rewritten.names.iter().any(|n| n == "MY_TOKEN"),
        "{:?}",
        rewritten.names
    );
    assert!(
        rewritten.names.iter().any(|n| n == "BOB_TOKEN"),
        "{:?}",
        rewritten.names
    );
    // In place: a contained session binds the file read-only by inode.
    assert_eq!(std::fs::metadata(&file)?.ino(), inode);
    Ok(())
}

/// SPEC-248 FUP-002 (review round 2): a rewrite that comes after control passed again — the
/// first of two quick hand-overs' — writes the fingerprints of the task's current owner, never
/// the middle owner's.
#[tokio::test]
async fn a_rewrite_follows_the_tasks_current_owner() -> TestResult {
    let w = world().await?;
    let carol = seed_github_user(&w.h, "u-carol", "carol").await?;
    w.h.github.set_perm("carol", "acme/app", perms("write"));
    add(
        &w,
        &w.bob,
        "s1",
        secret("personal", "BOB_TOKEN", BOBS, BOB_HOST),
    )
    .await?;
    add(
        &w,
        &carol,
        "s1",
        secret(
            "personal",
            "CAROL_TOKEN",
            "carol-personal-token-ca401",
            "api.carol.test",
        ),
    )
    .await?;
    let run = tempfile::tempdir()?;
    seed_task(&w.h, "t5", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt_at(
        &w.h,
        "t5",
        "scopes-task-token-alice-run-0123456789ab",
        &run.path().to_string_lossy(),
    )
    .await?;
    let file = run.path().join(yulius_app::vault::FINGERPRINTS_FILE);
    std::fs::write(&file, b"{}")?;
    let t5 = TaskId::parse("t5")?;
    let (alice, bob, carol_id) = (
        UserId::parse("u-alice")?,
        UserId::parse("u-bob")?,
        UserId::parse("u-carol")?,
    );
    assert!(
        w.h.store
            .take_control(&t5, &alice, &bob, w.h.app.now())
            .await?
    );
    assert!(
        w.h.store
            .take_control(&t5, &bob, &carol_id, w.h.app.now())
            .await?
    );
    // The first hand-over's rewrite comes last.
    w.h.app.rewrite_run_fingerprints(&t5).await?;
    let set: yulius_contracts::scrub::FingerprintSet =
        serde_json::from_slice(&std::fs::read(&file)?)?;
    assert!(
        set.names.iter().any(|n| n == "CAROL_TOKEN"),
        "{:?}",
        set.names
    );
    assert!(
        !set.names.iter().any(|n| n == "BOB_TOKEN"),
        "{:?}",
        set.names
    );
    Ok(())
}

/// Review of SPEC-245, M3: switching a secret on respects the limit of secrets reaching a project.
#[tokio::test]
async fn switching_on_respects_the_projects_limit() -> TestResult {
    use yulius_app::ports::vault::VaultStore as _;
    let w = world().await?;
    let id = add(
        &w,
        &w.alice,
        "s1",
        secret("personal", "MY_TOKEN", MINE, HOST),
    )
    .await?;
    let template = w.h.store.vault_get(&id).await?.ok_or("secret")?;
    for n in 0..200 {
        let mut filler = template.clone();
        filler.id = format!("filler-{n}");
        filler.owner_id = None;
        filler.sandbox_id = Some(SandboxId::parse("s2")?);
        filler.name = format!("FILLER_{n}");
        w.h.store.vault_insert(&filler).await?;
    }
    let refused = switch(&w, &w.alice, "s2", &id, true).await?;
    assert!(!refused.status.is_success(), "{}", refused.body);
    assert!(
        refused.body.to_string().contains("at most"),
        "{}",
        refused.body
    );
    Ok(())
}

/// SPEC-248 FUP-003: concurrent switch-ons cannot pass the limit: the store counts the secrets
/// reaching the project in the transaction that switches one on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_switch_ons_stop_at_the_projects_limit() -> TestResult {
    use yulius_app::ports::vault::VaultStore as _;
    let w = world().await?;
    let mut ids = Vec::new();
    for n in 0..20 {
        let body = secret("personal", &format!("MINE_{n}"), MINE, HOST);
        ids.push(add(&w, &w.alice, "s1", body).await?);
    }
    let template = w.h.store.vault_get(&ids[0]).await?.ok_or("secret")?;
    let s2 = SandboxId::parse("s2")?;
    for n in 0..199 {
        let mut filler = template.clone();
        filler.id = format!("filler-{n}");
        filler.owner_id = None;
        filler.sandbox_id = Some(s2.clone());
        filler.name = format!("FILLER_{n}");
        w.h.store.vault_insert(&filler).await?;
    }
    // Each switch-on on its own worker, all at once.
    let switches = ids.iter().map(|id| {
        let (router, alice) = (w.h.router.clone(), w.alice.clone());
        let path = format!("/api/sandboxes/s2/agent-secrets/{id}/enabled");
        tokio::spawn(async move {
            call(
                &router,
                Method::PUT,
                &path,
                Some(json!({"enabled": true})),
                &alice,
            )
            .await
            .map(|r| (r.status, r.body))
            .map_err(|e| e.to_string())
        })
    });
    let switched = futures::future::join_all(switches).await;
    let mut on = 0;
    for result in switched {
        let (status, body) = result??;
        if status.is_success() {
            on += 1;
        } else {
            assert!(body.to_string().contains("at most"), "{body}");
        }
    }
    assert_eq!(on, 1);
    assert_eq!(w.h.store.vault_agent_in(&s2).await?.len(), 200);
    Ok(())
}
