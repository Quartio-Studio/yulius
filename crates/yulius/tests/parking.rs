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

use crate::first_message::{
    data_dir, hooked, new_task, runtime, screen, start_hosted, start_with, wait_for,
};
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

/// Parks `id`: a pass runs at each look until it parked, so a pass that came in the millisecond
/// its state last changed (quiet 0 of a 1 ms grace) is followed by another.
async fn parked(app: &Arc<App>, id: &TaskId) -> TestResult {
    wait_for("the task to park", Duration::from_secs(30), || async {
        if status_of(app, id).await.0 == Some(TaskStatus::Running) {
            app.park_idle_sessions_now().await;
        }
        status_of(app, id).await.0 == Some(TaskStatus::Parked)
    })
    .await
}

/// Waits until the task's state is older than a 1 ms grace period, so a pass that parks
/// nothing says so because of what it checks, not because the grace is not over yet.
async fn past_the_grace(app: &App, id: &TaskId) -> TestResult {
    wait_for("the grace period", Duration::from_secs(10), || async {
        app.deps
            .store
            .task_times(id)
            .await
            .ok()
            .flatten()
            .is_some_and(|times| app.now().0 - times.state_at.0 > 2)
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
        // The Capacity card counts the queued task's session as working and the parked one
        // apart (PRK-008).
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let sessions = app.capacity_view(&ctx).await?.sessions;
        assert_eq!(
            (
                sessions.working,
                sessions.limit,
                sessions.parked,
                sessions.idle,
                sessions.queued,
                sessions.park_after_ms
            ),
            (1, 1, 1, 0, 0, 1)
        );
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
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003: a message that comes while a park is ending the session is refused with what to do,
/// never typed into the session that ends; sent again once the task parked, it resumes it.
#[test]
fn a_message_during_a_park_is_refused_and_resumes_the_task_once_parked() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let refused = Arc::new(std::sync::Mutex::new(None));
        {
            let (app, caller, refused) = (app.clone(), caller.clone(), refused.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::ParkDecided,
                Path::new(id.as_str()),
                move || {
                    let answer = tokio::task::block_in_place(|| {
                        handle.block_on(app.tool_task_message(
                            &caller,
                            &json!({"number": 1, "text": "Late: answer with an echo"}),
                        ))
                    });
                    if let Ok(mut slot) = refused.lock() {
                        *slot = Some(answer.map(|_| ()).map_err(|err| err.to_string()));
                    }
                },
            );
        }
        parked(&app, &id).await?;
        let answer = refused.lock().map_err(|_| "poisoned")?.clone();
        assert!(
            matches!(&answer, Some(Err(why)) if why.contains("is parking its idle session")),
            "{answer:?}"
        );
        let again = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Late: answer with an echo"}),
            )
            .await?;
        assert_eq!(again["delivered"], "resumed");
        wait_for("the resumed answer", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Late: answer with an echo")
        })
        .await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003 (review round 1, M1): a stop that lands while the parked run's end is being recorded
/// (after it took the park's intent) leaves the task stopped, never `stopping` with no run.
#[test]
fn a_stop_during_the_parks_end_stops_the_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        let stopped = Arc::new(std::sync::Mutex::new(None));
        {
            let (app, id, stopped) = (app.clone(), id.clone(), stopped.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::RunEndTaken,
                Path::new(id.clone().as_str()),
                move || {
                    let answer =
                        tokio::task::block_in_place(|| handle.block_on(app.stop_task_now(&id)));
                    if let Ok(mut slot) = stopped.lock() {
                        *slot = Some(answer.map_err(|err| err.to_string()));
                    }
                },
            );
        }
        past_the_grace(&app, &id).await?;
        app.park_idle_sessions_now().await;
        wait_for("the task to rest", Duration::from_secs(30), || async {
            matches!(status_of(&app, &id).await.0, Some(TaskStatus::Stopped(_)))
        })
        .await?;
        let answer = stopped.lock().map_err(|_| "poisoned")?.clone();
        assert!(matches!(answer, Some(Ok(()))), "{answer:?}");
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-004 (review round 1, M3): a task chained after one that is Done and parked starts, as it
/// starts after a Done one that kept its session.
#[test]
fn a_task_chained_after_a_parked_done_task_starts() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        parked(&app, &id).await?;
        // Its goal achieved: set on the parked task, which has no session whose turn's end
        // would read it again.
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let before = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        let mut waiting = before.clone();
        waiting.id = TaskId::parse("t-next")?;
        waiting.number = 99;
        waiting.title = "Next".into();
        waiting.status = TaskStatus::Chained;
        waiting.activity = None;
        waiting.branch = None;
        waiting.worktree_path = None;
        let waiting = app.deps.store.insert_task(&waiting).await?;
        yulius_app::ports::store_ext::IntakeStore::insert_chain(
            app.deps.store.as_ref(),
            &yulius_app::intake::ChainRecord {
                task_id: waiting.id.clone(),
                after_task_id: Some(id.clone()),
                after_number: before.number,
                blocked_reason: None,
                created_at: app.now(),
                rule: None,
            },
        )
        .await?;
        app.advance_all_chains().await;
        wait_for(
            "the chained task to leave its chain",
            Duration::from_secs(30),
            || async {
                status_of(&app, &waiting.id)
                    .await
                    .0
                    .is_some_and(|status| status != TaskStatus::Chained)
            },
        )
        .await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-001 (review round 1, M4): a parked task keeps its scope claims: another task claiming
/// the same paths is told of it.
#[test]
fn a_parked_task_keeps_its_claims() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (a, caller) = answered(&app, &data).await?;
        let agent = |id: TaskId| {
            let app = app.clone();
            async move {
                Ok::<_, Error>(yulius_app::coordination::AgentContext {
                    attempt: app
                        .deps
                        .store
                        .live_attempt(&id)
                        .await?
                        .ok_or("a live run")?,
                    task: TaskStore::task(app.deps.store.as_ref(), &id)
                        .await?
                        .ok_or("the task")?,
                })
            }
        };
        app.tool_scope_claim(
            &agent(a.clone()).await?,
            &json!({"pattern": "crates/a/src/x.rs", "note": "splitting x"}),
        )
        .await?;
        parked(&app, &a).await?;
        let b = new_task(
            &app,
            &caller.sandbox,
            &caller.user,
            "Feature: answer with an echo",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("its first answer", Duration::from_secs(30), || async {
            screen(&app, &b).contains("ECHO: Feature: answer with an echo")
        })
        .await?;
        let answer = app
            .tool_scope_claim(&agent(b).await?, &json!({"pattern": "crates/a/**"}))
            .await?;
        assert_eq!(
            answer["conflicts"][0]["their_note"], "splitting x",
            "{answer}"
        );
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-002 (review round 1, M5): a session is parked only on a fresh look at its background work;
/// a look that gives no answer counts as work running and keeps the session.
#[test]
fn a_look_that_gives_no_answer_keeps_the_session() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let host = Arc::new(crate::support::gated::GatedHost::new(
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius")),
        ));
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start_hosted(
            &data,
            &fake,
            agent,
            (
                0,
                Arc::new(ScriptedUsage::default()),
                Arc::new(yulius::wiring::clock::SystemClock),
            ),
            Some(host.clone()),
            |config| config.park_after_ms = 1,
        )
        .await?;
        let (id, _) = answered(&app, &data).await?;
        host.silence_work_probe(true);
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        host.silence_work_probe(false);
        parked(&app, &id).await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-002 (review round 1, M6): what the person still has to answer in the session, or a
/// person's pause, keeps the session, whatever the grace period.
#[test]
fn a_question_in_the_terminal_or_a_pause_keeps_the_session() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        // Claude Code asks in its terminal: only the live session can take the answer.
        app.deps
            .store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        app.deps
            .store
            .set_activity(&id, Some(Activity::Waiting), app.now())
            .await?;
        // A person paused it.
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.pause_task_for_person(&ctx, id.as_str()).await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-005 (review round 1, M6): a message to a parked task while every session is in use
/// queues the resume with its prompt; once a session is free it resumes with that message.
#[test]
fn a_resume_at_the_session_limit_waits_with_its_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 1, 1).await?;
        let (first, caller) = answered(&app, &data).await?;
        parked(&app, &first).await?;
        let second = new_task(
            &app,
            &caller.sandbox,
            &caller.user,
            "Busy: answer with an echo",
            TaskMode::Prompt,
        )
        .await?;
        wait_for(
            "the second task's answer",
            Duration::from_secs(30),
            || async { screen(&app, &second).contains("ECHO: Busy: answer with an echo") },
        )
        .await?;
        let answer = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Later: answer with an echo"}),
            )
            .await?;
        assert_eq!(answer["delivered"], "resumed");
        assert_eq!(status_of(&app, &first).await.0, Some(TaskStatus::Queued));
        app.stop_task_now(&second).await?;
        wait_for("the queued resume", Duration::from_secs(60), || async {
            screen(&app, &first).contains("Later: answer with an echo")
        })
        .await?;
        let runs = app.deps.store.attempts(&first).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        assert_eq!(runs[0].provider_session_id, runs[1].provider_session_id);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003 (review round 1, F11): a park whose session cannot be ended gives up: the session
/// goes on and a message is typed into it again, not refused as "parking" for ever.
#[test]
fn a_park_that_cannot_end_its_session_gives_up() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let host = Arc::new(crate::support::gated::GatedHost::new(
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius")),
        ));
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start_hosted(
            &data,
            &fake,
            agent,
            (
                0,
                Arc::new(ScriptedUsage::default()),
                Arc::new(yulius::wiring::clock::SystemClock),
            ),
            Some(host.clone()),
            |config| {
                config.park_after_ms = 1;
                config.stop_escalation_ms = 500;
            },
        )
        .await?;
        let (id, caller) = answered(&app, &data).await?;
        host.refuse_ends(true);
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 1);
        host.shutdown_refused().await;
        wait_for("the park to give up", Duration::from_secs(30), || async {
            app.tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Still here: answer with an echo"}),
            )
            .await
            .is_ok_and(|answer| answer["delivered"] == "sent")
        })
        .await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        host.refuse_ends(false);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003 (review round 2): a switch that starts after the park decided gives way to it: the
/// task parks with the activity it had, and the switch says why it did not happen.
#[test]
fn a_switch_overtaken_by_a_park_parks_with_its_activity() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        let switched = Arc::new(std::sync::Mutex::new(None));
        {
            let (app, target, switched) = (app.clone(), id.clone(), switched.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::ParkDecided,
                Path::new(id.as_str()),
                move || {
                    let answer = tokio::task::block_in_place(|| {
                        handle.block_on(app.switch_task_now(&target, None, "manual", None))
                    });
                    if let Ok(mut slot) = switched.lock() {
                        *slot = Some(answer.map_err(|err| err.to_string()));
                    }
                },
            );
        }
        parked(&app, &id).await?;
        assert_eq!(status_of(&app, &id).await.1, Some(Activity::Waiting));
        let answer = switched.lock().map_err(|_| "poisoned")?.clone();
        assert!(
            matches!(&answer, Some(Err(why)) if why.contains("the idle session was parked")),
            "{answer:?}"
        );
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003 (review round 2): a parked session that does not hear the request to end is ended by
/// the escalation, and its loss parks the task all the same (`mark_lost` with the park's intent).
#[test]
fn a_session_deaf_to_the_park_is_parked_by_the_escalation() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let host = Arc::new(crate::support::gated::GatedHost::new(
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius")),
        ));
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start_hosted(
            &data,
            &fake,
            agent,
            (
                0,
                Arc::new(ScriptedUsage::default()),
                Arc::new(yulius::wiring::clock::SystemClock),
            ),
            Some(host.clone()),
            |config| {
                config.park_after_ms = 1;
                config.stop_escalation_ms = 500;
            },
        )
        .await?;
        let (id, _) = answered(&app, &data).await?;
        host.deafen(true);
        parked(&app, &id).await?;
        // The escalation's loss or the reader's exit records it, whichever comes first; the
        // order where the loss comes first is pinned by
        // `a_loss_recorded_while_the_reader_holds_the_park_still_parks`.
        let runs = app.deps.store.attempts(&id).await?;
        assert!(
            matches!(runs[0].exit_reason.as_deref(), Some("lost" | "parked")),
            "{:?}",
            runs[0].exit_reason
        );
        assert_eq!(status_of(&app, &id).await.1, Some(Activity::Waiting));
        assert_eq!(audited(&data, "task.parked", &id)?, ["yulius"]);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-002 (review round 2): a prompt Yulius held for a person's decision keeps the session (it
/// is back in the prompt box).
#[test]
fn a_held_prompt_keeps_the_session() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        yulius_app::ports::store_ext::IntakeStore::insert_hold(
            app.deps.store.as_ref(),
            &yulius_app::intake::HoldRecord {
                id: "h-1".into(),
                task_id: id.clone(),
                class: yulius_domain::intake::PromptClass::NewScope,
                prompt: "Also rewrite the billing page".into(),
                prompt_sha256: "0".repeat(64),
                percent: 91,
                typed_by: Some(caller.user.clone()),
                state: yulius_app::intake::hold_state::PENDING.into(),
                created_at: app.now(),
                resolved_by: None,
                resolved_at: None,
                new_task_id: None,
            },
        )
        .await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-005 (review round 2): only who steers a task resumes it: another writer's Resume and
/// message are refused, and the task stays parked.
#[tokio::test]
async fn only_its_owner_resumes_a_parked_task() -> TestResult {
    use crate::support::{call, harness, perms, seed_github_user, seed_sandbox, seed_task};
    let h = harness()?;
    crate::support::exhausted_subscription(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    for login in ["alice", "bob"] {
        h.github.set_perm(login, "acme/app", perms("write"));
    }
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Parked, None).await?;
    let resumed = call(
        &h.router,
        axum::http::Method::POST,
        "/api/tasks/t1/continue",
        Some(json!({})),
        &bob,
    )
    .await?;
    assert_eq!(
        resumed.status,
        axum::http::StatusCode::FORBIDDEN,
        "{:?}",
        resumed.body
    );
    let caller = yulius_app::coordination::ProjectCaller {
        sandbox: yulius_domain::ids::SandboxId::parse("s1")?,
        user: yulius_domain::ids::UserId::parse("u-bob")?,
        task: None,
        channel: yulius_app::coordination::Channel::Superagent,
        actor: "cockpit:bob".into(),
        detail: json!({"via": "cockpit", "shell": "sh-test"}),
    };
    let messaged = h
        .app
        .tool_task_message(&caller, &json!({"task_id": "t1", "text": "go on"}))
        .await;
    assert!(
        matches!(&messaged, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("@alice")),
        "{messaged:?}"
    );
    let task = h.store.task(&TaskId::parse("t1")?).await?.ok_or("task")?;
    assert_eq!(task.status, TaskStatus::Parked);
    Ok(())
}

/// PRK-002 (review round 2): the onboarding task is never parked: its run's end decides the
/// sandbox's onboarding.
#[test]
fn the_onboarding_task_is_not_parked() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute(
            "UPDATE tasks SET origin = 'onboarding' WHERE id = ?1",
            [id.as_str()],
        )?;
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-003 (review round 3, L1): the session exits on the park's request and its reader takes
/// the park's intent, then holds; the escalation finds the run still recorded live and records
/// its loss first. The task is parked all the same, not stopped as lost.
#[test]
fn a_loss_recorded_while_the_reader_holds_the_park_still_parks() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start_with(
            &data,
            &fake,
            agent,
            0,
            Arc::new(ScriptedUsage::default()),
            |config| {
                config.park_after_ms = 1;
                config.stop_escalation_ms = 300;
            },
        )
        .await?;
        let (id, _) = answered(&app, &data).await?;
        let run = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("a live run")?;
        {
            // The reader holds the park's intent until the escalation recorded the loss.
            let (app, run) = (app.clone(), run.id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::RunEndTaken,
                Path::new(id.as_str()),
                move || {
                    tokio::task::block_in_place(|| {
                        handle.block_on(async {
                            let _ = wait_for("the loss", Duration::from_secs(20), || async {
                                app.deps
                                    .store
                                    .attempt(&run)
                                    .await
                                    .ok()
                                    .flatten()
                                    .is_some_and(|attempt| attempt.state.is_terminal())
                            })
                            .await;
                        });
                    });
                },
            );
        }
        parked(&app, &id).await?;
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(
            runs[0].exit_reason.as_deref(),
            Some("lost"),
            "the loss was recorded first"
        );
        assert_eq!(status_of(&app, &id).await.1, Some(Activity::Waiting));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-004 (review round 3, W1): a parked task's summary says what its idle run's did: a spec
/// that does not say done keeps it "Your turn" on the web as on the server.
#[test]
fn a_parked_tasks_summary_keeps_its_unfinished_work() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        app.deps
            .store
            .set_spec_status(&id, Some("in-progress"))
            .await?;
        let before = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        assert!(app.task_summary(&before).await?.spec_unfinished);
        parked(&app, &id).await?;
        let after = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        assert!(app.task_summary(&after).await?.spec_unfinished);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// PRK-004 (review round 4, G1): a goal task whose goal failed is parked "Your turn", and stays
/// so for a server started afresh, which has no session that knew the goal: the parked run's
/// directory keeps it.
#[test]
fn a_parked_unfinished_goal_stays_your_turn_across_a_restart() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let agent = FakeAgentOptions {
            goal_verdict: Some("failed"),
            ..hooked()
        };
        let (app, _) = start_with(
            &data,
            &fake,
            agent.clone(),
            0,
            Arc::new(ScriptedUsage::default()),
            |config| config.park_after_ms = 1,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Merge on green", TaskMode::Goal).await?;
        let summary = |app: Arc<App>, id: TaskId| async move {
            let task = TaskStore::task(app.deps.store.as_ref(), &id)
                .await?
                .ok_or("task")?;
            Ok::<_, Error>(app.task_summary(&task).await?)
        };
        wait_for(
            "the failed goal's turn end",
            Duration::from_secs(90),
            || async {
                summary(app.clone(), id.clone())
                    .await
                    .is_ok_and(|s| s.goal_unfinished && s.activity.as_deref() == Some("waiting"))
            },
        )
        .await?;
        parked(&app, &id).await?;
        assert!(summary(app.clone(), id.clone()).await?.goal_unfinished);
        // A server started afresh on the same data: no runtime knows the goal.
        let (fresh, _) = start_with(
            &data,
            &fake,
            agent,
            0,
            Arc::new(ScriptedUsage::default()),
            |config| config.park_after_ms = 1,
        )
        .await?;
        let shown = summary(fresh.clone(), id.clone()).await?;
        assert_eq!(shown.status, "parked");
        assert!(
            shown.goal_unfinished,
            "the parked run's directory keeps the goal's state"
        );
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
