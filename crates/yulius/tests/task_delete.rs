//! Permanent task deletion (SPEC-001 TSK-009, AC-TSK-8): write users delete resting tasks with
//! their records and directories; live tasks and read users are refused.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{TestResult, call, harness, perms, seed_github_user, seed_sandbox, seed_task};
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

#[tokio::test]
async fn write_users_delete_resting_tasks_with_their_records() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    let worktree = data.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Closed,
        Some(&worktree),
    )
    .await?;
    seed_task(&h, "t2", "s1", "u-alice", TaskStatus::Running, None).await?;
    let fb = call(
        &h.router,
        Method::POST,
        "/api/tasks/t1/feedback",
        Some(json!({"verdict": "correct", "comment": "ok"})),
        &alice,
    )
    .await?;
    assert_eq!(fb.status, StatusCode::NO_CONTENT, "{:?}", fb.body);

    // AC-ANL-2: feedback is stored per user and shown with the task; a second answer replaces
    // the first; a read user may answer too; an unknown verdict is refused.
    for (who, body) in [
        (
            &alice,
            json!({"verdict": "incorrect", "comment": "misses the empty cart"}),
        ),
        (&bob, json!({"verdict": "correct"})),
    ] {
        let answered = call(
            &h.router,
            Method::POST,
            "/api/tasks/t1/feedback",
            Some(body),
            who,
        )
        .await?;
        assert_eq!(
            answered.status,
            StatusCode::NO_CONTENT,
            "{:?}",
            answered.body
        );
    }
    let bad = call(
        &h.router,
        Method::POST,
        "/api/tasks/t1/feedback",
        Some(json!({"verdict": "maybe"})),
        &alice,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let detail = call(&h.router, Method::GET, "/api/tasks/t1", None, &alice).await?;
    let feedback = detail.body["feedback"].as_array().ok_or("feedback")?;
    assert_eq!(feedback.len(), 2, "one answer per user: {feedback:?}");
    let by = |login: &str| feedback.iter().find(|f| f["login"] == login).cloned();
    let alices = by("alice").ok_or("alice's feedback")?;
    assert_eq!(alices["verdict"], "incorrect");
    assert_eq!(alices["comment"], "misses the empty cart");
    assert_eq!(by("bob").ok_or("bob's feedback")?["verdict"], "correct");

    let refused = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &bob).await?;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "read users cannot delete"
    );
    let live = call(&h.router, Method::DELETE, "/api/tasks/t2", None, &alice).await?;
    assert_eq!(
        live.status,
        StatusCode::CONFLICT,
        "live tasks must be stopped first"
    );

    let deleted = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{:?}", deleted.body);
    assert!(h.store.task(&TaskId::parse("t1")?).await?.is_none());
    let gone = call(&h.router, Method::GET, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let again = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // The other task is untouched; the archive list does not show the deleted one.
    assert!(h.store.task(&TaskId::parse("t2")?).await?.is_some());
    let archived = call(&h.router, Method::GET, "/api/archived-tasks", None, &alice).await?;
    assert!(!archived.body.to_string().contains("\"t1\""));
    Ok(())
}

/// Yulius runs Claude Code only (F 2026-10-08): no harness choice on tasks and no setting for
/// harness switching.
#[tokio::test]
async fn tasks_run_claude_code_only() -> TestResult {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let codex = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "x", "harness": "codex"})),
        &alice,
    )
    .await?;
    assert_eq!(
        codex.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        codex.body
    );
    let settings = call(&h.router, Method::GET, "/api/admin/settings", None, &alice).await?;
    assert_eq!(settings.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// SPEC-039: closing a finished task for its deletion when its session already ended by itself
/// (between the Done check and the close) is not a conflict: the task rests and can go.
#[tokio::test]
async fn closing_a_task_whose_session_already_ended_is_not_a_conflict() -> TestResult {
    let h = harness()?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Closed, None).await?;
    h.app.close_task_and_wait(&TaskId::parse("t1")?).await?;
    Ok(())
}

/// A process host whose `terminate` fails (a signal that cannot be delivered, a process that
/// survives SIGKILL) until told otherwise; nothing else is reached by these tests.
#[derive(Default)]
struct StubbornHost {
    fails: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl yulius_app::ports::session::SessionHost for StubbornHost {
    async fn spawn(
        &self,
        _: &std::path::Path,
        _: &str,
        _: &str,
        _: &yulius_app::ports::session::LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::SpawnedRun> {
        Err(yulius_app::error::AppError::Upstream(
            "no spawn here".into(),
        ))
    }
    async fn attach(
        &self,
        _: &std::path::Path,
        _: &[u8; 32],
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::AttachedRun> {
        Err(yulius_app::error::AppError::Upstream(
            "no attach here".into(),
        ))
    }
    fn inspect(&self, _: &std::path::Path) -> yulius_app::ports::session::RunInspection {
        yulius_app::ports::session::RunInspection::Missing
    }
    fn terminate(&self, _: &std::path::Path) -> yulius_app::error::AppResult<()> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fails.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(yulius_app::error::AppError::Upstream(
                "process 4242 still alive after SIGKILL".into(),
            ));
        }
        Ok(())
    }
}

/// A harness whose stop and close escalation lasts `escalation_ms`, on `host`.
fn harness_on(
    host: std::sync::Arc<StubbornHost>,
    escalation_ms: u64,
) -> Result<support::Harness, Box<dyn std::error::Error>> {
    support::harness_with(support::HarnessOptions {
        session_host: Some(host),
        configure: Some(Box::new(move |c| c.stop_escalation_ms = escalation_ms)),
        ..support::HarnessOptions::default()
    })
}

/// Waits (bounded) until `check` holds.
async fn until<F, Fut>(what: &str, mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if check().await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

/// Waits (bounded) for a terminal notice of the task that contains `needle`.
async fn notice(
    frames: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>,
    needle: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let wait = async {
        loop {
            if let yulius_app::hub::HubFrame::Notice(text) = frames.recv().await?
                && text.contains(needle)
            {
                return Ok::<String, Box<dyn std::error::Error>>(text);
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), wait).await?
}

/// SPEC-065 END-001, END-002 (#284): a stopped run whose host cannot be terminated is not
/// recorded as ended: it stays live, the failure is said on the task, and deletion is refused
/// with nothing deleted. Once the host can be terminated, stopping again ends it.
#[tokio::test]
async fn a_stopped_run_whose_host_cannot_be_terminated_stays_live_and_blocks_deletion() -> TestResult
{
    let host = std::sync::Arc::new(StubbornHost::default());
    host.fails.store(true, std::sync::atomic::Ordering::SeqCst);
    let h = harness_on(host.clone(), 50)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let id = TaskId::parse("t1")?;
    let mut frames = h.app.runtime(&id).hub.watch();

    h.app.stop_task_now(&id).await?;
    let said = notice(&mut frames, "could not end").await?;
    assert!(said.contains("still alive after SIGKILL"), "{said}");
    assert!(
        h.store.live_attempt(&id).await?.is_some(),
        "the run is not recorded as ended"
    );
    let task = h.store.task(&id).await?.ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Stopping);

    let refused = call(&h.router, Method::DELETE, "/api/tasks/t1", None, &alice).await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(h.store.task(&id).await?.is_some(), "nothing is deleted");

    // The host can be terminated now: stopping again ends the run.
    host.fails.store(false, std::sync::atomic::Ordering::SeqCst);
    h.app.stop_task_now(&id).await?;
    until("the run to end", || async {
        h.store.live_attempt(&id).await.is_ok_and(|a| a.is_none())
            && h.store
                .task(&id)
                .await
                .is_ok_and(|t| t.is_some_and(|t| !t.status.is_live()))
    })
    .await?;
    assert!(host.calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    Ok(())
}

/// SPEC-065 END-001, END-002 (#284): closing a Done task for its deletion when its host cannot
/// be terminated answers at once that the session could not be ended; the run stays live and
/// the task needs attention.
#[tokio::test]
async fn a_closed_run_whose_host_cannot_be_terminated_is_a_conflict_and_stays_live() -> TestResult {
    let host = std::sync::Arc::new(StubbornHost::default());
    host.fails.store(true, std::sync::atomic::Ordering::SeqCst);
    let h = harness_on(host.clone(), 50)?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let id = TaskId::parse("t1")?;

    let started = std::time::Instant::now();
    let closed = h.app.close_task_and_wait(&id).await;
    let Err(yulius_app::error::AppError::Conflict(why)) = closed else {
        return Err(format!("expected a conflict, got {closed:?}").into());
    };
    assert!(why.contains("could not be ended"), "{why}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "answered at once, not at the bound: {:?}",
        started.elapsed()
    );
    assert!(
        h.store.live_attempt(&id).await?.is_some(),
        "the run stays live"
    );
    let task = h.store.task(&id).await?.ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(
        task.activity,
        Some(yulius_domain::task::Activity::NeedsAttention)
    );
    Ok(())
}

/// SPEC-065 END-004 (#290): the wait looks again every second even while unrelated events keep
/// arriving: a run that ends without its task's update being published is still seen.
#[tokio::test]
async fn the_close_recheck_fires_while_unrelated_events_keep_arriving() -> TestResult {
    let h = harness()?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let id = TaskId::parse("t1")?;
    let app = h.app.clone();
    let noise = tokio::spawn(async move {
        loop {
            app.events.publish(
                None,
                false,
                yulius_contracts::events::UiEvent::TaskRemoved {
                    task_id: "another".into(),
                },
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let app = h.app.clone();
    let attempt = yulius_domain::ids::AttemptId::parse("a-t1")?;
    let task_id = id.clone();
    let ended = tokio::spawn(async move {
        // The run ends behind the bus's back: no update of the task is published.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let store = &app.deps.store;
        let update = yulius_app::ports::store::AttemptUpdate {
            ended_at: Some(app.now()),
            exit_reason: Some("closed".into()),
            ..Default::default()
        };
        store
            .transition_attempt(&attempt, yulius_domain::task::AttemptState::Exited, &update)
            .await?;
        store
            .transition_task(
                &task_id,
                Some(TaskStatus::Running),
                TaskStatus::Closed,
                None,
                app.now(),
            )
            .await?;
        Ok::<(), yulius_app::error::AppError>(())
    });
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        h.app.close_task_and_wait(&id),
    )
    .await;
    noise.abort();
    ended.await??;
    closed??;
    Ok(())
}

/// SPEC-065 END-006 (#290): a task that is switching when it is closed for deletion was never
/// closed: it is refused at once with the stop-first message, not after the whole bound.
#[tokio::test]
async fn closing_a_switching_task_for_deletion_is_refused_at_once() -> TestResult {
    let h = harness()?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Switching, None).await?;
    support::seed_live_attempt(&h, "t1", "tok-t1").await?;
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        h.app.close_task_and_wait(&TaskId::parse("t1")?),
    )
    .await?;
    let Err(yulius_app::error::AppError::Conflict(why)) = closed else {
        return Err(format!("expected a conflict, got {closed:?}").into());
    };
    assert_eq!(why, "stop the task before deleting it");
    Ok(())
}
