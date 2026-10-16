//! SPEC-191 PER: a person pauses a running task and resumes it later. Its run is frozen as an
//! update freezes it (ADR-0131), or held at its turn boundary when it cannot be frozen; the pause
//! is stored, survives an update and a restart, and every way Yulius ends the run thaws it first.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yulius_app::ports::session::RunFreeze;
use yulius_app::ports::store::TaskStore as _;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

use crate::support::updates::FreezingHost;
use crate::support::{
    Creds, Harness, HarnessOptions, call, harness_with, perms, seed_github_user,
    seed_live_attempt_at, seed_sandbox, seed_task,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = "pause-token-0123456789abcdefghijklmn";
const RUN: &str = "/nonexistent/runs/a-t1";

/// Alice owns the running task `t1` (live run `RUN`); Bob may write to the project too.
struct Fixture {
    h: Harness,
    host: Arc<FreezingHost>,
    alice: Creds,
    bob: Creds,
    task: TaskId,
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let host = FreezingHost::new();
    let h = harness_with(HarnessOptions {
        session_host: Some(host.clone()),
        configure: Some(Box::new(|c| c.person_freeze_pass_ms = 20)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-a", "alice").await?;
    let bob = seed_github_user(&h, "u-b", "bob").await?;
    seed_sandbox(&h, "s1", "repo", "u-a").await?;
    h.github.set_perm("alice", "acme/repo", perms("write"));
    h.github.set_perm("bob", "acme/repo", perms("write"));
    let task = seed_task(&h, "t1", "s1", "u-a", TaskStatus::Running, None).await?;
    seed_live_attempt_at(&h, "t1", TOKEN, RUN).await?;
    Ok(Fixture {
        h,
        host,
        alice,
        bob,
        task: task.id,
    })
}

async fn post(
    f: &Fixture,
    path: &str,
    who: &Creds,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let r = call(&f.h.router, Method::POST, path, None, who).await?;
    Ok((r.status, r.body))
}

/// The pause in `task`'s summary (null without one).
async fn pause_of(f: &Fixture) -> Result<Value, Box<dyn std::error::Error>> {
    let record = f.h.store.task(&f.task).await?.ok_or("task")?;
    let summary = f.h.app.task_summary(&record).await?;
    Ok(serde_json::to_value(summary)?["pause"].clone())
}

/// The agent's hook client posting `event`; returns the HTTP status (202 = held).
async fn hook(f: &Fixture, event: &str) -> Result<u16, Box<dyn std::error::Error>> {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/hooks/claude/{event}"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"hook_event_name": event, "session_id": "s"}).to_string(),
        ))?;
    Ok(f.h
        .internal
        .clone()
        .oneshot(request)
        .await?
        .status()
        .as_u16())
}

/// Waits (bounded) until `check` holds.
async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// AC-PER-1: the owner pauses the running task: its run is frozen, the pause stored and shown;
/// pausing again changes nothing; resume thaws it and clears the pause. Someone else, and a task
/// that is not running, are refused.
#[tokio::test]
async fn a_person_pauses_and_resumes_a_running_task() -> TestResult {
    let f = fixture().await?;
    let mut rx = f.h.app.events.subscribe();

    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.bob).await?;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "only the owner changes it: {body}"
    );
    assert!(f.host.frozen().is_empty());

    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "running", "a paused task keeps its run");
    assert_eq!(body["pause"]["by"], "person");
    assert_eq!(body["pause"]["user"]["login"], "alice");
    assert_eq!(body["pause"]["pending"], false, "frozen at once");
    assert!(body["pause"]["since"].is_string());
    assert_eq!(f.host.frozen(), vec![RUN.to_owned()]);
    let stored = f.h.store.person_pause(&f.task).await?.ok_or("stored")?;
    assert_eq!(stored.attempt.as_str(), "a-t1");
    let mut published = false;
    while let Ok(envelope) = rx.try_recv() {
        if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event {
            published |= task.id == "t1" && task.pause.is_some();
        }
    }
    assert!(published, "the lists hear of the pause");

    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "pausing again changes nothing: {body}"
    );
    assert_eq!(f.h.store.person_pause(&f.task).await?, Some(stored));

    let (status, body) = post(&f, "/api/tasks/t1/resume", &f.bob).await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = post(&f, "/api/tasks/t1/resume", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["pause"].is_null(), "{body}");
    assert!(f.host.frozen().is_empty());
    assert_eq!(f.host.thawed(), vec![RUN.to_owned()]);
    assert_eq!(f.h.store.person_pause(&f.task).await?, None);
    let (status, _) = post(&f, "/api/tasks/t1/resume", &f.alice).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "resuming a running task changes nothing"
    );
    assert_eq!(f.host.thawed().len(), 1);

    seed_task(&f.h, "t2", "s1", "u-a", TaskStatus::Queued, None).await?;
    let (status, body) = post(&f, "/api/tasks/t2/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.to_string()
            .contains("Only a running task can be paused"),
        "{body}"
    );
    Ok(())
}

/// AC-PER-3: a run that cannot be frozen pauses at its turn boundary: pending until its Stop
/// hook is held, and released by resume.
#[tokio::test]
async fn a_run_that_cannot_be_frozen_pauses_at_its_turn_boundary() -> TestResult {
    let f = fixture().await?;
    f.host.answer(RUN, RunFreeze::Unsupported);
    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pause"]["pending"], true, "its turn goes on");
    assert_eq!(hook(&f, "Stop").await?, 202, "held at its turn boundary");
    assert_eq!(pause_of(&f).await?["pending"], false, "the pause took hold");
    assert_eq!(hook(&f, "UserPromptSubmit").await?, 202);

    let (status, body) = post(&f, "/api/tasks/t1/resume", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(hook(&f, "Stop").await?, 200, "released");
    assert!(f.host.thawed().is_empty(), "nothing was frozen");
    Ok(())
}

/// PER-001: a freeze the kernel does not confirm is asked again, then thawed, and the run pauses
/// at its turn boundary instead.
#[tokio::test]
async fn a_freeze_that_never_settles_falls_back_to_the_turn_boundary() -> TestResult {
    let f = fixture().await?;
    f.host.answer(RUN, RunFreeze::Freezing);
    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pause"]["pending"], true, "not confirmed yet");
    let host = f.host.clone();
    assert!(
        eventually(|| host.thawed() == vec![RUN.to_owned()]).await,
        "thawed after its passes: {:?}",
        host.thawed()
    );
    assert_eq!(pause_of(&f).await?["pending"], true, "its turn goes on");
    assert_eq!(hook(&f, "Stop").await?, 202, "held at its turn boundary");
    assert_eq!(pause_of(&f).await?["pending"], false);
    Ok(())
}

/// AC-PER-4 (PER-004): stopping a paused task thaws its run before the agent is asked to end, and
/// clears the pause.
#[tokio::test]
async fn stopping_a_paused_task_thaws_it_first() -> TestResult {
    let f = fixture().await?;
    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post(&f, "/api/tasks/t1/stop", &f.alice).await?;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(
        f.host.thawed(),
        vec![RUN.to_owned()],
        "thawed before the stop"
    );
    assert_eq!(
        f.h.store.person_pause(&f.task).await?,
        None,
        "the pause is cleared"
    );
    Ok(())
}

/// AC-PER-4 (PER-003/004): a pause left on an older attempt is not shown and freezes nothing at
/// start; a pause of the live run is frozen again by a restarted server.
#[tokio::test]
async fn a_restart_freezes_the_live_run_s_pause_again_and_ignores_an_old_one() -> TestResult {
    use yulius_domain::ids::{AttemptId, UserId};
    let f = fixture().await?;
    let alice = UserId::parse("u-a")?;
    f.h.store
        .set_person_pause(&f.task, &alice, &AttemptId::parse("a-old")?, f.h.app.now())
        .await?;
    assert_eq!(pause_of(&f).await?, Value::Null, "an older run's pause");
    f.h.app.restore_person_pauses().await;
    assert!(f.host.frozen().is_empty(), "nothing frozen for it");

    f.h.store.clear_person_pause(&f.task, f.h.app.now()).await?;
    f.h.store
        .set_person_pause(&f.task, &alice, &AttemptId::parse("a-t1")?, f.h.app.now())
        .await?;
    assert_eq!(
        pause_of(&f).await?["pending"],
        true,
        "not frozen by this server yet"
    );
    f.h.app.restore_person_pauses().await;
    assert_eq!(f.host.frozen(), vec![RUN.to_owned()]);
    assert_eq!(pause_of(&f).await?["pending"], false);
    Ok(())
}

/// Review M1 (PER-001/002): a resume that lands while the pause's freeze is under way never
/// leaves the run frozen without a pause.
#[tokio::test]
async fn a_resume_during_the_pause_s_freeze_leaves_nothing_frozen() -> TestResult {
    let f = Arc::new(fixture().await?);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    *f.host.entered.lock().map_err(|_| "lock")? = Some(entered_tx);
    *f.host.gate.lock().map_err(|_| "lock")? = Some(gate);
    let pausing = {
        let f = Arc::clone(&f);
        tokio::spawn(async move {
            post(&f, "/api/tasks/t1/pause", &f.alice)
                .await
                .map(|r| r.0)
                .map_err(|e| e.to_string())
        })
    };
    tokio::task::spawn_blocking(move || entered.recv_timeout(Duration::from_secs(30))).await??;
    // The freeze is under way: the owner resumes.
    let resuming = {
        let f = Arc::clone(&f);
        tokio::spawn(async move {
            post(&f, "/api/tasks/t1/resume", &f.alice)
                .await
                .map(|r| r.0)
                .map_err(|e| e.to_string())
        })
    };
    // Serialized: the resume waits for the freeze (without the lock it would be done by now).
    let done_early = tokio::time::timeout(Duration::from_millis(200), async {
        while !resuming.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(done_early.is_err(), "the resume waits for the freeze");
    release.send(()).map_err(|_| "release")?;
    assert_eq!(pausing.await??, StatusCode::OK);
    assert_eq!(resuming.await??, StatusCode::OK);
    assert_eq!(f.h.store.person_pause(&f.task).await?, None);
    assert!(
        f.host.frozen().is_empty(),
        "left frozen: {:?}",
        f.host.frozen()
    );
    Ok(())
}

/// Review M2 (PER-005): a paused task is not switched, and the launch-goal watch does not count
/// the time it is paused.
#[tokio::test]
async fn a_paused_task_is_not_switched_and_its_launch_goal_waits() -> TestResult {
    let f = fixture().await?;
    assert!(!f.h.app.launch_goal_held(&f.task).await);
    let (status, _) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        f.h.app.launch_goal_held(&f.task).await,
        "paused time is not counted"
    );
    let r = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/switch",
        Some(json!({})),
        &f.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert!(
        r.body.to_string().contains("Resume the task first"),
        "{}",
        r.body
    );
    let (status, _) = post(&f, "/api/tasks/t1/resume", &f.alice).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(!f.h.app.launch_goal_held(&f.task).await);
    Ok(())
}

/// Review M2 (PER-004): closing a paused task (TSK-014) thaws its run first and clears the pause.
#[tokio::test]
async fn closing_a_paused_task_thaws_it_first() -> TestResult {
    let f = fixture().await?;
    let (status, _) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK);
    let _ = f.h.app.close_task_now(&f.task).await;
    assert_eq!(f.host.thawed(), vec![RUN.to_owned()]);
    assert_eq!(f.h.store.person_pause(&f.task).await?, None);
    Ok(())
}

/// Review L4 (PER-001): a run that cannot be frozen and waits at its prompt is paused already:
/// it makes no hook call until someone types, which the hold then catches.
#[tokio::test]
async fn an_idle_run_that_cannot_be_frozen_is_paused_at_once() -> TestResult {
    let f = fixture().await?;
    f.host.answer(RUN, RunFreeze::Unsupported);
    f.h.store
        .set_activity(
            &f.task,
            Some(yulius_domain::task::Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    let (status, body) = post(&f, "/api/tasks/t1/pause", &f.alice).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pause"]["pending"], false, "{body}");
    Ok(())
}
