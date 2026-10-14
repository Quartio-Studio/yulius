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
use yulius_app::ports::store::{SandboxStore, TaskStore};
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

async fn act(
    h: &Harness,
    creds: &Creds,
    method: Method,
    path: String,
    body: Option<Value>,
) -> Result<support::Resp, Error> {
    call(&h.router, method, &path, body, creds).await
}

/// AC-PRP-2: Start now starts a prepared task whatever its time, Cancel ends one, Delete removes
/// one; Start now on a task that already left `prepared` is 409; someone who may only read
/// changes nothing.
#[tokio::test]
async fn a_person_starts_cancels_or_deletes_a_prepared_task() -> TestResult {
    let (h, alice, bob) = world().await?;
    let later = created(
        &h,
        &alice,
        json!({"prompt": "Later", "start_at": rfc3339(now(&h) + HOUR)}),
    )
    .await?;
    let cancel = created(
        &h,
        &alice,
        json!({"prompt": "Not needed", "prepared": true}),
    )
    .await?;
    let gone = created(&h, &alice, json!({"prompt": "Delete me", "prepared": true})).await?;

    for path in [
        format!("/api/tasks/{later}/start-now"),
        format!("/api/tasks/{cancel}/stop"),
    ] {
        let r = act(&h, &bob, Method::POST, path.clone(), None).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {:?}", r.body);
    }
    assert_eq!(status(&h, &later).await?, TaskStatus::Prepared);

    let r = act(
        &h,
        &alice,
        Method::POST,
        format!("/api/tasks/{later}/start-now"),
        None,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_ne!(r.body["status"], "prepared", "{:?}", r.body);
    assert_eq!(r.body["start_at"], Value::Null, "{:?}", r.body);
    assert_ne!(status(&h, &later).await?, TaskStatus::Prepared);
    let again = act(
        &h,
        &alice,
        Method::POST,
        format!("/api/tasks/{later}/start-now"),
        None,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);

    let r = act(
        &h,
        &alice,
        Method::POST,
        format!("/api/tasks/{cancel}/stop"),
        None,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "stopped_cancelled", "{:?}", r.body);

    let r = act(
        &h,
        &alice,
        Method::DELETE,
        format!("/api/tasks/{gone}"),
        None,
    )
    .await?;
    assert!(r.status.is_success(), "{:?}", r.body);
    assert!(h.store.task(&TaskId::parse(&gone)?).await?.is_none());
    Ok(())
}

/// AC-PRP-3: a scheduled task stays prepared before its time and starts on the tick past it; a
/// prepared task without a time never starts by itself; while onboarding blocks the sandbox's
/// tasks a due task waits, and Start now says why.
#[tokio::test]
async fn a_scheduled_task_starts_by_itself_at_its_time() -> TestResult {
    let (h, alice, _) = world().await?;
    let at = now(&h) + HOUR;
    let scheduled = created(
        &h,
        &alice,
        json!({"prompt": "At ten", "start_at": rfc3339(at)}),
    )
    .await?;
    let manual = created(
        &h,
        &alice,
        json!({"prompt": "When I decide", "prepared": true}),
    )
    .await?;

    h.clock.0.store(at - 1_000, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    assert_eq!(status(&h, &scheduled).await?, TaskStatus::Prepared);

    h.clock.0.store(at, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    assert_ne!(status(&h, &scheduled).await?, TaskStatus::Prepared);
    assert_eq!(
        summary(&h, &alice, &scheduled).await?["start_at"],
        Value::Null
    );

    h.clock.0.store(at + 400 * 24 * HOUR, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    assert_eq!(status(&h, &manual).await?, TaskStatus::Prepared);

    // Onboarding blocks the sandbox: a due task waits for it.
    let soon = now(&h) + 60_000;
    let waiting = created(
        &h,
        &alice,
        json!({"prompt": "After onboarding", "start_at": rfc3339(soon)}),
    )
    .await?;
    let sid = SandboxId::parse("s1")?;
    h.store.set_onboarding(&sid, "pending", None).await?;
    h.clock.0.store(soon + 1_000, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    assert_eq!(status(&h, &waiting).await?, TaskStatus::Prepared);
    let r = act(
        &h,
        &alice,
        Method::POST,
        format!("/api/tasks/{manual}/start-now"),
        None,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    assert_eq!(status(&h, &manual).await?, TaskStatus::Prepared);
    h.store.set_onboarding(&sid, "ready", None).await?;
    h.app.release_due_prepared().await;
    assert_ne!(status(&h, &waiting).await?, TaskStatus::Prepared);
    Ok(())
}

/// AC-PRP-4: the schedule moves, clears to "when I decide", starts a task at once when the new
/// time is due, refuses a time outside the window and a task that is not prepared.
#[tokio::test]
async fn a_prepared_task_is_rescheduled() -> TestResult {
    let (h, alice, bob) = world().await?;
    let id = created(&h, &alice, json!({"prompt": "Move me", "prepared": true})).await?;
    let path = format!("/api/tasks/{id}/schedule");
    let at = now(&h) + 2 * HOUR;

    let r = act(
        &h,
        &bob,
        Method::PUT,
        path.clone(),
        Some(json!({"start_at": rfc3339(at)})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);

    let r = act(
        &h,
        &alice,
        Method::PUT,
        path.clone(),
        Some(json!({"start_at": rfc3339(at)})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["start_at"], json!(rfc3339(at)), "{:?}", r.body);
    assert_eq!(r.body["status"], "prepared");

    let r = act(
        &h,
        &alice,
        Method::PUT,
        path.clone(),
        Some(json!({"start_at": rfc3339(now(&h) - 5 * 60_000)})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    assert_eq!(
        summary(&h, &alice, &id).await?["start_at"],
        json!(rfc3339(at))
    );

    let r = act(
        &h,
        &alice,
        Method::PUT,
        path.clone(),
        Some(json!({"start_at": null})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["start_at"], Value::Null, "{:?}", r.body);
    h.clock.0.store(at + HOUR, Ordering::SeqCst);
    h.app.release_due_prepared().await;
    assert_eq!(status(&h, &id).await?, TaskStatus::Prepared);

    // A time that is already due starts it on the call.
    let r = act(
        &h,
        &alice,
        Method::PUT,
        path.clone(),
        Some(json!({"start_at": rfc3339(now(&h))})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_ne!(status(&h, &id).await?, TaskStatus::Prepared);
    let r = act(
        &h,
        &alice,
        Method::PUT,
        path,
        Some(json!({"start_at": rfc3339(now(&h) + HOUR)})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "{:?}", r.body);
    Ok(())
}

/// Review of SPEC-105: a request starts only its own task when its time is due; another
/// sandbox-wide due task waits for the tick, and a second writer who does not own the task
/// (SPEC-070) changes nothing.
#[tokio::test]
async fn a_request_releases_only_its_own_task_and_only_for_its_owner() -> TestResult {
    let (h, alice, _) = world().await?;
    let carol = user_with_token(&h, "u-carol", "carol", false).await?;
    h.github
        .set_perm("carol", "Quartio-Studio/studio", perms("write"));
    let at = now(&h) + HOUR;
    let waiting = created(
        &h,
        &alice,
        json!({"prompt": "At its time", "start_at": rfc3339(at)}),
    )
    .await?;
    h.clock.0.store(at + 1_000, Ordering::SeqCst);
    // Creating another prepared task does not start the due one: that is the tick's job.
    let other = created(&h, &alice, json!({"prompt": "Later", "prepared": true})).await?;
    assert_eq!(status(&h, &waiting).await?, TaskStatus::Prepared);
    // Nor does rescheduling another task to now: it starts that one only.
    let r = act(
        &h,
        &alice,
        Method::PUT,
        format!("/api/tasks/{other}/schedule"),
        Some(json!({"start_at": rfc3339(now(&h))})),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_ne!(status(&h, &other).await?, TaskStatus::Prepared);
    assert_eq!(status(&h, &waiting).await?, TaskStatus::Prepared);

    for (method, path, body) in [
        (
            Method::POST,
            format!("/api/tasks/{waiting}/start-now"),
            None,
        ),
        (Method::POST, format!("/api/tasks/{waiting}/stop"), None),
        (
            Method::PUT,
            format!("/api/tasks/{waiting}/schedule"),
            Some(json!({"start_at": null})),
        ),
    ] {
        let r = act(&h, &carol, method, path.clone(), body).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {:?}", r.body);
    }
    assert_eq!(status(&h, &waiting).await?, TaskStatus::Prepared);
    h.app.release_due_prepared().await;
    assert_ne!(status(&h, &waiting).await?, TaskStatus::Prepared);
    Ok(())
}
