//! Tasks hand work to another sandbox (SPEC-512 part 5, #1106, ADR-0395): the instance switch
//! (off by default, ask, automatic), a sandbox's opt-out, `sandbox_handoff`'s checks and bounds,
//! the owner's answer in a browser, provenance, the outcome told back and the audit.

use crate::support;

use std::net::SocketAddr;

use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, perms,
    seed_live_attempt, seed_sandbox, seed_task, setup_superadmin_creds,
};
use yulius_app::coordination::AgentContext;
use yulius_app::model::TaskOrigin;
use yulius_app::ports::store::{
    AccountStore as _, AuditStore as _, TaskStore as _, UserStore as _,
};
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

type Error = Box<dyn std::error::Error>;

struct World {
    h: Harness,
    internal: axum::Router,
    alice: Creds,
    bob: Creds,
}

/// Alice (an instance administrator) may write in `s1` (`app`), `s2` (`api`) and `s3` (`web`);
/// `s4` (`secret`) is not hers. Her task `t1` in `s1` runs with the token `tok-t1`.
async fn world() -> Result<World, Error> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 12, false).await?;
    let alice_id = h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    for (id, slug) in [
        ("s1", "app"),
        ("s2", "api"),
        ("s3", "web"),
        ("s4", "secret"),
    ] {
        seed_sandbox(&h, id, slug, alice_id.as_str()).await?;
    }
    for repo in ["acme/app", "acme/api", "acme/web"] {
        h.github.set_perm("alice", repo, perms("write"));
    }
    h.github.set_perm("bob", "acme/app", perms("write"));
    // A signed-in subscription, so tasks may start.
    h.store
        .insert_account(&yulius_app::model::AccountRecord {
            id: yulius_domain::ids::AccountId::parse("acc-ok")?,
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "Team Max".into(),
            credential: Some(
                h.app
                    .deps
                    .crypto
                    .seal(b"synthetic-subscription-credential")?,
            ),
            status: yulius_app::model::AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: h.app.now(),
            updated_at: h.app.now(),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    seed_task(&h, "t1", "s1", alice_id.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", "tok-t1").await?;
    let internal = yulius_server::internal_router(h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    Ok(World {
        h,
        internal,
        alice,
        bob,
    })
}

async fn agent(w: &World, task: &str) -> Result<AgentContext, Error> {
    let id = TaskId::parse(task)?;
    Ok(AgentContext {
        attempt: w.h.store.live_attempt(&id).await?.ok_or("no live run")?,
        task: w.h.store.task(&id).await?.ok_or("no task")?,
    })
}

async fn set_mode(w: &World, mode: &str, max_depth: u32, per_day: u32) -> TestResult {
    let r = call(
        &w.h.router,
        Method::PUT,
        "/api/admin/handoffs",
        Some(json!({"mode": mode, "max_depth": max_depth, "per_day": per_day})),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(())
}

async fn tool_names(w: &World, token: &str) -> Result<Vec<String>, Error> {
    let r = call(
        &w.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    Ok(r.body["result"]["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

async fn hand(w: &World, task: &str, args: Value) -> Result<Result<Value, String>, Error> {
    let ctx = agent(w, task).await?;
    Ok(w.h
        .app
        .tool_sandbox_handoff(&ctx, &args)
        .await
        .map_err(|e| e.public_message()))
}

async fn audits(w: &World, sandbox: &str) -> Result<Vec<String>, Error> {
    Ok(w.h.store.audit_details(sandbox, "task.handoff", 50).await?)
}

/// SPEC-512 HOF-001, HOF-003: off by default: no task agent is served `sandbox_handoff` and a
/// call is an unknown tool; an instance administrator turns it on; others may not.
#[tokio::test]
async fn handoffs_are_off_by_default_and_set_by_an_instance_administrator() -> TestResult {
    let w = world().await?;
    let view = call(
        &w.h.router,
        Method::GET,
        "/api/admin/handoffs",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(
        view.body,
        json!({"mode": "off", "max_depth": 3, "per_day": 20})
    );
    assert!(
        !tool_names(&w, "tok-t1")
            .await?
            .contains(&"sandbox_handoff".to_owned())
    );
    let refused = hand(
        &w,
        "t1",
        json!({"sandbox": "api", "title": "T", "prompt": "P"}),
    )
    .await?;
    assert!(refused.is_err_and(|e| e.contains("unknown tool")));
    let bob = call(
        &w.h.router,
        Method::PUT,
        "/api/admin/handoffs",
        Some(json!({"mode": "automatic", "max_depth": 3, "per_day": 20})),
        &w.bob,
    )
    .await?;
    assert_eq!(bob.status, StatusCode::FORBIDDEN);
    for bad in [
        json!({"mode": "sometimes", "max_depth": 3, "per_day": 20}),
        json!({"mode": "ask", "max_depth": 6, "per_day": 20}),
        json!({"mode": "ask", "max_depth": 3, "per_day": 0}),
    ] {
        let r = call(
            &w.h.router,
            Method::PUT,
            "/api/admin/handoffs",
            Some(bad.clone()),
            &w.alice,
        )
        .await?;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    set_mode(&w, "ask", 3, 20).await?;
    assert!(
        tool_names(&w, "tok-t1")
            .await?
            .contains(&"sandbox_handoff".to_owned())
    );
    let alice = w.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    assert_eq!(
        w.h.store
            .audit_details("handoffs", "instance.handoffs.set", 5)
            .await?
            .len(),
        1,
        "{alice}"
    );
    Ok(())
}

/// SPEC-512 HOF-003..HOF-006, automatic: the task is created at once as the owner, labelled
/// handed off, shows its source and is audited on both sandboxes; another sandbox only, one the
/// owner can work in (else not found), one that accepts, visible text only; an automation's task
/// hands nothing.
#[tokio::test]
async fn automatic_creates_the_task_as_the_owner_within_the_checks() -> TestResult {
    let w = world().await?;
    set_mode(&w, "automatic", 3, 20).await?;
    let made = hand(
        &w,
        "t1",
        json!({"sandbox": "acme/api", "title": "Add the endpoint", "prompt": "Add GET /v1/items"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    assert_eq!(made["state"], "created", "{made}");
    let number = made["task_number"].as_u64().ok_or("number")?;
    let tasks = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s2/tasks",
        None,
        &w.alice,
    )
    .await?;
    let task = tasks
        .body
        .as_array()
        .and_then(|t| t.iter().find(|t| t["number"] == number))
        .ok_or("the task")?
        .clone();
    assert_eq!(task["origin"], "handoff");
    assert_eq!(task["owner"]["login"], "alice");
    assert_eq!(task["title"], "Add the endpoint");
    let id = task["id"].as_str().ok_or("id")?;
    let from = call(
        &w.h.router,
        Method::GET,
        &format!("/api/tasks/{id}/handoffs"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(from.body["from"]["sandbox"], "app", "{}", from.body);
    assert_eq!(from.body["from"]["number"], 1);
    for sandbox in ["s1", "s2"] {
        let rows = audits(&w, sandbox).await?;
        assert!(
            rows.iter().any(|d| d.starts_with("created:")),
            "{sandbox}: {rows:?}"
        );
        assert!(
            rows.iter().all(|d| !d.contains("GET /v1/items")),
            "never the prompt: {rows:?}"
        );
    }

    let refusal = |r: Result<Value, String>| r.err().unwrap_or_default();
    assert!(
        refusal(
            hand(
                &w,
                "t1",
                json!({"sandbox": "app", "title": "T", "prompt": "P"})
            )
            .await?
        )
        .contains("another sandbox")
    );
    assert!(
        refusal(
            hand(
                &w,
                "t1",
                json!({"sandbox": "secret", "title": "T", "prompt": "P"})
            )
            .await?
        )
        .contains("no sandbox secret found")
    );
    assert!(
        refusal(
            hand(
                &w,
                "t1",
                json!({"sandbox": "api", "title": "T", "prompt": "P\u{200b}"})
            )
            .await?
        )
        .contains("cannot see"),
        "text with characters a person cannot see"
    );
    let closed = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s3/handoffs",
        Some(json!({"accept": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.body);
    assert!(
        refusal(
            hand(
                &w,
                "t1",
                json!({"sandbox": "web", "title": "T", "prompt": "P"})
            )
            .await?
        )
        .contains("does not accept")
    );
    assert!(
        audits(&w, "s1")
            .await?
            .iter()
            .any(|d| d.starts_with("refused:"))
    );

    // An automation's task holds nobody: it hands nothing over.
    let mut automated = w.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;
    automated.id = TaskId::parse("t-auto")?;
    automated.number = 99;
    automated.origin = TaskOrigin::Automation;
    w.h.store.insert_task(&automated).await?;
    seed_live_attempt(&w.h, "t-auto", "tok-auto").await?;
    assert!(
        refusal(
            hand(
                &w,
                "t-auto",
                json!({"sandbox": "api", "title": "T", "prompt": "P"})
            )
            .await?
        )
        .contains("automation")
    );
    Ok(())
}

/// SPEC-512 HOF-003: a chain goes at most `max_depth` deep, never back to one of its sandboxes,
/// and an owner's handoffs a day are bounded.
#[tokio::test]
async fn depth_cycles_and_the_daily_bound_are_refused() -> TestResult {
    let w = world().await?;
    set_mode(&w, "automatic", 2, 3).await?;
    let first = hand(
        &w,
        "t1",
        json!({"sandbox": "api", "title": "A", "prompt": "A"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    let a = task_id_of(&w, "s2", first["task_number"].as_u64().ok_or("n")?).await?;
    seed_live_attempt(&w.h, &a, "tok-a").await?;
    let cycle = hand(
        &w,
        &a,
        json!({"sandbox": "app", "title": "Back", "prompt": "Back"}),
    )
    .await?;
    assert!(cycle.is_err_and(|e| e.contains("cycle")));
    let second = hand(
        &w,
        &a,
        json!({"sandbox": "web", "title": "B", "prompt": "B"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    let b = task_id_of(&w, "s3", second["task_number"].as_u64().ok_or("n")?).await?;
    seed_live_attempt(&w.h, &b, "tok-b").await?;
    w.h.github.set_perm("alice", "acme/secret", perms("write"));
    let third = hand(
        &w,
        &b,
        json!({"sandbox": "secret", "title": "C", "prompt": "C"}),
    )
    .await?;
    assert!(
        third.is_err_and(|e| e.contains("at most 2 deep")),
        "a third level"
    );
    let daily = hand(
        &w,
        "t1",
        json!({"sandbox": "secret", "title": "D", "prompt": "D"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    assert_eq!(daily["state"], "created");
    let over = hand(
        &w,
        "t1",
        json!({"sandbox": "web", "title": "E", "prompt": "E"}),
    )
    .await?;
    assert!(
        over.is_err_and(|e| e.contains("24 hours")),
        "the fourth in a day, with 3 allowed"
    );
    Ok(())
}

async fn task_id_of(w: &World, sandbox: &str, number: u64) -> Result<String, Error> {
    let tasks = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        None,
        &w.alice,
    )
    .await?;
    Ok(tasks
        .body
        .as_array()
        .and_then(|t| t.iter().find(|t| t["number"] == number))
        .and_then(|t| t["id"].as_str())
        .ok_or("the task")?
        .to_owned())
}

/// SPEC-512 HOF-004, ask: the handoff is stored and the call returns; the owner sees it on the
/// task's page and answers it in a browser session only (not with a token, not another person);
/// approval creates the task with exactly the arguments shown; decline creates nothing.
#[tokio::test]
async fn ask_stores_a_request_the_owner_answers_in_a_browser() -> TestResult {
    let w = world().await?;
    set_mode(&w, "ask", 3, 20).await?;
    let asked = hand(&w, "t1", json!({"sandbox": "api", "title": "Add the endpoint", "prompt": "Add GET /v1/items", "mode": "prompt"})).await?
        .map_err(|e| e.to_string())?;
    assert_eq!(asked["state"], "requested", "{asked}");
    let hid = asked["id"].as_str().ok_or("id")?.to_owned();
    let page = call(
        &w.h.router,
        Method::GET,
        "/api/tasks/t1/handoffs",
        None,
        &w.alice,
    )
    .await?;
    let shown = &page.body["handoffs"][0];
    assert_eq!(
        (
            shown["state"].as_str(),
            shown["prompt"].as_str(),
            shown["target_sandbox"].as_str()
        ),
        (Some("requested"), Some("Add GET /v1/items"), Some("api")),
        "{}",
        page.body
    );
    let path = format!("/api/tasks/t1/handoffs/{hid}");

    let user = w.h.store.user_by_login("alice").await?.ok_or("alice")?;
    let token =
        w.h.app
            .issue_api_token(&user, "cli", yulius_domain::permission::TokenScope::Operate)
            .await?;
    let bearer = Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    };
    let by_token = call(
        &w.h.router,
        Method::POST,
        &path,
        Some(json!({"approve": true})),
        &bearer,
    )
    .await?;
    assert_eq!(by_token.status, StatusCode::FORBIDDEN, "{}", by_token.body);
    let by_bob = call(
        &w.h.router,
        Method::POST,
        &path,
        Some(json!({"approve": true})),
        &w.bob,
    )
    .await?;
    assert_eq!(by_bob.status, StatusCode::FORBIDDEN, "{}", by_bob.body);
    assert!(
        call(
            &w.h.router,
            Method::GET,
            "/api/sandboxes/s2/tasks",
            None,
            &w.alice
        )
        .await?
        .body
        .as_array()
        .is_some_and(Vec::is_empty)
    );

    let approved = call(
        &w.h.router,
        Method::POST,
        &path,
        Some(json!({"approve": true})),
        &w.alice,
    )
    .await?;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
    assert_eq!(approved.body["handoffs"][0]["state"], "created");
    let number = approved.body["handoffs"][0]["target_number"]
        .as_u64()
        .ok_or("number")?;
    let id = task_id_of(&w, "s2", number).await?;
    let task = w.h.store.task(&TaskId::parse(&id)?).await?.ok_or("task")?;
    assert_eq!(
        (task.title.as_str(), task.prompt.as_str()),
        ("Add the endpoint", "Add GET /v1/items")
    );
    assert_eq!(task.owner, user.id);
    let again = call(
        &w.h.router,
        Method::POST,
        &path,
        Some(json!({"approve": true})),
        &w.alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "answered once");

    let other = hand(
        &w,
        "t1",
        json!({"sandbox": "web", "title": "Other", "prompt": "Other"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    let declined = call(
        &w.h.router,
        Method::POST,
        &format!(
            "/api/tasks/t1/handoffs/{}",
            other["id"].as_str().unwrap_or_default()
        ),
        Some(json!({"approve": false})),
        &w.alice,
    )
    .await?;
    assert_eq!(declined.status, StatusCode::OK);
    assert!(
        call(
            &w.h.router,
            Method::GET,
            "/api/sandboxes/s3/tasks",
            None,
            &w.alice
        )
        .await?
        .body
        .as_array()
        .is_some_and(Vec::is_empty)
    );
    for (sandbox, what) in [("s1", "approved:"), ("s2", "created:"), ("s3", "declined:")] {
        assert!(
            audits(&w, sandbox)
                .await?
                .iter()
                .any(|d| d.starts_with(what)),
            "{sandbox} {what}"
        );
    }
    Ok(())
}

/// SPEC-512 HOF-005: a target's outcome waits until it ended; a source that ended is not told
/// (nothing to tell), once.
#[tokio::test]
async fn an_outcome_waits_for_the_targets_end() -> TestResult {
    let w = world().await?;
    set_mode(&w, "automatic", 3, 20).await?;
    let made = hand(
        &w,
        "t1",
        json!({"sandbox": "api", "title": "A", "prompt": "A"}),
    )
    .await?
    .map_err(|e| e.to_string())?;
    let target =
        TaskId::parse(&task_id_of(&w, "s2", made["task_number"].as_u64().ok_or("n")?).await?)?;
    assert_eq!(
        w.h.app.tell_handoff_outcomes().await,
        0,
        "the target has not ended"
    );
    w.h.store
        .transition_task(&target, None, TaskStatus::Closed, None, w.h.app.now())
        .await?;
    w.h.store
        .transition_task(
            &TaskId::parse("t1")?,
            None,
            TaskStatus::Closed,
            None,
            w.h.app.now(),
        )
        .await?;
    assert_eq!(
        w.h.app.tell_handoff_outcomes().await,
        0,
        "a closed source takes nothing"
    );
    use yulius_app::ports::store_ext::HandoffStore as _;
    assert!(
        w.h.store.untold_handoffs().await?.is_empty(),
        "and is not asked again"
    );
    Ok(())
}
