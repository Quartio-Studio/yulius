//! Prepared tasks (SPEC-105, SPEC-001 TSK-019): a task written now and started later, when a
//! person presses Start now ("when I decide") or by itself at its time. These tests use the
//! in-memory harness, where a start fails at once (no Git, no session host): a task that was
//! released leaves `prepared` and `queued`.

use crate::support;

use std::sync::atomic::Ordering;

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
const HOUR: i64 = 3_600_000;

/// Sandbox `s1` (studio) where alice may write; her task `t-agent` runs (task token
/// `tok-agent`). Bob may read only.
async fn world() -> Result<(Harness, Creds, Creds), Error> {
    let h = harness_configured(|c| c.max_live_sessions = 0)?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    let bob = user_with_token(&h, "u-bob", "bob", false).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    seed_task(&h, "t-agent", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t-agent", "tok-agent").await?;
    Ok((h, alice, bob))
}

fn rfc3339(millis: i64) -> String {
    yulius_app::auth::rfc3339(yulius_domain::time::Timestamp(millis))
}

fn now(h: &Harness) -> i64 {
    h.clock.0.load(Ordering::SeqCst)
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

async fn created(h: &Harness, creds: &Creds, body: Value) -> Result<String, Error> {
    let r = post(h, creds, body).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    Ok(r.body["id"].as_str().ok_or("id")?.to_owned())
}

async fn status(h: &Harness, id: &str) -> Result<TaskStatus, Error> {
    Ok(h.store
        .task(&TaskId::parse(id)?)
        .await?
        .ok_or("task")?
        .status)
}

/// AC-PRP-1, AC-PRP-5: New task and `task_create` make a prepared task, with or without a time;
/// a time outside the window, a prepared task that also runs after a task, a rule or as a
/// supertask are refused and create nothing.
#[tokio::test]
async fn a_task_is_created_prepared_or_scheduled() -> TestResult {
    let (h, alice, _) = world().await?;
    let r = post(
        &h,
        &alice,
        json!({"prompt": "Tidy the docs", "prepared": true}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "prepared", "{:?}", r.body);
    assert_eq!(r.body["start_at"], Value::Null, "{:?}", r.body);
    let id = r.body["id"].as_str().ok_or("id")?.to_owned();
    // No run started, and none starts on the queue's or the chains' pass.
    h.app.drain_queue().await;
    h.app.advance_all_chains().await;
    h.app.release_due_prepared().await;
    assert_eq!(status(&h, &id).await?, TaskStatus::Prepared);
    assert!(h.store.attempts(&TaskId::parse(&id)?).await?.is_empty());

    // A time implies prepared.
    let at = now(&h) + HOUR;
    let r = post(
        &h,
        &alice,
        json!({"prompt": "Nightly sweep", "start_at": rfc3339(at)}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["status"], "prepared");
    assert_eq!(r.body["start_at"], json!(rfc3339(at)));
    let scheduled = r.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(
        summary(&h, &alice, &scheduled).await?["start_at"],
        json!(rfc3339(at))
    );

    let before = h.store.tasks(&SandboxId::parse("s1")?, false).await?.len();
    for (body, says) in [
        (
            json!({"prompt": "x", "start_at": rfc3339(now(&h) - 2 * 60_000)}),
            "passed",
        ),
        (
            json!({"prompt": "x", "start_at": rfc3339(now(&h) + 367 * 24 * HOUR)}),
            "366 days",
        ),
        (json!({"prompt": "x", "start_at": "tomorrow"}), "RFC 3339"),
        (
            json!({"prompt": "x", "prepared": true, "after_task_id": "t-agent"}),
            "prepared",
        ),
        (
            json!({"prompt": "x", "prepared": true, "start_rule": "#1 done"}),
            "prepared",
        ),
        (
            json!({"prompt": "x", "start_at": rfc3339(at), "supertask": true}),
            "prepared",
        ),
    ] {
        let r = post(&h, &alice, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {:?}",
            r.body
        );
        assert!(r.body.to_string().contains(says), "{body}: {:?}", r.body);
    }
    assert_eq!(
        h.store.tasks(&SandboxId::parse("s1")?, false).await?.len(),
        before
    );

    // A time within the last minute is accepted: it starts on the next tick.
    let r = post(
        &h,
        &alice,
        json!({"prompt": "Now-ish", "start_at": rfc3339(now(&h) - 30_000)}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);

    // An agent prepares one for its creator.
    let result = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "Follow-up for later", "prepared": true}),
    )
    .await?;
    let text = tool_text(&result);
    assert_eq!(result["isError"], json!(false), "{text}");
    let id = text["task"]["task_id"].as_str().ok_or("task_id")?;
    assert_eq!(status(&h, id).await?, TaskStatus::Prepared);
    let result = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "Later", "start_at": rfc3339(at)}),
    )
    .await?;
    let text = tool_text(&result);
    assert_eq!(result["isError"], json!(false), "{text}");
    let id = TaskId::parse(text["task"]["task_id"].as_str().ok_or("task_id")?)?;
    assert_eq!(
        h.store.task(&id).await?.ok_or("task")?.status,
        TaskStatus::Prepared
    );
    let refused = mcp_tool(
        &h,
        "tok-agent",
        "task_create",
        json!({"prompt": "x", "prepared": "yes"}),
    )
    .await?;
    assert_eq!(refused["isError"], json!(true), "{refused}");
    Ok(())
}

/// PRP-001: at most 32 prepared tasks wait in one sandbox.
#[tokio::test]
async fn a_sandbox_holds_a_bounded_number_of_prepared_tasks() -> TestResult {
    let (h, alice, _) = world().await?;
    for n in 0..32 {
        created(
            &h,
            &alice,
            json!({"prompt": format!("later {n}"), "prepared": true}),
        )
        .await?;
    }
    let r = post(
        &h,
        &alice,
        json!({"prompt": "one too many", "prepared": true}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", r.body);
    Ok(())
}
