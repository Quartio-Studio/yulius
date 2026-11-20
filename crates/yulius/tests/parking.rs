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
        named: Default::default(),
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
        let (id, caller) = answered(&app, &data).await?;
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
        let original = screen(&app, &id);
        assert!(!original.contains("Yulius parked"), "{original}");
        assert_cold_screen(&data, &id, &caller, &original).await?;
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

pub(crate) async fn assert_cold_screen(
    data: &Path,
    id: &TaskId,
    caller: &ProjectCaller,
    expected: &str,
) -> TestResult {
    let restart = self::app(data, 8, 1).await?;
    let ctx = AuthContext {
        user: restart
            .deps
            .store
            .user(&caller.user)
            .await?
            .ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    // Read through MCP first, before a page can warm the terminal hub.
    let remote = yulius_app::remote::RemoteCaller {
        ctx: ctx.clone(),
        token_id: "cold-screen".into(),
        scope: yulius_domain::permission::TokenScope::Read,
        login: "admin".into(),
        superagent: None,
    };
    let request = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "task_screen", "arguments": {"project":"acme/pool","task_id":id.as_str()},
            "_meta": {"yulius/structured":true}}
    }))?;
    let result = restart
        .remote_mcp_handle(&remote, request)
        .await
        .and_then(|reply| reply.result)
        .ok_or("task_screen")?;
    assert_eq!(
        crate::support::tool_text(&result)["screen"],
        expected.trim_end()
    );
    let terminal = restart
        .terminal_open(
            &ctx,
            id.as_str(),
            matches!(
                status_of(&restart, id).await.0,
                Some(TaskStatus::Queued | TaskStatus::WaitingCapacity | TaskStatus::Starting)
            )
            .then_some((22, 5)),
        )
        .await?;
    assert_eq!(screen(&restart, id), expected);
    assert_eq!(
        restart.terminal_state(id).await?.parked_screen,
        expected.contains("Yulius parked")
    );
    drop(terminal);
    Ok(())
}

/// The last attempt's screen stays visible while capacity holds a parked resume in the queue.
async fn queued_park_screen(
    app: &App,
    id: &TaskId,
    data: &Path,
    caller: &ProjectCaller,
) -> TestResult {
    wait_for(
        "the committed park screen",
        Duration::from_secs(10),
        || async { screen(app, id).contains("Yulius parked") },
    )
    .await?;
    let captured = screen(app, id);
    assert!(
        app.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Parked),
                TaskStatus::Queued,
                None,
                app.now()
            )
            .await?
    );
    // Loss and normal exit can save identical bytes in this fake agent. Make
    // the ordinary final screen distinct to prove which snapshot each reader uses.
    let last = app
        .deps
        .store
        .attempts(id)
        .await?
        .pop()
        .ok_or("parked attempt")?;
    let dir = Path::new(&last.run_dir);
    std::fs::write(dir.join("final-screen"), b"80 24\nShutdown screen")?;
    assert_cold_screen(data, id, caller, &captured).await?;
    // A restart in the gap between the state commit and the filename change.
    std::fs::rename(dir.join("parked-screen"), dir.join("parked-screen.pending"))?;
    assert_cold_screen(data, id, caller, &captured).await?;
    let renamed = dir.to_owned();
    yulius_app::race::on(yulius_app::race::Point::ParkScreenRead, dir, move || {
        let published = std::fs::rename(
            renamed.join("parked-screen.pending"),
            renamed.join("parked-screen"),
        );
        assert!(published.is_ok(), "{published:?}");
    });
    assert_cold_screen(data, id, caller, &captured).await?;
    // Starting is recorded before the provider acquires its account.
    assert!(
        app.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Queued),
                TaskStatus::Starting,
                None,
                app.now()
            )
            .await?
    );
    assert_cold_screen(data, id, caller, &captured).await?;
    // The resumed launch has no subscription available, so it waits without a new attempt.
    assert!(
        app.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Starting),
                TaskStatus::WaitingCapacity,
                None,
                app.now()
            )
            .await?
    );
    assert_cold_screen(data, id, caller, &captured).await
}

/// PRK-003 (review round 2): a switch that starts after the park decided gives way to it: the
/// task parks with the activity it had, and the switch says why it did not happen.
#[test]
fn a_switch_overtaken_by_a_park_parks_with_its_activity() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
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
        queued_park_screen(&app, &id, &data, &caller).await?;
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
        let (id, caller) = answered(&app, &data).await?;
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
        queued_park_screen(&app, &id, &data, &caller).await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A declared unfinished spec keeps the idle session live.
#[test]
fn an_unfinished_spec_keeps_its_session() -> TestResult {
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
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        let after = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        assert_eq!(after.status, TaskStatus::Running);
        assert!(app.task_summary(&after).await?.spec_unfinished);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A failed native goal has unfinished work even without a plan or pull request.
#[test]
fn an_unfinished_goal_keeps_its_session() -> TestResult {
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
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(summary(app.clone(), id.clone()).await?.status, "running");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-512 ANS-002 with PRK-005 (review round 3 of part 2): a question stays answerable while
/// it is open, also once its task parked: the named person's answer resumes the task in its own
/// session with the answer as its first prompt, ends the question, and the task stays its
/// owner's.
#[test]
fn a_named_persons_answer_resumes_a_parked_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let bob = yulius_app::model::UserRecord {
            id: yulius_domain::ids::UserId::parse("u-bob")?,
            github_login: Some("bob".into()),
            github_id: None,
            display_name: "Bob".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: yulius_domain::time::Timestamp(1),
            disabled_at: None,
        };
        yulius_app::ports::store::UserStore::insert_user(app.deps.store.as_ref(), &bob).await?;
        yulius_app::ports::store::PermissionStore::put_access(
            app.deps.store.as_ref(),
            &bob.id,
            &caller.sandbox,
            yulius_domain::permission::AccessLevel::Write,
            app.now(),
        )
        .await?;
        let asked = NeedsYouRecord {
            action: "Decide M1".into(),
            happened: None,
            problem: None,
            decision: true,
            link: None,
            at: 1,
            cause: None,
            named: yulius_app::needs_you::Named {
                who: vec!["bob".into()],
                withdrawn: Vec::new(),
            },
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;
        parked(&app, &id).await?;
        let ctx = AuthContext {
            user: bob,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.answer_needs_you(&ctx, id.as_str(), "M1: yes", Some(1))
            .await?;
        wait_for("the resumed answer", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Answer from bob")
        })
        .await?;
        assert!(screen(&app, &id).contains("M1: yes"));
        assert!(!asks(&app, &id).await?, "the question is answered");
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        let task = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        assert_eq!(task.owner, caller.user);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-512 ANS-003 with PRK-005 (review round 4 of part 2): the named person's own agent
/// answers a parked task's question with `task_message`: the task resumes with the answer.
#[test]
fn a_named_persons_message_resumes_a_parked_task() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let bob = yulius_app::model::UserRecord {
            id: yulius_domain::ids::UserId::parse("u-bob")?,
            github_login: Some("bob".into()),
            github_id: None,
            display_name: "Bob".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: yulius_domain::time::Timestamp(1),
            disabled_at: None,
        };
        yulius_app::ports::store::UserStore::insert_user(app.deps.store.as_ref(), &bob).await?;
        yulius_app::ports::store::PermissionStore::put_access(
            app.deps.store.as_ref(),
            &bob.id,
            &caller.sandbox,
            yulius_domain::permission::AccessLevel::Write,
            app.now(),
        )
        .await?;
        let asked = NeedsYouRecord {
            action: "Decide M1".into(),
            happened: None,
            problem: None,
            decision: true,
            link: None,
            at: 1,
            cause: None,
            named: yulius_app::needs_you::Named {
                who: vec!["bob".into()],
                withdrawn: Vec::new(),
            },
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), app.now())
            .await?;
        parked(&app, &id).await?;
        let _ = bob;
        let bobs_agent = yulius_app::coordination::ProjectCaller {
            sandbox: caller.sandbox.clone(),
            user: yulius_domain::ids::UserId::parse("u-bob")?,
            task: None,
            channel: yulius_app::coordination::Channel::Remote,
            actor: "remote:bob".into(),
            detail: json!({"via": "remote"}),
        };
        let sent = app
            .tool_task_message(
                &bobs_agent,
                &json!({"task_id": id.as_str(), "text": "M1: yes"}),
            )
            .await?;
        assert_eq!(sent["delivered"], "answered", "{sent}");
        wait_for("the resumed answer", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Answer from bob")
        })
        .await?;
        assert!(screen(&app, &id).contains("M1: yes"));
        assert!(!asks(&app, &id).await?, "the question is answered");
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        let task = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        assert_eq!(task.owner, caller.user);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// An agent secret's value, built so no scanner takes it for a real credential.
const DIGEST_SECRET: &str = concat!("sunflower", "-digest-", "7f3a9c2e");

/// The digest the owner last got, as JSON.
async fn latest_digest(
    app: &App,
    user: &yulius_domain::ids::UserId,
) -> Result<serde_json::Value, Error> {
    let stored = app
        .deps
        .store
        .latest_digest(user)
        .await?
        .ok_or("no digest")?;
    Ok(serde_json::from_str(&stored.body)?)
}

/// SPEC-512 DIG-003, DIG-004 (#1413), with part 1's parking off: the digest leaves a task that
/// asks its person running and lists it first; once nothing is asked, it parks the quiet task,
/// lists it with what its last message left, and a message resumes it in its own session.
#[test]
fn the_digest_parks_a_quiet_task_that_asks_nobody_and_lists_its_last_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 0).await?;
        // Its agent echoes an agent secret: the digest shows its name, never its value.
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        // A set-up instance keeps where each digest's listing ended.
        rusqlite::Connection::open(data.join("yulius.db"))?.execute_batch(
            "INSERT OR IGNORE INTO instance (singleton, public_url) VALUES (1, 'https://yulius.test')",
        )?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.create_agent_secret(
            &ctx,
            sandbox.as_str(),
            &yulius_contracts::api::secrets::AgentSecretInput {
                name: "DIGEST_KEY".into(),
                value: DIGEST_SECRET.into(),
                ..Default::default()
            },
        )
        .await?;
        let prompt = format!("Budget {DIGEST_SECRET}: answer with an echo");
        let id = new_task(&app, &sandbox, &user, &prompt, TaskMode::Prompt).await?;
        wait_for("the turn's end", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Budget")
                && status_of(&app, &id).await.1 == Some(Activity::Waiting)
        })
        .await?;
        let caller = superagent(&sandbox, &user, "admin");
        ask(&app, &id).await?;
        past_the_grace(&app, &id).await?;
        let run = app.send_digests_now(Some(1)).await;
        assert_eq!((run.sent, run.parked), (1, 0), "it asks");
        let digest = latest_digest(&app, &caller.user).await?;
        assert_eq!(
            digest["waiting"][0]["detail"], "Choose the colour",
            "{digest}"
        );
        assert_eq!(digest["parked"], json!([]), "{digest}");

        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
            .await?;
        past_the_grace(&app, &id).await?;
        let run = app.send_digests_now(Some(1)).await;
        assert_eq!((run.sent, run.parked), (1, 1));
        wait_for("the digest's park", Duration::from_secs(30), || async {
            status_of(&app, &id).await.0 == Some(TaskStatus::Parked)
        })
        .await?;
        let digest = latest_digest(&app, &caller.user).await?;
        let left = digest["parked"][0]["detail"]
            .as_str()
            .ok_or("parked detail")?;
        assert!(
            // The list shows an answered task under Done: the digest says so.
            left.starts_with("Finished. Last message: "),
            "{left}"
        );
        assert!(left.contains("ECHO: Budget [secret:DIGEST_KEY]"), "{left}");
        assert!(!left.contains(DIGEST_SECRET), "{left}");
        assert_eq!(audited(&data, "task.parked", &id)?, ["yulius"]);
        // The next digest does not list it as parked again (review round 1): nothing to report.
        let run = app.send_digests_now(Some(1)).await;
        assert_eq!(
            (run.sent, run.parked),
            (0, 0),
            "{}",
            latest_digest(&app, &caller.user).await?
        );

        let answer = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Go on: answer with an echo"}),
            )
            .await?;
        assert_eq!(answer["delivered"], "resumed");
        wait_for("the resumed message", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Go on: answer with an echo")
        })
        .await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-512 HOF-005, with real processes: once a handed-off task ended, its source task's live
/// session is told the outcome by Yulius, once.
#[test]
fn a_handed_off_tasks_outcome_reaches_its_source() -> TestResult {
    use yulius_app::ports::store_ext::HandoffRecord;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 0).await?;
        let (id, _) = answered(&app, &data).await?;
        let source = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("source")?;
        let mut other = app
            .deps
            .store
            .sandbox(&source.sandbox_id)
            .await?
            .ok_or("sandbox")?;
        other.id = yulius_domain::ids::SandboxId::parse("sb-api")?;
        other.slug = "api".into();
        other.repo_name = "api".into();
        other.port_base += 100;
        app.deps.store.insert_sandbox(&other).await?;
        let mut target = source.clone();
        target.id = TaskId::parse("t-api")?;
        target.sandbox_id = other.id.clone();
        target.number = 1;
        target.title = "Add the endpoint".into();
        target.status = TaskStatus::Running;
        target.activity = Some(Activity::Working);
        TaskStore::insert_task(app.deps.store.as_ref(), &target).await?;
        // Its pull request merged (review round 1: the link names owner and repository).
        app.deps
            .store
            .transition_delivery(
                &yulius_domain::delivery::DeliveryStage::None,
                &yulius_app::delivery::DeliveryRecord {
                    task_id: target.id.clone(),
                    stage: yulius_domain::delivery::DeliveryStage::MergedIntegration,
                    pr_number: Some(7),
                    updated_at: app.now(),
                },
            )
            .await?;
        app.deps
            .store
            .insert_handoff(&HandoffRecord {
                id: "ho_1".into(),
                source_task: source.id.clone(),
                source_sandbox: source.sandbox_id.clone(),
                target_sandbox: other.id.clone(),
                owner: source.owner.clone(),
                args: r#"{"sandbox":"api","title":"Add the endpoint","prompt":"P"}"#.into(),
                chain: vec![source.sandbox_id.as_str().to_owned()],
                depth: 1,
                state: "created".into(),
                reason: None,
                target_task: Some(target.id.clone()),
                created_at: app.now(),
            })
            .await?;
        // While the source shows its person a prompt, nothing is typed (review round 1).
        TaskStore::transition_task(
            app.deps.store.as_ref(),
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::NeedsAttention),
            app.now(),
        )
        .await?;
        assert_eq!(app.tell_handoff_outcomes().await, 0);
        assert!(!screen(&app, &id).contains("the task you handed off"));
        assert_eq!(app.deps.store.untold_handoffs().await?.len(), 1);
        TaskStore::transition_task(
            app.deps.store.as_ref(),
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
        wait_for("the outcome told", Duration::from_secs(30), || async {
            app.tell_handoff_outcomes().await;
            let shown = screen(&app, &id);
            shown.contains(&format!(
                "the task you handed off, {}/api #1",
                other.repo_owner
            )) && shown.contains(&format!(
                "is merged. Pull request: https://github.com/{}/api/pull/7",
                other.repo_owner
            ))
        })
        .await?;
        assert!(
            app.deps.store.untold_handoffs().await?.is_empty(),
            "told once"
        );
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn an_idle_task_keeps_its_session_while_its_recorded_pr_cannot_be_read() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 1, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        app.deps
            .store
            .transition_delivery(
                &DeliveryStage::None,
                &yulius_app::delivery::DeliveryRecord {
                    task_id: id.clone(),
                    stage: DeliveryStage::PrOpen,
                    pr_number: Some(1461),
                    updated_at: app.now(),
                },
            )
            .await?;
        past_the_grace(&app, &id).await?;
        // The fake project has no installation from which to read this head.
        let digest = app.send_digests_now(Some(1)).await;
        assert_eq!(digest.parked, 0);
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
