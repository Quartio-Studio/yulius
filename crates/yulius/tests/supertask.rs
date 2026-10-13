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
use yulius_app::model::TaskRecord;
use yulius_app::ports::store::{AttemptUpdate, TaskStore};
use yulius_app::supervisor::RunStart;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};

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

async fn create(
    h: &Harness,
    creds: &Creds,
    sandbox: &str,
    body: Value,
) -> Result<TaskRecord, Error> {
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/tasks"),
        Some(body),
        creds,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = TaskId::parse(r.body["id"].as_str().ok_or("id")?)?;
    Ok(h.store.task(&id).await?.ok_or("task")?)
}

/// Offers `task` a session slot as the queue does; the in-memory harness cannot launch, so a
/// task the hold lets through leaves `queued` (it fails to start).
async fn offer(h: &Harness, task: &TaskRecord) -> Result<TaskStatus, Error> {
    let _ = h
        .app
        .start_run(&task.id, RunStart::Fresh { prompt: None }, None)
        .await;
    Ok(h.store.task(&task.id).await?.ok_or("task")?.status)
}

async fn held_by(h: &Harness, creds: &Creds, task: &TaskRecord) -> Result<Value, Error> {
    Ok(summary(h, creds, task.id.as_str()).await?["held_by"].clone())
}

/// AC-SUP-2..4 (the rule, SUP-002, SUP-003, SUP-005): a supertask waits for the project's
/// working task; every other start of the project waits for the supertask (a new task, a
/// continued one, Start now on a chained one); another project is not held; once the working
/// task is Done the supertask goes, and once it ended the others go.
#[tokio::test]
async fn a_supertask_waits_for_working_tasks_and_holds_its_project() -> TestResult {
    let (h, alice) = world(8).await?;
    add_sandbox(&h, "s2", "other", 20100).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/other", perms("write"));
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let closed = seed_task(&h, "t-closed", "s1", "u-alice", TaskStatus::Closed, None).await?;

    let s = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Harden", "supertask": true}),
    )
    .await?;
    assert_eq!(
        offer(&h, &s).await?,
        TaskStatus::Queued,
        "waits for #{}",
        agent.number
    );
    assert_eq!(held_by(&h, &alice, &s).await?, json!([agent.number]));

    let b = create(&h, &alice, "s1", json!({"prompt": "Small fix"})).await?;
    assert_eq!(
        offer(&h, &b).await?,
        TaskStatus::Queued,
        "waits for the supertask"
    );
    assert_eq!(held_by(&h, &alice, &b).await?, json!([s.number]));

    // Continue of an ended task and Start now of a chained one wait too.
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{}/continue", closed.id),
        Some(json!({})),
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    assert_eq!(r.body["status"], "queued", "{:?}", r.body);
    assert_eq!(held_by(&h, &alice, &closed).await?, json!([s.number]));
    let chained = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "After the agent", "after_task_id": "t-agent"}),
    )
    .await?;
    let r = call(
        &h.router,
        Method::POST,
        &format!("/api/tasks/{}/start-now", chained.id),
        None,
        &alice,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    assert_eq!(
        summary(&h, &alice, chained.id.as_str()).await?["status"],
        "queued"
    );
    assert_eq!(held_by(&h, &alice, &chained).await?, json!([s.number]));

    // Another project is not held.
    let other = create(&h, &alice, "s2", json!({"prompt": "Elsewhere"})).await?;
    assert_ne!(offer(&h, &other).await?, TaskStatus::Queued);
    assert_eq!(held_by(&h, &alice, &other).await?, json!([]));

    // The agent is Done: the supertask goes (a Done task holds nothing), the others still wait.
    h.store
        .set_activity(&agent.id, Some(Activity::Done), h.app.now())
        .await?;
    assert_eq!(held_by(&h, &alice, &b).await?, json!([s.number]));
    assert_ne!(
        offer(&h, &s).await?,
        TaskStatus::Queued,
        "the supertask starts"
    );
    // It ended (here: it could not start): the hold is over.
    assert_ne!(offer(&h, &b).await?, TaskStatus::Queued, "the others start");
    Ok(())
}

/// SUP-003: supertasks of one project go one at a time, by task number; a supertask without
/// working tasks starts at once.
#[tokio::test]
async fn supertasks_go_one_at_a_time() -> TestResult {
    let (h, alice) = world(8).await?;
    let agent = h
        .store
        .task(&TaskId::parse("t-agent")?)
        .await?
        .ok_or("agent")?;
    let first = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "One", "supertask": true}),
    )
    .await?;
    let second = create(
        &h,
        &alice,
        "s1",
        json!({"prompt": "Two", "supertask": true}),
    )
    .await?;
    assert_eq!(held_by(&h, &alice, &first).await?, json!([agent.number]));
    assert_eq!(
        held_by(&h, &alice, &second).await?,
        json!([agent.number, first.number])
    );
    // The agent's run ends.
    h.store
        .transition_task(
            &agent.id,
            None,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled),
            None,
            h.app.now(),
        )
        .await?;
    end_attempt(&h, "t-agent").await?;
    assert_eq!(held_by(&h, &alice, &first).await?, json!([]));
    assert_eq!(held_by(&h, &alice, &second).await?, json!([first.number]));
    assert_eq!(offer(&h, &second).await?, TaskStatus::Queued);
    assert_ne!(
        offer(&h, &first).await?,
        TaskStatus::Queued,
        "no working task: it starts"
    );
    Ok(())
}

async fn end_attempt(h: &Harness, task: &str) -> TestResult {
    h.store
        .transition_attempt(
            &AttemptId::parse(&format!("a-{task}"))?,
            AttemptState::Exited,
            &AttemptUpdate {
                ended_at: Some(h.app.now()),
                ..AttemptUpdate::default()
            },
        )
        .await?;
    Ok(())
}
