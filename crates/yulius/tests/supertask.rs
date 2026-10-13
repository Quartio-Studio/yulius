//! Supertasks (SPEC-071, SPEC-001 TSK-018): a task that waits for the running work of its
//! project, then runs alone while every other task of the project waits for it. The scenario
//! with real processes is in `supervision.rs`; these tests use the in-memory harness, where a
//! start that passes the hold fails at once (no Git, no session host), so a task the hold keeps
//! stays `queued`.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, add_sandbox, call, harness_configured, mcp_tool, perms,
    seed_live_attempt, seed_task, tool_text, user_with_token,
};
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;

type Error = Box<dyn std::error::Error>;

const CREATE: &str = "/api/sandboxes/s1/tasks";

/// Sandbox `s1` (studio) where alice may write; her task `t-agent` runs (a live attempt, task
/// token `tok-agent`).
async fn world(max_live_sessions: u32) -> Result<(Harness, Creds), Error> {
    let h = harness_configured(move |c| c.max_live_sessions = max_live_sessions)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    Ok((h, alice))
}

async fn post(h: &Harness, creds: &Creds, body: Value) -> Result<support::Resp, Error> {
    call(&h.router, Method::POST, CREATE, Some(body), creds).await
}

async fn summary(h: &Harness, creds: &Creds, id: &str) -> Result<Value, Error> {
    Ok(call(
        &h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        creds,
    )
    .await?
    .body)
}

/// AC-SUP-1: New task and `task_create` make a supertask; it cannot also run after a task or a
/// rule (400-class refusal, nothing created).
#[tokio::test]
async fn a_task_is_created_as_a_supertask() -> TestResult {
    let (h, alice) = world(0).await?;
    let r = post(
        &h,
        &alice,
        json!({"prompt": "Harden the whole app", "supertask": true}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["supertask"], json!(true), "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(summary(&h, &alice, &id).await?["supertask"], json!(true));
    let stored = h.store.task(&TaskId::parse(&id)?).await?.ok_or("task")?;
    assert!(stored.supertask);

    // A plain task is not one.
    let r = post(&h, &alice, json!({"prompt": "Small fix"})).await?;
    assert_eq!(r.body["supertask"], json!(false), "{:?}", r.body);

    // Not together with a chain or a rule.
    let before = h.store.tasks(&SandboxId::parse("s1")?, false).await?.len();
    for body in [
        json!({"prompt": "x", "supertask": true, "after_task_id": "t-agent"}),
        json!({"prompt": "x", "supertask": true, "start_rule": "#1 done"}),
    ] {
        let r = post(&h, &alice, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {:?}",
            r.body
        );
        assert!(
            r.body.to_string().contains("supertask"),
            "{body}: {:?}",
            r.body
        );
    }
    assert_eq!(
        h.store.tasks(&SandboxId::parse("s1")?, false).await?.len(),
        before
    );

    // An agent creates one for its creator.
    let result = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "Security hardening", "supertask": true}),
    )
    .await?;
    let created = tool_text(&result);
    assert_eq!(result["isError"], json!(false), "{created}");
    let id = TaskId::parse(created["task"]["task_id"].as_str().ok_or("task_id")?)?;
    assert!(h.store.task(&id).await?.ok_or("task")?.supertask);
    let refused = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "x", "supertask": "yes"}),
    )
    .await?;
    assert_eq!(refused["isError"], json!(true), "{refused}");
    Ok(())
}
