//! Parked sessions (SPEC-512 part 1, #1412, ADR-0367), with real processes (supervisor →
//! detached `yulius ptyhost` → fake agent): a task whose turn ended and that waits for a person
//! is parked after the grace period, frees its session for a queued task, keeps its place, and
//! resumes in its own session when someone writes to it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::coordination::ProjectCaller;
use yulius_app::model::{LaunchKind, TaskMode};
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store::TaskStore;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

use crate::first_message::{data_dir, hooked, new_task, runtime, screen, start_with, wait_for};
use crate::pool_supervision::{ScriptedUsage, seed};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use crate::task_message::superagent;

type Error = Box<dyn std::error::Error>;

/// A hooked app whose idle sessions park at once (`park_after_ms` 1) under a limit of `slots`.
async fn app(data: &Path, slots: u32, park_after_ms: u64) -> Result<Arc<App>, Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let agent = FakeAgentOptions {
        answers_first_message: true,
        // A resume's prompt reaches the agent, as `claude --resume <id> <prompt>` gives it.
        resume_prompts: true,
        ..hooked()
    };
    let (app, _) = start_with(
        data,
        &fake,
        agent,
        0,
        Arc::new(ScriptedUsage::default()),
        |config| {
            config.max_live_sessions = slots;
            config.park_after_ms = park_after_ms;
        },
    )
    .await?;
    Ok(app)
}

/// A running task whose agent answered its first message and waits for a person.
async fn answered(app: &Arc<App>, data: &Path) -> Result<(TaskId, ProjectCaller), Error> {
    let (sandbox, user) = seed(app, data, &["a-only"]).await?;
    let id = new_task(
        app,
        &sandbox,
        &user,
        "Budget: answer with an echo",
        TaskMode::Prompt,
    )
    .await?;
    wait_for("the first answer", Duration::from_secs(30), || async {
        screen(app, &id).contains("ECHO: Budget: answer with an echo")
    })
    .await?;
    wait_for("the turn's end", Duration::from_secs(30), || async {
        status_of(app, &id).await.1 == Some(Activity::Waiting)
    })
    .await?;
    Ok((id, superagent(&sandbox, &user, "admin")))
}

async fn status_of(app: &App, id: &TaskId) -> (Option<TaskStatus>, Option<Activity>) {
    match TaskStore::task(app.deps.store.as_ref(), id).await {
        Ok(Some(task)) => (Some(task.status), task.activity),
        _ => (None, None),
    }
}

async fn parked(app: &Arc<App>, id: &TaskId) -> TestResult {
    app.park_idle_sessions_now().await;
    wait_for("the task to park", Duration::from_secs(30), || async {
        status_of(app, id).await.0 == Some(TaskStatus::Parked)
    })
    .await
}

/// The task's agent asks its person to choose (`needs_you`).
async fn ask(app: &App, id: &TaskId) -> TestResult {
    let asked = NeedsYouRecord {
        action: "Choose the colour".into(),
        happened: None,
        problem: Some("Two options".into()),
        decision: true,
        link: None,
        at: 1,
        cause: None,
    };
    app.deps
        .store
        .set_task_needs_you(id, Some(&asked), app.now())
        .await?;
    Ok(())
}

async fn asks(app: &App, id: &TaskId) -> Result<bool, Error> {
    Ok(app.deps.store.task_needs_you(id).await?.is_some())
}

fn audited(data: &Path, action: &str, id: &TaskId) -> Result<Vec<String>, Error> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    let mut query =
        db.prepare("SELECT actor FROM audit_log WHERE action = ?1 AND target = ?2 ORDER BY id")?;
    let rows = query.query_map([action, id.as_str()], |row| row.get(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// PRK-002, PRK-003, PRK-006: under a limit of one session, an answered task parks after the
/// grace period, keeps its activity and the list's group, ends its process and run, and its slot
/// starts the queued task.
#[test]
fn an_answered_task_parks_and_its_slot_starts_a_queued_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 1, 1).await?;
        let (first, caller) = answered(&app, &data).await?;
        let second = new_task(
            &app,
            &caller.sandbox,
            &caller.user,
            "Second: answer with an echo",
            TaskMode::Prompt,
        )
        .await?;
        // An answered agent is not Done locally: it holds the only slot, and the queued task
        // says so (PRK-007).
        app.drain_queue_now().await;
        assert_eq!(status_of(&app, &second).await.0, Some(TaskStatus::Queued));
        let queued = TaskStore::task(app.deps.store.as_ref(), &second)
            .await?
            .ok_or("task")?;
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Waiting for a free session: 1 of 1 in use")
        );
        parked(&app, &first).await?;
        wait_for(
            "the queued task to start",
            Duration::from_secs(60),
            || async { screen(&app, &second).contains("ECHO: Second: answer with an echo") },
        )
        .await?;
        let task = TaskStore::task(app.deps.store.as_ref(), &first)
            .await?
            .ok_or("task")?;
        assert_eq!(
            task.activity,
            Some(Activity::Waiting),
            "it keeps its activity"
        );
        assert!(app.deps.store.live_attempt(&first).await?.is_none());
        let runs = app.deps.store.attempts(&first).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].exit_reason.as_deref(), Some("parked"));
        assert_eq!(audited(&data, "task.parked", &first)?, ["yulius"]);
        let summary = app.task_summary(&task).await?;
        assert_eq!(summary.status, "parked");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-005: `task_message` to a parked task resumes it in the same provider session with the
/// message as its first prompt, and ends the request it had open.
#[test]
fn a_message_resumes_a_parked_task_in_its_own_session() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        ask(&app, &id).await?;
        parked(&app, &id).await?;
        assert!(asks(&app, &id).await?, "the request stays");
        let answer = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Blue: answer with an echo"}),
            )
            .await?;
        assert_eq!(answer["delivered"], "resumed");
        wait_for("the resumed answer", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Blue: answer with an echo")
        })
        .await?;
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        assert_eq!(runs[0].provider_session_id, runs[1].provider_session_id);
        assert!(!asks(&app, &id).await?, "answered");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-005, PRK-001: the web's Resume resumes a parked task with nothing typed and keeps its
/// request; archive closes a parked task and a stop stops one.
#[test]
fn resume_keeps_the_request_and_archive_or_stop_end_a_parked_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        ask(&app, &id).await?;
        parked(&app, &id).await?;
        app.continue_task(&ctx, id.as_str(), &serde_json::from_value(json!({}))?)
            .await?;
        wait_for("the resumed run", Duration::from_secs(60), || async {
            status_of(&app, &id).await.0 == Some(TaskStatus::Running)
                && app
                    .deps
                    .store
                    .attempts(&id)
                    .await
                    .is_ok_and(|a| a.len() == 2)
        })
        .await?;
        assert!(asks(&app, &id).await?, "Resume answers nothing");
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        // Parked again, then archived: closed.
        wait_for("the turn's end", Duration::from_secs(30), || async {
            status_of(&app, &id).await.1.is_some_and(Activity::is_idle)
        })
        .await?;
        parked(&app, &id).await?;
        app.archive_task(&ctx, id.as_str(), true).await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Closed));
        // A second task, parked and stopped.
        let other = new_task(
            &app,
            &caller.sandbox,
            &caller.user,
            "Other: answer with an echo",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("its turn's end", Duration::from_secs(30), || async {
            status_of(&app, &other).await.1 == Some(Activity::Waiting)
        })
        .await?;
        parked(&app, &other).await?;
        app.stop_task_now(&other).await?;
        assert!(matches!(
            status_of(&app, &other).await.0,
            Some(TaskStatus::Stopped(_))
        ));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-002: nothing parks with parking off (grace 0), within the grace period, or while a
/// person typed into the terminal.
#[test]
fn a_task_parks_only_after_its_grace_period_and_never_with_parking_off() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let off = app(&data, 8, 0).await?;
        let (id, _) = answered(&off, &data).await?;
        assert_eq!(off.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&off, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let long = app(&data, 8, 3_600_000).await?;
        let (id, _) = answered(&long, &data).await?;
        assert_eq!(long.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&long, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        // A person typed and sent nothing: their draft may be in the prompt box.
        app.record_typing(&id, &caller.user);
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
