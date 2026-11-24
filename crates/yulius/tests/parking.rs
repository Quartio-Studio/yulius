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
use yulius_app::race::Point;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, StopReason, TaskStatus};
use yulius_ptyhost::test_support::locate_fake_agent;

use crate::first_message::{
    data_dir, hooked, new_task, runtime, screen, start_hosted, start_with, wait_for,
};
use crate::pool_supervision::{ScriptedUsage, seed};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use crate::task_message::superagent;

mod armed_notice;
mod candidate;
mod replaced_output;

type Error = Box<dyn std::error::Error>;

/// A hooked app whose idle sessions park at once (`park_after_ms` 1) under a limit of `slots`.
async fn app(data: &Path, slots: u32, park_after_ms: u64) -> Result<Arc<App>, Error> {
    Ok(app_with_pulls(data, slots, park_after_ms).await?.0)
}

async fn app_with_pulls(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    configured_app_with_pulls(data, slots, park_after_ms, |_| {}).await
}

async fn configured_app_with_pulls(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
    configure: impl FnOnce(&mut yulius_app::context::AppConfig),
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    app_with_pulls_and_clock(
        data,
        slots,
        park_after_ms,
        Arc::new(yulius::wiring::clock::SystemClock),
        configure,
    )
    .await
}

async fn app_with_pulls_and_clock(
    data: &Path,
    slots: u32,
    park_after_ms: u64,
    clock: Arc<dyn yulius_app::ports::Clock>,
    configure: impl FnOnce(&mut yulius_app::context::AppConfig),
) -> Result<(Arc<App>, Arc<crate::support::FakeGitHub>), Error> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let agent = FakeAgentOptions {
        answers_first_message: true,
        // A resume's prompt reaches the agent, as `claude --resume <id> <prompt>` gives it.
        resume_prompts: true,
        ..hooked()
    };
    let (app, _, github) = crate::first_message::start_hosted_with_github(
        data,
        &fake,
        agent,
        (0, Arc::new(ScriptedUsage::default()), clock),
        None,
        None,
        |config| {
            config.max_live_sessions = slots;
            config.park_after_ms = park_after_ms;
            configure(config);
        },
    )
    .await?;
    Ok((app, github))
}

/// Give the real parking decision a readable open PR at the head the fixture controls.
async fn open_pull_at(
    app: &App,
    github: &crate::support::FakeGitHub,
    task: &TaskId,
    number: u64,
    head: &str,
) -> TestResult {
    use yulius_app::ports::github::GhPull;
    let store = &app.deps.store;
    store.init_instance("https://yulius.test", "x").await?;
    let cfg = json!({"app_id":1,"slug":"test","client_id":"test","client_secret":"synthetic","private_key_pem":"synthetic"});
    store
        .set_github_app(
            &app.deps.crypto.seal(&serde_json::to_vec(&cfg)?)?,
            "acme",
            "org",
        )
        .await?;
    store.set_installation(Some(7)).await?;
    let branch = store
        .task(task)
        .await?
        .and_then(|t| t.branch)
        .ok_or("task branch")?;
    github
        .pulls
        .lock()
        .map_err(|_| "fixture pull")?
        .push(GhPull {
            number,
            html_url: format!("https://github.test/acme/pool/pull/{number}"),
            title: "Repair checks".into(),
            state: "open".into(),
            merged: false,
            base: "dev".into(),
            base_sha: String::new(),
            head: branch,
            head_sha: head.into(),
            mergeable: Some(true),
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    Ok(())
}

fn set_pull_head(github: &crate::support::FakeGitHub, number: u64, head: &str) -> TestResult {
    github
        .pulls
        .lock()
        .map_err(|_| "fixture pull")?
        .iter_mut()
        .find(|p| p.number == number)
        .ok_or("open fixture pull")?
        .head_sha = head.into();
    Ok(())
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

/// How long [`parked`] waits for a park.
const PARK_WAIT: Duration = Duration::from_secs(30);

/// Parks `id`: a pass runs at each look until it parked, so a pass that came in the millisecond
/// its state last changed (quiet 0 of a 1 ms grace) is followed by another.
async fn parked(app: &Arc<App>, id: &TaskId) -> TestResult {
    parked_within(app, id, PARK_WAIT).await
}

/// [`parked`] with its own bound.
async fn parked_within(app: &Arc<App>, id: &TaskId, bound: Duration) -> TestResult {
    wait_for("the task to park", bound, || async {
        if status_of(app, id).await.0 == Some(TaskStatus::Running) {
            app.park_idle_sessions_now().await;
        }
        status_of(app, id).await.0 == Some(TaskStatus::Parked)
    })
    .await
}

#[test]
fn a_continue_cannot_reopen_a_replacement_that_also_closed() -> TestResult {
    superseded_owner_continue(false)
}

#[test]
fn an_archive_at_the_continue_claim_keeps_the_new_decision() -> TestResult {
    superseded_owner_continue(true)
}

fn superseded_owner_continue(archive_at_claim: bool) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.close_task_and_wait(&id).await?;
        let point = if archive_at_claim {
            Point::StartClaimReady
        } else {
            Point::OwnerContinueRequested
        };
        let (selected, launch) = pause_watch_or_queue(point, &id);
        let start = app.clone();
        let owner = ctx.clone();
        let target = id.clone();
        let pending = tokio::spawn(async move {
            start
                .continue_task(&owner, target.as_str(), &Default::default())
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), selected.notified()).await?;
        if archive_at_claim {
            app.archive_task(&ctx, id.as_str(), true).await?;
        } else {
            app.continue_task(&ctx, id.as_str(), &Default::default())
                .await?;
            app.close_task_and_wait(&id).await?;
        }
        ask(&app, &id).await?;
        let runs = app.deps.store.attempts(&id).await?.len();
        launch.notify_one();
        let stale = tokio::time::timeout(Duration::from_secs(10), pending).await??;
        assert!(
            matches!(stale, Err(yulius_app::error::AppError::Conflict(_))),
            "{stale:?}"
        );
        let kept = app.deps.store.task(&id).await?.ok_or("closed target")?;
        assert_eq!(kept.status, TaskStatus::Closed);
        assert_eq!(kept.archived_at.is_some(), archive_at_claim);
        assert!(asks(&app, &id).await?);
        assert_eq!(app.deps.store.attempts(&id).await?.len(), runs);
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

#[test]
fn a_direct_resume_cannot_enter_while_a_replacement_reader_settles() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.close_task_and_wait(&id).await?;
        let (prepared, proceed) = pause_watch_or_queue(Point::StartPreflightFinished, &id);
        let starting = app.clone();
        let selected = id.clone();
        let resume = tokio::spawn(async move {
            starting
                .start_run(
                    &selected,
                    yulius_app::supervisor::RunStart::Resume { prompt: None },
                    None,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(10), prepared.notified()).await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        // The replacement is durably closed while the supervisor still holds its process.
        assert!(
            app.deps
                .store
                .transition_task(
                    &id,
                    Some(TaskStatus::Running),
                    TaskStatus::Closed,
                    None,
                    app.now()
                )
                .await?
        );
        let (reaped, exit) = pause_watch_or_queue(Point::RunReaped, &id);
        let mut frames = app.runtime(&id).hub.watch();
        app.stop_task_now(&id).await?;
        tokio::time::timeout(Duration::from_secs(10), reaped.notified()).await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        let account = yulius_domain::ids::AccountId::parse("a-only")?;
        let usage = json!({"primary":{"used_percent":100,"window_minutes":300,
                            "resets_at":app.now().plus_millis(3_600_000).0}})
        .to_string();
        app.deps
            .store
            .set_account_state(
                &account,
                yulius_app::model::AccountStatus::Limited,
                Some(&usage),
                None,
                app.now(),
            )
            .await?;
        proceed.notify_one();
        let late = tokio::time::timeout(Duration::from_secs(10), resume).await??;
        exit.notify_one();
        assert!(
            matches!(late, Err(yulius_app::error::AppError::Conflict(_))),
            "{late:?}"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if matches!(frames.recv().await?, yulius_app::hub::HubFrame::Exit { .. }) {
                    return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
                }
            }
        })
        .await??;
        app.start_run(
            &id,
            yulius_app::supervisor::RunStart::Resume { prompt: None },
            None,
        )
        .await?;
        assert_eq!(
            status_of(&app, &id).await.0,
            Some(TaskStatus::WaitingCapacity)
        );
        assert!(app.deps.store.open_pause(&id).await?.is_some());
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContinuedOwner {
    Queued,
    WaitingCapacity,
    GoalWaitingCapacity,
    SettlementTimeout,
    ArchivedDuringWait,
}

#[test]
fn a_queued_continue_waits_for_the_reaped_park_reader() -> TestResult {
    continue_during_park_settlement(ContinuedOwner::Queued)
}

#[test]
fn a_capacity_delayed_continue_waits_for_the_reaped_park_reader() -> TestResult {
    continue_during_park_settlement(ContinuedOwner::WaitingCapacity)
}

#[test]
fn a_goal_resume_waits_for_the_reaped_park_reader() -> TestResult {
    continue_during_park_settlement(ContinuedOwner::GoalWaitingCapacity)
}

#[test]
fn a_continue_that_exhausts_the_settlement_wait_can_retry_after_exit() -> TestResult {
    continue_during_park_settlement(ContinuedOwner::SettlementTimeout)
}

#[test]
fn a_later_archive_wins_over_a_continue_waiting_for_the_reader() -> TestResult {
    continue_during_park_settlement(ContinuedOwner::ArchivedDuringWait)
}

#[test]
fn a_live_run_refusal_preserves_the_continuing_owners_archive_and_decision() -> TestResult {
    refuse_continuation_of_held_owner(false)
}

#[test]
fn a_run_reaped_during_continue_admission_preserves_the_owners_archive_and_decision() -> TestResult
{
    refuse_continuation_of_held_owner(true)
}

fn refuse_continuation_of_held_owner(reap_during_admission: bool) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let (app, _) = configured_app_with_pulls(&data, 1, 1, |config| {
            config.stop_escalation_ms = 500;
        }).await?;
        let (id, caller) = answered(&app, &data).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("owner")?,
            csrf_token: None, session_hash: None, token: None,
        };
        // SPEC-458 permits a resting task whose process is still held by this server.
        let status = TaskStatus::Stopped(StopReason::ServerLost);
        assert!(app.deps.store.transition_task(&id, Some(TaskStatus::Running), status, None, app.now()).await?);
        assert!(app.deps.store.live_attempt(&id).await?.is_some());
        app.archive_task(&ctx, id.as_str(), true).await?;
        ask(&app, &id).await?;
        let req = serde_json::from_value(json!({}))?;
        let refusal = if reap_during_admission {
            let (prepared, enter) = pause_watch_or_queue(Point::OwnerContinueRequested, &id);
            let (reaped, settle) = pause_watch_or_queue(Point::RunReaped, &id);
            let owner = ctx.clone();
            let stopped_id = id.clone();
            let continuing_app = app.clone();
            let continuing = tokio::spawn(async move {
                continuing_app.continue_task(&owner, stopped_id.as_str(), &req).await
            });
            tokio::time::timeout(Duration::from_secs(10), prepared.notified()).await?;
            app.stop_task_now(&id).await?;
            tokio::time::timeout(Duration::from_secs(10), reaped.notified()).await?;
            assert!(app.deps.store.live_attempt(&id).await?.is_none());
            enter.notify_one();
            let result = tokio::time::timeout(Duration::from_secs(10), continuing).await??;
            settle.notify_one();
            result
        } else {
            app.continue_task(&ctx, id.as_str(), &req).await
        };
        let reason = if reap_during_admission { "settling" } else { "live run" };
        assert!(matches!(refusal, Err(yulius_app::error::AppError::Conflict(ref msg)) if msg.contains(reason)), "{refusal:?}");
        let current = app.deps.store.task(&id).await?.ok_or("stopped owner")?;
        assert_eq!(current.status, status);
        assert!(current.archived_at.is_some(), "refused Continue unarchived the owner");
        assert!(asks(&app, &id).await?);
        if !reap_during_admission {
            app.stop_task_now(&id).await?;
        }
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

fn continue_during_park_settlement(case: ContinuedOwner) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(exercise_continue_during_park(&data, case));
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

async fn exercise_continue_during_park(data: &Path, case: ContinuedOwner) -> TestResult {
    let (app, _) = configured_app_with_pulls(data, 2, 1, |config| {
        if case == ContinuedOwner::SettlementTimeout {
            config.stop_escalation_ms = 500;
        }
    })
    .await?;
    let (id, caller) = answered(&app, data).await?;
    let ctx = AuthContext {
        user: app.deps.store.user(&caller.user).await?.ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    // Establish the occupied slot before holding the old reader. Spawning a second
    // host while that reader is held would spend the race budget on fixture setup.
    let peer = if case == ContinuedOwner::Queued {
        let peer = new_task(
            &app,
            &caller.sandbox,
            &caller.user,
            "Keep this session occupied",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the occupied session", Duration::from_secs(30), || async {
            screen(&app, &peer).contains("ECHO: Keep this session occupied")
        })
        .await?;
        app.pause_task_for_person(&ctx, peer.as_str()).await?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        assert_eq!(app.max_live_sessions(), 1);
        Some(peer)
    } else {
        None
    };
    let mut frames = app.runtime(&id).hub.watch();
    let (ending, release) = pause_watch_or_queue(Point::EndDecided, &id);
    let paused = app.clone();
    let paused_id = id.clone();
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(Point::ParkDecided, Path::new(id.as_str()), move || {
        let result = tokio::task::block_in_place(|| {
            handle.block_on(paused.pause_task(
                &paused_id,
                yulius_app::model::PauseKind::Prompt,
                None,
                None,
            ))
        });
        assert!(result.is_ok(), "{result:?}");
    });
    wait_for(
        "the park with a capacity hold",
        Duration::from_secs(20),
        || async { app.park_idle_sessions_now().await == 1 },
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(10), ending.notified()).await?;
    assert!(app.deps.store.live_attempt(&id).await?.is_none());
    app.stop_task_now(&id).await?;
    let stopped = Some(TaskStatus::Stopped(StopReason::Cancelled));
    assert_eq!(status_of(&app, &id).await.0, stopped);
    if matches!(
        case,
        ContinuedOwner::SettlementTimeout | ContinuedOwner::Queued
    ) {
        app.archive_task(&ctx, id.as_str(), true).await?;
        ask(&app, &id).await?;
    }
    if case != ContinuedOwner::Queued {
        let account = yulius_domain::ids::AccountId::parse("a-only")?;
        let usage = json!({
            "primary": {
                "used_percent": 100.0,
                "window_minutes": 300,
                "resets_at": app.now().plus_millis(3_600_000).0
            }
        })
        .to_string();
        app.deps
            .store
            .set_account_state(
                &account,
                yulius_app::model::AccountStatus::Limited,
                Some(&usage),
                None,
                app.now(),
            )
            .await?;
    }
    let waiting = Arc::new(tokio::sync::Notify::new());
    let ready = waiting.clone();
    yulius_app::race::on(
        Point::ContinueWaitsForReader,
        Path::new(id.as_str()),
        move || ready.notify_one(),
    );
    let continuing_app = app.clone();
    let continuing_id = id.clone();
    let owner = ctx.clone();
    let req = serde_json::from_value(json!({}))?;
    let mut continuing = tokio::spawn(async move {
        if matches!(
            case,
            ContinuedOwner::WaitingCapacity | ContinuedOwner::GoalWaitingCapacity
        ) {
            // Restart and onboarding also resume directly through the supervisor.
            let start = if case == ContinuedOwner::GoalWaitingCapacity {
                yulius_app::supervisor::RunStart::GoalResume {
                    prompt: "Finish the remaining steps".into(),
                }
            } else {
                yulius_app::supervisor::RunStart::Resume { prompt: None }
            };
            continuing_app.start_run(&continuing_id, start, None).await
        } else {
            continuing_app
                .continue_task(&owner, continuing_id.as_str(), &req)
                .await
                .map(drop)
        }
    });
    tokio::select! {
        early = &mut continuing => {
            release.notify_one();
            let result = early?;
            return Err(format!("Continue completed before its reaped reader settled: {result:?}").into());
        },
        () = waiting.notified() => (),
    }
    assert_eq!(status_of(&app, &id).await.0, stopped);
    if case == ContinuedOwner::ArchivedDuringWait {
        app.archive_task(&ctx, id.as_str(), true).await?;
        ask(&app, &id).await?;
    }
    if case == ContinuedOwner::SettlementTimeout {
        let result = tokio::time::timeout(Duration::from_secs(5), &mut continuing).await??;
        assert!(matches!(
            result,
            Err(yulius_app::error::AppError::Conflict(reason)) if reason.contains("settling")
        ));
        assert_eq!(status_of(&app, &id).await.0, stopped);
        assert!(
            app.deps
                .store
                .task(&id)
                .await?
                .ok_or("task")?
                .archived_at
                .is_some(),
            "a timed-out Continue unarchived the stopped task"
        );
        assert!(
            asks(&app, &id).await?,
            "a timed-out Continue cleared the person's decision"
        );
    }
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if matches!(frames.recv().await?, yulius_app::hub::HubFrame::Exit { .. }) {
                return Ok::<(), tokio::sync::broadcast::error::RecvError>(());
            }
        }
    })
    .await??;
    if case == ContinuedOwner::ArchivedDuringWait {
        let result = continuing.await?;
        assert!(
            matches!(result, Err(yulius_app::error::AppError::Conflict(_))),
            "Continue passed a later Archive: {result:?}"
        );
        let owner = app.deps.store.task(&id).await?.ok_or("archived owner")?;
        assert_eq!(Some(owner.status), stopped);
        assert!(owner.archived_at.is_some());
        assert!(asks(&app, &id).await?);
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        return Ok(());
    }
    if case == ContinuedOwner::SettlementTimeout {
        app.continue_task(&ctx, id.as_str(), &serde_json::from_value(json!({}))?)
            .await?;
        assert!(
            app.deps
                .store
                .task(&id)
                .await?
                .ok_or("task")?
                .archived_at
                .is_none()
        );
        assert!(!asks(&app, &id).await?);
    } else {
        continuing.await??;
    }
    let state = if peer.is_some() {
        TaskStatus::Queued
    } else {
        TaskStatus::WaitingCapacity
    };
    assert_eq!(status_of(&app, &id).await.0, Some(state));
    assert!(app.deps.store.live_attempt(&id).await?.is_none());
    if peer.is_some() {
        let accepted = app.deps.store.task(&id).await?.ok_or("queued owner")?;
        assert!(accepted.archived_at.is_none());
        assert!(!asks(&app, &id).await?);
    }
    app.stop_task_now(&id).await?;
    if let Some(peer) = peer {
        app.stop_task_now(&peer).await?;
    }
    Ok(())
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
pub(crate) async fn ask(app: &App, id: &TaskId) -> TestResult {
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

#[test]
fn a_parked_message_can_retry_after_screen_publication_times_out() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let (app, _) = configured_app_with_pulls(&data, 8, 1, |config| {
            config.stop_escalation_ms = 500;
        })
        .await?;
        let (id, caller) = answered(&app, &data).await?;
        ask(&app, &id).await?;
        let (recorded, restore) = pause_watch_or_queue(Point::ParkStateRecorded, &id);
        request_one_park(&app).await?;
        tokio::time::timeout(Duration::from_secs(10), recorded.notified()).await?;
        let prompt = json!({"number": 1, "text": "Retry the saved turn"});
        let expired = tokio::time::timeout(
            Duration::from_secs(5),
            app.tool_task_message(&caller, &prompt),
        )
        .await?;
        let still_asks = asks(&app, &id).await?;
        let attempts = app.deps.store.attempts(&id).await?.len();
        restore.notify_one();
        assert!(matches!(
            expired,
            Err(yulius_app::error::AppError::Conflict(_))
        ));
        assert_eq!(attempts, 1);
        assert!(still_asks);
        assert_eq!(
            app.tool_task_message(&caller, &prompt).await?["delivered"],
            "resumed"
        );
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

#[test]
fn a_parked_message_checked_on_an_earlier_run_is_refused_during_screen_restore() -> TestResult {
    replaced_parked_message(Point::ParkedResumePrepared, true)
}

#[test]
fn a_parked_message_checked_on_an_earlier_run_is_refused_after_settlement() -> TestResult {
    replaced_parked_message(Point::ParkedResumePrepared, false)
}

#[test]
fn a_message_selected_before_preflight_cannot_answer_a_later_park() -> TestResult {
    replaced_parked_message(Point::ParkedMessageSelected, false)
}
fn replaced_parked_message(checkpoint: Point, while_restoring: bool) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        park_through_terminal_exit(&app, &id).await?;
        let (prepared, submit) = pause_watch_or_queue(checkpoint, &id);
        let first = app.clone();
        let first_caller = caller.clone();
        let mut first = tokio::spawn(async move {
            first
                .tool_task_message(
                    &first_caller,
                    &json!({"number": 1, "text": "Old pending message"}),
                )
                .await
        });
        tokio::select! {
            result = &mut first => return Err(format!("message finished before {checkpoint:?}: {result:?}").into()),
            reached = tokio::time::timeout(Duration::from_secs(30), prepared.notified()) =>
                reached.map_err(|_| format!("message did not reach {checkpoint:?}"))?,
        }
        let accepted = app
            .tool_task_message(&caller, &json!({"number": 1, "text": "Intervening turn"}))
            .await?;
        assert_eq!(accepted["delivered"], "resumed");
        wait_for(
            "the intervening answer",
            Duration::from_secs(20),
            || async {
                screen(&app, &id).contains("Intervening turn")
                    && status_of(&app, &id).await.1 == Some(Activity::Waiting)
            },
        )
        .await?;
        let restore = if while_restoring {
            let (recorded, restore) = pause_watch_or_queue(Point::ParkStateRecorded, &id);
            request_one_park(&app).await?;
            tokio::time::timeout(Duration::from_secs(15), recorded.notified()).await?;
            Some(restore)
        } else {
            park_through_terminal_exit(&app, &id).await?;
            None
        };
        ask(&app, &id).await?;
        submit.notify_one();
        let stale = tokio::time::timeout(Duration::from_secs(10), first).await??;
        assert!(
            matches!(stale, Err(yulius_app::error::AppError::Conflict(_))),
            "message prepared for the old park was accepted"
        );
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 2);
        assert!(asks(&app, &id).await?);
        if let Some(restore) = restore {
            restore.notify_one();
        }
        let fresh = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Message for the latest park"}),
            )
            .await?;
        assert_eq!(fresh["delivered"], "resumed");
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

async fn park_through_terminal_exit(app: &Arc<App>, id: &TaskId) -> TestResult {
    let mut frames = app.runtime(id).hub.watch();
    request_one_park(app).await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if matches!(frames.recv().await?, yulius_app::hub::HubFrame::Exit { .. }) {
                return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
            }
        }
    })
    .await
    .map_err(|_| "the accepted park did not publish its terminal exit")??;
    assert_eq!(status_of(app, id).await.0, Some(TaskStatus::Parked));
    Ok(())
}

async fn request_one_park(app: &Arc<App>) -> TestResult {
    // Issue one successful park request, then observe its completion. Looking for the final
    // status by running more parking passes could overlap this run's exit reader.
    wait_for("the parking request", Duration::from_secs(30), || async {
        app.park_idle_sessions_now().await == 1
    })
    .await?;
    Ok(())
}

#[test]
fn a_parked_message_resumes_while_its_successor_checks_the_pull_request() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let (app, github) = configured_app_with_pulls(&data, 8, 1, |config| {
            config.stop_escalation_ms = 500;
        })
        .await?;
        let (id, caller) = answered(&app, &data).await?;
        let (recorded, publish) = pause_watch_or_queue(Point::ParkStateRecorded, &id);
        request_one_park(&app).await?;
        tokio::time::timeout(Duration::from_secs(10), recorded.notified()).await?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        open_pull_at(&app, &github, &id, 1481, "0123456789abcdef").await?;
        let mut next = app.deps.store.task(&id).await?.ok_or("parked owner")?;
        next.id = TaskId::parse("t-next")?;
        next.number = 99;
        next.status = TaskStatus::Chained;
        next.activity = None;
        next.branch = None;
        next.worktree_path = None;
        app.deps.store.insert_task(&next).await?;
        yulius_app::ports::store_ext::IntakeStore::insert_chain(
            app.deps.store.as_ref(),
            &yulius_app::intake::ChainRecord {
                task_id: next.id.clone(),
                after_task_id: Some(id.clone()),
                after_number: 1,
                blocked_reason: None,
                created_at: app.now(),
                rule: None,
            },
        )
        .await?;
        let (checking, release) = pause_watch_or_queue(Point::ParkedPredecessorChecksPr, &id);
        publish.notify_one();
        tokio::time::timeout(Duration::from_secs(12), checking.notified()).await?;
        let message = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Read this while the successor waits"}),
            )
            .await;
        release.notify_one();
        assert_eq!(message?["delivered"], "resumed");
        assert_eq!(status_of(&app, &next.id).await.0, Some(TaskStatus::Chained));
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
}

#[test]
fn a_parked_resume_waits_for_the_exit_reader_that_lost_to_escalation() -> TestResult {
    losing_exit_reader(false)
}

#[test]
fn a_late_exit_reader_preserves_the_lost_run_screen_after_resume() -> TestResult {
    losing_exit_reader(true)
}

fn losing_exit_reader(before_finalization: bool) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let result = rt.block_on(async {
        let (app, _) = configured_app_with_pulls(&data, 8, 1, |config| {
            config.stop_escalation_ms = 1000;
        })
        .await?;
        let (id, caller) = answered(&app, &data).await?;
        let old = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("the first run")?;
        let (point, key) = if before_finalization {
            (Point::HostExitReceived, old.id.as_str())
        } else {
            (Point::RunEndTaken, id.as_str())
        };
        let (entered, finish) = pause_at_key(point, key);
        let finished = Arc::new(tokio::sync::Notify::new());
        let done = finished.clone();
        yulius_app::race::on(
            Point::HostExitFinalized,
            Path::new(old.id.as_str()),
            move || done.notify_one(),
        );
        let mut frames = app.runtime(&id).hub.watch();
        request_one_park(&app).await?;
        tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if matches!(frames.recv().await?, yulius_app::hub::HubFrame::Exit { .. }) {
                    return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
                }
            }
        })
        .await??;
        assert_eq!(
            app.deps
                .store
                .attempt(&old.id)
                .await?
                .ok_or("lost run")?
                .exit_reason
                .as_deref(),
            Some("lost")
        );
        let saved = Path::new(&old.run_dir).join("final-screen");
        let original = std::fs::read(&saved)?;
        assert!(String::from_utf8_lossy(&original).contains("Budget"));
        let target = app.clone();
        let first_caller = caller.clone();
        let mut message = tokio::spawn(async move {
            target
                .tool_task_message(
                    &first_caller,
                    &json!({"number":1,"text":"Text for the second run only"}),
                )
                .await
        });
        let accepted = if before_finalization {
            let accepted = (&mut message).await??;
            wait_for(
                "the second run's answer",
                Duration::from_secs(10),
                || async { screen(&app, &id).contains("Text for the second run only") },
            )
            .await?;
            let replacement = app
                .deps
                .store
                .live_attempt(&id)
                .await?
                .ok_or("replacement run")?;
            let (exiting, release_exit) =
                pause_at_key(Point::HostExitReceived, replacement.id.as_str());
            let replacement_finished = Arc::new(tokio::sync::Notify::new());
            let signal = replacement_finished.clone();
            yulius_app::race::on(
                Point::HostExitFinalized,
                Path::new(replacement.id.as_str()),
                move || signal.notify_one(),
            );
            request_one_park(&app).await?;
            tokio::time::timeout(Duration::from_secs(10), exiting.notified()).await?;
            finish.notify_one();
            tokio::time::timeout(Duration::from_secs(10), finished.notified()).await?;
            release_exit.notify_one();
            tokio::time::timeout(Duration::from_secs(10), replacement_finished.notified()).await?;
            let settled = app
                .deps
                .store
                .attempt(&replacement.id)
                .await?
                .ok_or("settled replacement")?;
            assert_eq!(
                settled.exit_reason.as_deref(),
                Some("parked"),
                "the late reader consumed the replacement's park request"
            );
            let parked = app
                .deps
                .store
                .task(&id)
                .await?
                .ok_or("parked replacement")?;
            assert_eq!(parked.resume_attempt_id.as_ref(), Some(&replacement.id));
            assert!(screen(&app, &id).contains("Text for the second run only"));
            let capture = std::fs::read(Path::new(&replacement.run_dir).join("parked-screen"))?;
            assert!(String::from_utf8_lossy(&capture).contains("Text for the second run only"));
            accepted
        } else {
            // The losing exit reader is still using the old session. A bounded
            // conflict leaves the request pending and a subsequent message resumes it.
            let response = (&mut message).await?;
            finish.notify_one();
            assert!(
                matches!(response, Err(yulius_app::error::AppError::Conflict(_))),
                "a new turn started while the losing exit reader held the old run"
            );
            tokio::time::timeout(Duration::from_secs(10), finished.notified()).await?;
            app.tool_task_message(
                &caller,
                &json!({"number":1, "text":"Text for the second run only"}),
            )
            .await?
        };
        assert_eq!(accepted["delivered"], "resumed");
        assert_eq!(
            std::fs::read(&saved)?,
            original,
            "the stale exit reader rewrote the lost run's saved screen"
        );
        if !before_finalization {
            app.stop_task_now(&id).await?;
        }
        Ok::<_, Error>(())
    });
    rt.shutdown_timeout(Duration::from_secs(5));
    result
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
        let (recorded, restore_screen) = pause_watch_or_queue(Point::ParkStateRecorded, &id);
        request_one_park(&app).await?;
        tokio::time::timeout(Duration::from_secs(10), recorded.notified()).await?;
        assert!(asks(&app, &id).await?, "the request stays");
        let waiting = Arc::new(tokio::sync::Notify::new());
        let ready = waiting.clone();
        yulius_app::race::on(Point::ParkedResumeWaitsForReader, Path::new(id.as_str()), move || ready.notify_one());
        let resumed = app.clone();
        let mut message = tokio::spawn(async move {
            resumed.tool_task_message(&caller, &json!({"number": 1, "text": "Blue: answer with an echo"})).await
        });
        tokio::select! {
            early = &mut message => {
                restore_screen.notify_one();
                return Err(format!("message resumed the task before its old reader published the parked screen: {:?}", early?).into());
            }
            () = waiting.notified() => (),
        }
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        restore_screen.notify_one();
        let answer = message.await??;
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

/// PRK-003 (#1605): a second idle-park pass whose process probe saw the session alive, and that
/// gets the launch lock only after the first park's run end took the park's intent (the task
/// still reads Running), parks nothing. A message in that interval is refused as parking, never
/// typed into the ending session; the next one, once the task is Parked, resumes it in the same
/// conversation, once.
#[test]
fn a_second_park_pass_at_the_runs_end_leaves_the_parked_task_open_to_a_message() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        past_the_grace(&app, &id).await?;
        // The first pass decides to park and holds the launch lock; its session still runs.
        let (decided, shut_down) = pause_watch_or_queue(Point::ParkDecided, &id);
        let first = tokio::spawn({
            let app = app.clone();
            async move { app.park_idle_sessions_now().await }
        });
        tokio::time::timeout(Duration::from_secs(30), decided.notified()).await?;
        // The second pass probes the live session, then waits for the launch lock.
        let probed = Arc::new(tokio::sync::Notify::new());
        {
            let probed = probed.clone();
            yulius_app::race::on(Point::ParkProbed, Path::new(id.as_str()), move || {
                probed.notify_one();
            });
        }
        let (prepared, claim) = pause_watch_or_queue(Point::ParkPrepared, &id);
        let (taken, record) = pause_watch_or_queue(Point::RunEndTaken, &id);
        let second = tokio::spawn({
            let app = app.clone();
            async move { app.park_idle_sessions_now().await }
        });
        tokio::time::timeout(Duration::from_secs(30), probed.notified()).await?;
        // The first park ends the session; its run's end takes the park's intent while the
        // second pass reaches its claim.
        shut_down.notify_one();
        assert_eq!(first.await?, 1);
        tokio::time::timeout(Duration::from_secs(30), taken.notified()).await?;
        tokio::time::timeout(Duration::from_secs(30), prepared.notified()).await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));
        claim.notify_one();
        let again = tokio::time::timeout(Duration::from_secs(30), second).await??;
        let early = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Early: answer with an echo"}),
            )
            .await;
        record.notify_one();
        assert_eq!(again, 0, "a second pass parked the ending run again");
        assert!(
            matches!(&early, Err(yulius_app::error::AppError::Conflict(why))
                if why.contains("is parking its idle session")),
            "{early:?}"
        );
        wait_for("the task to park", Duration::from_secs(30), || async {
            status_of(&app, &id).await.0 == Some(TaskStatus::Parked)
        })
        .await?;
        let resumed = app
            .tool_task_message(
                &caller,
                &json!({"number": 1, "text": "Late: answer with an echo"}),
            )
            .await?;
        assert_eq!(resumed["delivered"], "resumed");
        wait_for("the resumed answer", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Late: answer with an echo")
        })
        .await?;
        // Counted once the resumed turn ended, so a later second delivery would show.
        wait_for(
            "the resumed turn's end",
            Duration::from_secs(30),
            || async {
                status_of(&app, &id).await == (Some(TaskStatus::Running), Some(Activity::Waiting))
            },
        )
        .await?;
        let resumed = screen(&app, &id);
        assert_eq!(
            resumed.matches("Late: answer with an echo").count(),
            1,
            "{resumed}"
        );
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[1].launch_kind, LaunchKind::Resume);
        assert_eq!(runs[0].provider_session_id, runs[1].provider_session_id);
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
        ask(&app, &id).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute_batch("ALTER TABLE tasks RENAME COLUMN needs_you TO temporarily_unreadable")?;
        app.advance_chains(&id).await;
        assert_eq!(
            status_of(&app, &waiting.id).await.0,
            Some(TaskStatus::Chained),
            "unreadable owner decisions must hold the successor"
        );
        db.execute_batch("ALTER TABLE tasks RENAME COLUMN temporarily_unreadable TO needs_you")?;
        app.advance_chains(&id).await;
        assert_eq!(
            status_of(&app, &waiting.id).await.0,
            Some(TaskStatus::Chained)
        );
        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
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
    app: &Arc<App>,
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
    // A real park wakes the queue asynchronously. Reconstruct the cold states
    // on a consistent copy so that drain cannot launch the synthetic queued run.
    let (_cold_dir, cold_data) = data_dir()?;
    yulius_store::SqliteStore::open(&data.join("yulius.db"))?
        .backup_to(&cold_data.join("yulius.db"))
        .await?;
    let cold = self::app(&cold_data, 8, 1).await?;
    assert!(
        cold.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Parked),
                TaskStatus::Queued,
                None,
                cold.now()
            )
            .await?
    );
    // Force the live queue pass that previously resumed the synthetic queued task.
    app.drain_queue_now().await;
    assert_eq!(status_of(&cold, id).await.0, Some(TaskStatus::Queued));
    // Loss and normal exit can save identical bytes in this fake agent. Make
    // the ordinary final screen distinct to prove which snapshot each reader uses.
    let last = cold
        .deps
        .store
        .attempts(id)
        .await?
        .pop()
        .ok_or("parked attempt")?;
    let dir = Path::new(&last.run_dir);
    std::fs::write(dir.join("final-screen"), b"80 24\nShutdown screen")?;
    assert_cold_screen(&cold_data, id, caller, &captured).await?;
    // A restart in the gap between the state commit and the filename change.
    std::fs::rename(dir.join("parked-screen"), dir.join("parked-screen.pending"))?;
    assert_cold_screen(&cold_data, id, caller, &captured).await?;
    let renamed = dir.to_owned();
    yulius_app::race::on(yulius_app::race::Point::ParkScreenRead, dir, move || {
        let published = std::fs::rename(
            renamed.join("parked-screen.pending"),
            renamed.join("parked-screen"),
        );
        assert!(published.is_ok(), "{published:?}");
    });
    assert_cold_screen(&cold_data, id, caller, &captured).await?;
    // Starting is recorded before the provider acquires its account.
    assert!(
        cold.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Queued),
                TaskStatus::Starting,
                None,
                cold.now()
            )
            .await?
    );
    assert_cold_screen(&cold_data, id, caller, &captured).await?;
    // The resumed launch has no subscription available, so it waits without a new attempt.
    assert!(
        cold.deps
            .store
            .transition_task(
                id,
                Some(TaskStatus::Starting),
                TaskStatus::WaitingCapacity,
                None,
                cold.now()
            )
            .await?
    );
    assert_cold_screen(&cold_data, id, caller, &captured).await?;
    legacy_queued_park_screen(&cold_data, id, caller, dir, &last).await
}

/// A run parked before the snapshot existed, its task already queued again: only the
/// recorded park transition says the final screen was a park (SPEC-621 FIN-001).
async fn legacy_queued_park_screen(
    data: &Path,
    id: &TaskId,
    caller: &ProjectCaller,
    dir: &Path,
    last: &yulius_app::model::AttemptRecord,
) -> TestResult {
    std::fs::remove_file(dir.join("parked-screen"))?;
    let footer = "Token usage: total=389,557\r\nTo continue this session, run:\r\n  codex resume 01a1f59f-f713-7aa2-a183-1466175f06b5\r\nOr run codex resume and select Budget";
    std::fs::write(
        dir.join("final-screen"),
        format!("80 24\nThe budget answer\r\n{footer}"),
    )?;
    // Only Codex prints that block when it exits: another harness's screen keeps it.
    assert_ne!(last.harness, yulius_domain::harness::Harness::Codex);
    let kept = cold_task_screen(data, id, caller).await?;
    assert!(kept.starts_with("The budget answer"), "{kept}");
    assert!(kept.contains("codex resume 01a1f59f"), "{kept}");
    assert_eq!(
        kept.matches("Yulius parked this session").count(),
        1,
        "{kept}"
    );
    rusqlite::Connection::open(data.join("yulius.db"))?.execute(
        "UPDATE attempts SET harness = 'codex' WHERE id = ?1",
        [last.id.as_str()],
    )?;
    let recovered = cold_task_screen(data, id, caller).await?;
    assert!(recovered.starts_with("The budget answer"), "{recovered}");
    assert!(
        !recovered.contains("codex resume") && !recovered.contains("Token usage"),
        "{recovered}"
    );
    assert_eq!(
        recovered.matches("Yulius parked this session").count(),
        1,
        "{recovered}"
    );
    Ok(())
}

async fn cold_task_screen(
    data: &Path,
    id: &TaskId,
    caller: &ProjectCaller,
) -> Result<String, Error> {
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
    let remote = yulius_app::remote::RemoteCaller {
        ctx,
        token_id: "legacy-screen".into(),
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
    Ok(crate::support::tool_text(&result)["screen"]
        .as_str()
        .ok_or("screen")?
        .to_owned())
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

#[derive(Clone, Copy)]
enum EndRequests {
    Close,
    Cancel,
    CloseThenCancel,
}

/// SPEC-563: escalation records the loss with the reader held just before or just after it
/// takes the request. EndDecided releases the reader once the loss has consumed the intent.
fn host_loss_with_request(
    requests: EndRequests,
    hold_reader_at: Point,
    expected: TaskStatus,
) -> TestResult {
    use yulius_app::race::on;
    use yulius_domain::task::AttemptState;

    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
        let agent = FakeAgentOptions {
            answers_first_message: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, Arc::default(), |config| {
            config.stop_escalation_ms = 1;
        })
        .await?;
        let (id, _) = answered(&app, &data).await?;
        let key = Path::new(id.as_str());
        let bound = Duration::from_secs(30);
        let (reader_ready, wait_reader) = tokio::sync::oneshot::channel();
        let (escalate, wait_escalate) = std::sync::mpsc::channel();
        let (lost, wait_lost) = std::sync::mpsc::channel();
        let (ordered, observed) = tokio::sync::oneshot::channel();
        on(Point::EndEscalating, key, move || {
            assert!(
                tokio::task::block_in_place(|| wait_escalate.recv_timeout(bound)).is_ok(),
                "reader reached the selected race point"
            );
        });
        on(hold_reader_at, key, move || {
            let held = reader_ready.send(()).is_ok()
                && tokio::task::block_in_place(|| wait_lost.recv_timeout(bound)).is_ok();
            let _ = ordered.send(held);
        });
        on(Point::EndDecided, key, move || {
            let _ = lost.send(());
        });
        match requests {
            EndRequests::Close | EndRequests::CloseThenCancel => app.close_task_now(&id).await?,
            EndRequests::Cancel => app.stop_task_now(&id).await?,
        }
        tokio::time::timeout(bound, wait_reader).await??;
        if matches!(requests, EndRequests::CloseThenCancel) {
            // Close is held by the reader. The newer Cancel is in the live intent slot.
            app.stop_task_now(&id).await?;
        }
        escalate.send(())?;
        assert!(
            tokio::time::timeout(bound, observed).await??,
            "reader held until loss"
        );
        wait_for("the task to settle after loss", bound, || async {
            status_of(&app, &id)
                .await
                .0
                .is_some_and(TaskStatus::is_resting)
        })
        .await?;
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, AttemptState::Lost);
        assert_eq!(runs[0].exit_reason.as_deref(), Some("lost"));
        assert_eq!(status_of(&app, &id).await.0, Some(expected));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn host_loss_while_the_reader_holds_close_closes_an_answered_task() -> TestResult {
    host_loss_with_request(EndRequests::Close, Point::RunEndTaken, TaskStatus::Closed)
}

#[test]
fn host_loss_while_the_reader_holds_cancel_stops_an_answered_task() -> TestResult {
    host_loss_with_request(
        EndRequests::Cancel,
        Point::RunEndTaken,
        TaskStatus::Stopped(StopReason::Cancelled),
    )
}

#[test]
fn host_loss_before_the_reader_takes_close_keeps_the_loss_outcome() -> TestResult {
    host_loss_with_request(
        EndRequests::Close,
        Point::RunEndTaking,
        TaskStatus::Stopped(StopReason::Lost),
    )
}

#[test]
fn host_loss_before_the_reader_takes_cancel_keeps_the_loss_outcome() -> TestResult {
    host_loss_with_request(
        EndRequests::Cancel,
        Point::RunEndTaking,
        TaskStatus::Stopped(StopReason::Lost),
    )
}

#[test]
fn host_loss_with_a_newer_cancel_uses_the_live_intent_slot() -> TestResult {
    host_loss_with_request(
        EndRequests::CloseThenCancel,
        Point::RunEndTaken,
        TaskStatus::Stopped(StopReason::Lost),
    )
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
        // Both a person request and an achieved activity can take precedence in the display.
        ask(&app, &id).await?;
        app.deps
            .store
            .transition_task(
                &id,
                Some(TaskStatus::Running),
                TaskStatus::Running,
                Some(Activity::Done),
                app.now(),
            )
            .await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "a human request cannot hide the unfinished spec"
        );
        app.deps.store.set_spec_status(&id, Some("done")).await?;
        parked(&app, &id).await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn an_unreadable_recorded_spec_keeps_the_session_through_digest_parking() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        app.deps
            .store
            .set_spec_status(&id, Some("in-progress"))
            .await?;
        let store = yulius_store::SqliteStore::open(&data.join("yulius.db"))?;
        store
            .execute_for_tests(
                "ALTER TABLE tasks RENAME COLUMN spec_status TO unavailable_spec_status",
            )
            .await?;
        assert!(app.deps.store.spec_status(&id).await.is_err());
        past_the_grace(&app, &id).await?;
        assert_eq!(app.send_digests_now(Some(1)).await.parked, 0);
        assert_eq!(app.park_idle_sessions_now().await, 0);
        store
            .execute_for_tests(
                "ALTER TABLE tasks RENAME COLUMN unavailable_spec_status TO spec_status",
            )
            .await?;
        assert_eq!(
            app.deps.store.spec_status(&id).await?.as_deref(),
            Some("in-progress")
        );
        app.deps
            .store
            .set_spec_status(&id, Some("unknown-status"))
            .await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        app.deps.store.set_spec_status(&id, Some("done")).await?;
        parked(&app, &id).await?;
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

/// SPEC-553 PRK-102: ordinary owner terminal input and `continue_task_with_prompt` wake a parked
/// task through the supervisor preserving its exact `provider_session_id` and `worktree_path`,
/// while bare interrupts/scroll, non-owners, and operator-stopped tasks are never woken.
#[test]
fn ordinary_owner_terminal_input_and_continue_prompt_wake_parked_session_with_continuity()
-> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let first_runs = app.deps.store.attempts(&id).await?;
        let session_id = first_runs[0].provider_session_id.clone();
        assert!(session_id.is_some(), "provider_session_id recorded");
        let worktree = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?
            .worktree_path;
        assert!(worktree.is_some(), "worktree recorded");
        let owner_ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };

        parked(&app, &id).await?;
        let attach = app.terminal_open(&owner_ctx, id.as_str(), None).await?;
        assert!(attach.can_write);

        // Scroll and bare Ctrl-C must NOT wake the parked session.
        app.terminal_input(&owner_ctx, &attach, b"a", true).await?;
        app.terminal_input(&owner_ctx, &attach, b"\x03", false)
            .await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));

        // Ownership boundary: a non-owner with sandbox write access MUST NOT wake via terminal.
        let bob = yulius_app::model::UserRecord {
            id: yulius_domain::ids::UserId::parse("u-bob-term")?,
            github_login: Some("bob-term".into()),
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
        let bob_ctx = AuthContext {
            user: bob,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let bob_attach = app.terminal_open(&bob_ctx, id.as_str(), None).await?;
        assert!(!bob_attach.can_write);
        assert!(
            app.terminal_input(&bob_ctx, &bob_attach, b"bob wake\r", false)
                .await
                .is_err(),
            "non-owner terminal input must be refused"
        );
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));

        // Ordinary submitted owner typing wakes the parked session with the exact rollout/worktree.
        app.terminal_input(
            &owner_ctx,
            &attach,
            b"Wake from terminal: answer with an echo\r",
            false,
        )
        .await?;
        wait_for("terminal wake echo", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Wake from terminal: answer with an echo")
        })
        .await?;
        let runs_after_term = app.deps.store.attempts(&id).await?;
        assert_eq!(runs_after_term.len(), 2);
        assert_eq!(runs_after_term[1].launch_kind, LaunchKind::Resume);
        assert_eq!(runs_after_term[1].provider_session_id, session_id);
        assert_eq!(
            TaskStore::task(app.deps.store.as_ref(), &id)
                .await?
                .ok_or("task")?
                .worktree_path,
            worktree
        );

        // Park again and verify oversized ContinueTask.prompt (> 64 KiB) is rejected with Invalid
        // while keeping the task Parked, then wake cleanly via `continue_task` with a bounded prompt.
        wait_for("idle again", Duration::from_secs(30), || async {
            status_of(&app, &id).await.1 == Some(Activity::Waiting)
        })
        .await?;
        parked(&app, &id).await?;
        let oversized = "x".repeat(yulius_app::task_message::MAX_MESSAGE_BYTES + 1);
        let too_big = app
            .continue_task(
                &owner_ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: None,
                    prompt: Some(oversized),
                },
            )
            .await;
        assert!(
            matches!(too_big, Err(yulius_app::error::AppError::Invalid(_))),
            "oversized prompt must return Invalid: {too_big:?}"
        );
        assert_eq!(
            status_of(&app, &id).await.0,
            Some(TaskStatus::Parked),
            "rejected prompt must keep task parked"
        );
        app.continue_task(
            &owner_ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: None,
                prompt: Some("Wake from reply: answer with an echo".into()),
            },
        )
        .await?;
        wait_for("reply wake echo", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: Wake from reply: answer with an echo")
        })
        .await?;
        let runs_after_reply = app.deps.store.attempts(&id).await?;
        assert_eq!(runs_after_reply.len(), 3);
        assert_eq!(runs_after_reply[2].launch_kind, LaunchKind::Resume);
        assert_eq!(runs_after_reply[2].provider_session_id, session_id);

        // Operator stop boundary: an explicitly stopped task MUST NOT wake on terminal input.
        app.stop_task_now(&id).await?;
        wait_for("stopped", Duration::from_secs(30), || async {
            matches!(status_of(&app, &id).await.0, Some(TaskStatus::Stopped(_)))
        })
        .await?;
        let stopped_err = app
            .terminal_input(&owner_ctx, &attach, b"should not wake\r", false)
            .await;
        assert!(
            stopped_err.is_err(),
            "operator-stopped task must refuse terminal input instead of waking"
        );
        assert!(matches!(
            status_of(&app, &id).await.0,
            Some(TaskStatus::Stopped(_))
        ));
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// #325: initial pending checks become a durable failure before digest parking.
#[test]
fn a_ci_failure_before_digest_parking_reaches_the_idle_author_once() -> TestResult {
    use yulius_app::ci::CiRunRecord;
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::ports::github::ChecksState;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::delivery::DeliveryStage;
    use yulius_domain::time::Timestamp;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let wt = Path::new(task.worktree_path.as_deref().ok_or("worktree")?);
        let sha = app
            .deps
            .git
            .resolve_commit(wt, "HEAD")
            .await?
            .ok_or("head")?;
        open_pull_at(&app, &github, &id, 1461, &sha).await?;
        app.deps
            .store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: id.clone(),
                    stage: DeliveryStage::PrOpen,
                    pr_number: Some(1461),
                    updated_at: app.now(),
                },
            )
            .await?;
        github.set_checks(&sha, ChecksState::Pending);
        let pending = CiRunRecord {
            id: "before-digest".into(),
            sandbox_id: caller.sandbox.clone(),
            reference: CiRef::PullRequest(1461),
            sha: sha.clone(),
            source: GateSource::ProjectConfig,
            state: RunState::Running,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(10),
            started_at: Some(Timestamp(11)),
            ended_at: None,
        };
        app.deps.store.insert_ci_run(&pending, &[]).await?;
        let obsolete = CiRunRecord {
            id: "obsolete-train".into(),
            reference: CiRef::Train(vec![1461]),
            sha: "obsolete-candidate-sha".into(),
            state: RunState::Failed,
            queued_at: Timestamp(9),
            ended_at: Some(Timestamp(15)),
            ..pending.clone()
        };
        app.deps.store.insert_ci_run(&obsolete, &[]).await?;
        app.tool_task_message(
            &caller,
            &json!({"number": 1, "text":"CI pending: answer with an echo"}),
        )
        .await?;
        wait_for(
            "the author to finish its turn while CI is pending",
            Duration::from_secs(30),
            || async {
                let answer = screen(&app, &id);
                answer.contains("ECHO: Message from admin")
                    && answer.contains("CI pending: answer with an echo")
                    && status_of(&app, &id).await.1 == Some(Activity::Waiting)
            },
        )
        .await?;
        assert!(
            app.deps
                .store
                .transition_ci_run(
                    &pending.id,
                    &[RunState::Running],
                    RunState::Failed,
                    &yulius_app::ci::CiRunUpdate {
                        ended_at: Some(Timestamp(22)),
                        ..Default::default()
                    }
                )
                .await?
        );
        // The remote result still looks green from the previous check.
        github.set_checks(&sha, ChecksState::Success);
        ask(&app, &id).await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        let bounds = WatchBounds::default();
        assert!(app.watch_prs(&bounds).await.is_empty());
        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
            .await?;
        assert_eq!(app.send_digests_now(Some(1)).await.parked, 0);
        let notices = app.watch_prs(&bounds).await;
        assert_eq!(
            notices
                .iter()
                .filter(|(task, text)| task == &id && text.contains("before-digest"))
                .count(),
            1,
            "{notices:?}"
        );
        assert!(app.watch_prs(&bounds).await.is_empty());
        assert!(
            notices
                .iter()
                .all(|(_, text)| !text.contains("obsolete-train"))
        );
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-553 PRK-103: an idle task continues to park safely while CI runs on its open PR, does not
/// wake on either a running check or a settled check (SPEC-612).
#[test]
fn parked_task_stays_parked_when_its_pull_request_checks_finish() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::delivery::DeliveryStage;
    use yulius_domain::resources::Need;
    use yulius_domain::time::Timestamp;

    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(42),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;

        // Active CI run on PR #42: safe parking while CI runs is preserved.
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let wt = Path::new(task.worktree_path.as_deref().ok_or("worktree")?);
        let initial_head = app
            .deps
            .git
            .resolve_commit(wt, "HEAD")
            .await?
            .ok_or("head")?;
        let running_ci = CiRunRecord {
            id: "run-running".into(),
            sandbox_id: caller.sandbox.clone(),
            reference: CiRef::PullRequest(42),
            sha: initial_head.clone(),
            source: GateSource::ProjectConfig,
            state: RunState::Running,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(10),
            started_at: Some(Timestamp(11)),
            ended_at: None,
        };
        open_pull_at(&app, &github, &id, 42, &running_ci.sha).await?;
        github.set_checks(
            &running_ci.sha,
            yulius_app::ports::github::ChecksState::Pending,
        );
        let running_job = CiJobRecord {
            id: "job-running".into(),
            run_id: "run-running".into(),
            ordinal: 0,
            name: "checks".into(),
            key: "checks".into(),
            command: "scripts/dev ci".into(),
            matrix: None,
            state: RunState::Running,
            reason: None,
            exit_code: None,
            output: String::new(),
            failures: None,
            started_at: Some(Timestamp(11)),
            ended_at: None,
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        app.deps
            .store
            .insert_ci_run(&running_ci, &[running_job])
            .await?;

        // The CI run exists, but without a recorded PR the parked watcher cannot deliver it.
        let external = DeliveryRecord {
            stage: DeliveryStage::None,
            pr_number: None,
            ..open.clone()
        };
        assert!(
            app.deps
                .store
                .transition_delivery(&open.stage, &external)
                .await?
        );
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "external pending checks have no parked watcher"
        );
        assert!(
            app.deps
                .store
                .transition_delivery(&external.stage, &open)
                .await?
        );
        // A watch for an older head cannot take responsibility for the current checks.
        let newer = "4444444444444444444444444444444444444444";
        set_pull_head(&github, 42, newer)?;
        github.set_checks(newer, yulius_app::ports::github::ChecksState::Pending);
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "the active run is for an older PR head"
        );
        // Displaying an achieved goal or a human request does not hand its
        // unobserved current-head checks to the CI watcher.
        app.deps
            .store
            .transition_task(
                &id,
                Some(TaskStatus::Running),
                TaskStatus::Running,
                Some(Activity::Done),
                app.now(),
            )
            .await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "an achieved goal still owns the unwatched checks"
        );
        app.deps
            .store
            .transition_task(
                &id,
                Some(TaskStatus::Running),
                TaskStatus::Running,
                Some(Activity::Waiting),
                app.now(),
            )
            .await?;
        ask(&app, &id).await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "a pending human request does not settle unwatched checks"
        );
        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
            .await?;
        set_pull_head(&github, 42, &running_ci.sha)?;
        // The watcher switches to a different active head while the original head is being
        // prepared for parking. It can only deliver the result of the run it still follows.
        let competing = CiRunRecord {
            id: "newer-pr-head".into(),
            sha: "another-head".into(),
            queued_at: Timestamp(12),
            ..running_ci.clone()
        };
        let writing = Arc::clone(&app);
        let handle = tokio::runtime::Handle::current();
        yulius_app::race::on(Point::ParkPrepared, Path::new(id.as_str()), move || {
            assert!(
                tokio::task::block_in_place(|| {
                    handle.block_on(writing.deps.store.insert_ci_run(&competing, &[]))
                })
                .is_ok()
            );
        });
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "the watcher changed heads"
        );
        let restored = CiRunRecord {
            id: "current-pr-head".into(),
            queued_at: Timestamp(14),
            ..running_ci.clone()
        };
        app.deps.store.insert_ci_run(&restored, &[]).await?;
        parked(&app, &id).await?;
        let bounds = WatchBounds::default();
        // Still running: `watch_prs` must NOT wake the parked task.
        let woke = app.watch_prs(&bounds).await;
        assert!(
            woke.is_empty(),
            "active CI must not wake parked task: {woke:?}"
        );
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));

        // An unarmed green verdict remains visible on the PR. Only its person resumes this task.
        let passed_ci = CiRunRecord {
            id: "run-passed".into(),
            state: RunState::Passed,
            queued_at: Timestamp(20),
            started_at: Some(Timestamp(21)),
            ended_at: Some(Timestamp(22)),
            ..running_ci.clone()
        };
        github.set_checks(
            &passed_ci.sha,
            yulius_app::ports::github::ChecksState::Success,
        );
        let passed_job = CiJobRecord {
            id: "job-passed".into(),
            run_id: "run-passed".into(),
            ordinal: 0,
            name: "checks".into(),
            key: "checks".into(),
            command: "scripts/dev ci".into(),
            matrix: None,
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: String::new(),
            failures: None,
            started_at: Some(Timestamp(21)),
            ended_at: Some(Timestamp(22)),
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        app.deps
            .store
            .insert_ci_run(&passed_ci, &[passed_job])
            .await?;

        assert!(app.watch_prs(&bounds).await.is_empty());
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-553 PRK-103: an unpublished clean committed fast-forward CI repair above a failing open
/// PR head still needs its session when the task requests a person (SPEC-558 PKR-004).
#[test]
fn an_unpublished_ci_repair_keeps_its_session_even_with_a_human_request() -> TestResult {
    use std::process::Command;
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::delivery::DeliveryRecord;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::delivery::DeliveryStage;
    use yulius_domain::resources::Need;
    use yulius_domain::time::Timestamp;

    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        let wt = Path::new(task.worktree_path.as_deref().ok_or("worktree")?);
        let git = |args: &[&str]| -> Result<String, Error> {
            let out = Command::new("git")
                .args(args)
                .current_dir(wt)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        let failed_head = git(&["rev-parse", "HEAD"])?;
        std::fs::write(wt.join("repair.txt"), "clean fix\n")?;
        git(&["add", "repair.txt"])?;
        git(&["commit", "-m", "Clean CI repair"])?;
        let repair_head = git(&["rev-parse", "HEAD"])?;
        assert_ne!(repair_head, failed_head);

        open_pull_at(&app, &github, &id, 99, &failed_head).await?;
        github.set_checks(
            &failed_head,
            yulius_app::ports::github::ChecksState::Failure,
        );
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(99),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let failed_ci = CiRunRecord {
            id: "run-park-fail".into(),
            sandbox_id: caller.sandbox.clone(),
            reference: CiRef::PullRequest(99),
            sha: failed_head.clone(),
            source: GateSource::ProjectConfig,
            state: RunState::Failed,
            reason: None,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(10),
            started_at: Some(Timestamp(11)),
            ended_at: Some(Timestamp(12)),
        };
        let failed_job = CiJobRecord {
            id: "job-park-fail".into(),
            run_id: "run-park-fail".into(),
            ordinal: 0,
            name: "checks".into(),
            key: "checks".into(),
            command: "true".into(),
            matrix: None,
            state: RunState::Failed,
            reason: None,
            exit_code: Some(1),
            output: String::new(),
            failures: None,
            started_at: Some(Timestamp(11)),
            ended_at: Some(Timestamp(12)),
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        app.deps
            .store
            .insert_ci_run(&failed_ci, &[failed_job])
            .await?;
        // Pre-seed `pr-notices.json` at `PR_NOTICES_FILE` (`data/pr-notices.json`) so `pr_news_due`
        // is false while the upstream is red and the repair still needs to be pushed.
        let told_json = json!({
            id.as_str(): {
                "pr": 99,
                "head": failed_head,
                "keys": { "run": "run-park-fail" }
            }
        });
        std::fs::write(
            data.join("pr-notices.json"),
            serde_json::to_vec(&told_json)?,
        )?;

        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "unpublished clean fast-forward CI repair must defer parking"
        );
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Running));

        // Once the repair IS pushed to `origin/<branch>` (even with no new `CiRunRecord` yet),
        // safe parking is no longer deferred.
        let branch = task.branch.as_deref().ok_or("branch")?;
        git(&["push", "-q", "origin", branch])?;
        set_pull_head(&github, 99, &repair_head)?;
        github.set_checks(
            &repair_head,
            yulius_app::ports::github::ChecksState::Success,
        );
        parked(&app, &id).await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));

        // With green upstream checks the newer local commit alone keeps the session live,
        // including while a human request is pending.
        let owner_ctx = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.continue_task(
            &owner_ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask::default(),
        )
        .await?;
        wait_for(
            "resumed for second repair",
            Duration::from_secs(30),
            || async {
                status_of(&app, &id).await.0 == Some(TaskStatus::Running)
                    && status_of(&app, &id).await.1 == Some(Activity::Waiting)
            },
        )
        .await?;
        std::fs::write(wt.join("repair2.txt"), "second fix\n")?;
        git(&["add", "repair2.txt"])?;
        git(&["commit", "-m", "Second unpublished CI repair"])?;
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "the upstream is green but the newer repair is unpublished"
        );
        ask(&app, &id).await?;
        past_the_grace(&app, &id).await?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "a human request does not publish the outstanding repair"
        );
        // Re-running CI on the *published* head cannot hand the newer local commit to the watcher.
        for (offset, state) in [(1, RunState::Running), (2, RunState::Passed)] {
            let rerun = CiRunRecord {
                id: format!("repeat-{offset}"),
                sha: repair_head.clone(),
                state,
                queued_at: Timestamp(20 + offset),
                started_at: Some(Timestamp(20 + offset)),
                ended_at: (state == RunState::Passed).then_some(Timestamp(23)),
                ..failed_ci.clone()
            };
            app.deps.store.insert_ci_run(&rerun, &[]).await?;
            assert_eq!(
                app.park_idle_sessions_now().await,
                0,
                "a rerun on an older published commit cannot deliver the new one: {state:?}"
            );
        }
        let other = data.join("independent-clone");
        let other_path = other.to_str().ok_or("clone path")?;
        git(&[
            "clone",
            "-q",
            "-b",
            branch,
            &git(&["remote", "get-url", "origin"])?,
            other_path,
        ])?;
        git(&[
            "-C",
            other_path,
            "commit",
            "--allow-empty",
            "-m",
            "Parallel upstream update",
        ])?;
        git(&["-C", other_path, "push", "-q", "origin", branch])?;
        let parallel = git(&["-C", other_path, "rev-parse", "HEAD"])?;
        assert!(git(&["cat-file", "-e", &parallel]).is_err());
        set_pull_head(&github, 99, &parallel)?;
        github.set_checks(&parallel, yulius_app::ports::github::ChecksState::Success);
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "the external commit cannot prove publication"
        );
        git(&["fetch", "-q", "origin", branch])?;
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "the fetched head lacks the repair"
        );
        git(&["merge", "--no-edit", &parallel])?;
        git(&["push", "-q", "origin", branch])?;
        let last = git(&["rev-parse", "HEAD"])?;
        set_pull_head(&github, 99, &last)?;
        github.set_checks(&last, yulius_app::ports::github::ChecksState::Success);
        // The upstream may advance again. It now contains the repair, so the older checkout can park.
        let tree = git(&["rev-parse", "HEAD^{tree}"])?;
        let upstream = git(&[
            "commit-tree",
            &tree,
            "-p",
            &last,
            "-m",
            "Upstream after publication",
        ])?;
        git(&[
            "push",
            "-q",
            "origin",
            &format!("{upstream}:refs/heads/{branch}"),
        ])?;
        set_pull_head(&github, 99, &upstream)?;
        github.set_checks(&upstream, yulius_app::ports::github::ChecksState::Success);
        let path = wt.to_path_buf();
        yulius_app::race::on(Point::ParkPrepared, Path::new(id.as_str()), move || {
            let status = Command::new("git")
                .current_dir(path)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=t@example.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "Repair during park",
                ])
                .status();
            assert!(status.is_ok_and(|s| s.success()));
        });
        assert_eq!(
            app.park_idle_sessions_now().await,
            0,
            "commit after the PR lookup needs publication"
        );
        git(&["merge", "--no-edit", &upstream])?;
        git(&["push", "-q", "origin", branch])?;
        let latest = git(&["rev-parse", "HEAD"])?;
        set_pull_head(&github, 99, &latest)?;
        github.set_checks(&latest, yulius_app::ports::github::ChecksState::Success);
        parked(&app, &id).await?;
        assert_eq!(status_of(&app, &id).await.0, Some(TaskStatus::Parked));
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

/// SPEC-612: actual terminal recipients of a red integration branch, including reassignment.
#[test]
fn a_red_integration_branch_is_assigned_to_one_worker_and_announced_to_the_other() -> TestResult {
    use yulius_app::delivery::{
        DeliveryRecord,
        evidence::{Landing, LandingSource},
    };
    use yulius_domain::{ci::RunState, delivery::DeliveryStage};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = app(&data, 8, 0).await?;
        let (finished, caller) = answered(&app, &data).await?;
        let project = app
            .deps
            .store
            .sandbox(&caller.sandbox)
            .await?
            .ok_or("project")?;
        let mut ids = Vec::new();
        for prompt in ["Held worker", "Repair worker", "Other worker"] {
            let id = new_task(&app, &project.id, &caller.user, prompt, TaskMode::Prompt).await?;
            wait_for("worker ready", Duration::from_secs(30), || async {
                status_of(&app, &id).await.1 == Some(Activity::Waiting)
            })
            .await?;
            crate::pr_watch::leave_verification_step(&app, &id).await?;
            ids.push(id);
        }
        let repo = data
            .join("sandboxes")
            .join(project.id.as_str())
            .join("repo");
        let sha = crate::cd::git(&repo, &["rev-parse", "HEAD"])?;
        crate::cd::git(&repo, &["update-ref", "refs/remotes/origin/dev", &sha])?;
        let landing = Landing {
            branch: "dev".into(),
            head: sha.clone(),
            sha: sha.clone(),
            source: LandingSource::MergeCommand,
        };
        let merged = DeliveryRecord {
            task_id: finished.clone(),
            pr_number: Some(1),
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
        };
        assert!(
            app.deps
                .store
                .transition_with_landing(&DeliveryStage::None, &merged, None, Some(&landing))
                .await?
        );
        crate::staging::recorded_gate(
            &app,
            &project.id,
            &sha,
            RunState::Failed,
            Some("---- delivery::gate stdout ----\nassertion failed\n"),
        )
        .await?;
        let (owner, terminal) = crate::task_message::terminal(&app, &caller, &ids[0]).await?;
        app.terminal_input(&owner, &terminal, b"keep my draft", false)
            .await?;
        let messages = || async { app.tell_gate_repairs(&project).await };
        assert_eq!(messages().await?, 2);
        wait_for(
            "workers saw their assignments",
            Duration::from_secs(30),
            || async {
                screen(&app, &ids[1]).contains("You own this repair")
                    && screen(&app, &ids[2]).contains("Task #3 owns this repair")
            },
        )
        .await?;
        assert!(!screen(&app, &ids[0]).contains("You own this repair"));
        for worker in &ids[1..] {
            let shown = screen(&app, worker);
            assert!(
                shown.contains("delivery::gate")
                    && shown.contains(&sha[..12])
                    && shown.contains("task #1"),
                "{shown}"
            );
        }
        assert_eq!(
            messages().await?,
            0,
            "a repeated tick cannot duplicate the repair"
        );
        assert!(!screen(&app, &finished).contains("integration gate failed"));
        assert_eq!(app.deps.store.attempts(&finished).await?.len(), 1);
        app.deps
            .store
            .replace_steps(&ids[1], &[], "person", app.now())
            .await?;
        app.deps
            .store
            .set_activity(&ids[1], Some(Activity::Done), app.now())
            .await?;
        assert_eq!(messages().await?, 1);
        wait_for(
            "remaining worker took over",
            Duration::from_secs(30),
            || async { screen(&app, &ids[2]).contains("You own this repair") },
        )
        .await?;
        let (_, last_terminal) = crate::task_message::terminal(&app, &caller, &ids[2]).await?;
        app.terminal_input(&owner, &last_terminal, b"another draft", false)
            .await?;
        assert_eq!(messages().await?, 0);
        assert_eq!(
            app.status(&owner).await?.sandboxes[0].gate_notices[0].test,
            "delivery::gate"
        );
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-575 MFS-007, with real processes: a task's staging outcome (here its candidate's failed
/// gate) is typed into its live session once, with the run to read and the next step, and not
/// while its person must answer a prompt there first.
#[test]
fn a_staging_outcome_reaches_its_task_once() -> TestResult {
    use yulius_app::ports::store_ext::StagingRequestRecord;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let browser = Arc::new(crate::delivery::HeldBrowserLookup::for_publication());
        let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
        let (app, _, _) = crate::first_message::start_hosted_with_github(
            &data, &fake, FakeAgentOptions { answers_first_message: true, ..hooked() },
            (0, Arc::new(ScriptedUsage::default()), Arc::new(yulius::wiring::clock::SystemClock)),
            None, Some(browser.clone()), |config| { config.max_live_sessions = 8; },
        ).await?;
        let (id, caller) = answered(&app, &data).await?;
        crate::pr_watch::leave_verification_step(&app, &id).await?;
        let task = TaskStore::task(app.deps.store.as_ref(), &id)
            .await?
            .ok_or("task")?;
        let revision = "0123456789abcdef0123456789abcdef01234567";
        assert!(
            app.deps
                .store
                .insert_staging_request(&StagingRequestRecord {
                    id: "sr_1".into(),
                    sandbox_id: task.sandbox_id.clone(),
                    environment: "staging".into(),
                    task_id: Some(task.id.clone()),
                    revision: revision.into(),
                    state: yulius_domain::staging::StagingState::GateFailed,
                    ci_run_id: Some("ci-run-9".into()),
                    cd_run_id: None,
                    attempts: 0,
                    note: Some("the full gate failed on revision 0123456789ab: run ci-run-9 (full); staging did not change".into()),
                    requested_by: "task:1".into(),
                    told: false,
                    created_at: app.now(),
                    updated_at: app.now(),
                })
                .await?
        );
        app.deps.store.set_activity(&id, Some(Activity::Done), app.now()).await?;
        let owner = AuthContext {
            user: app.deps.store.user(&caller.user).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        assert_eq!(app.capacity_view(&owner).await?.accounts[0].in_use_by, Some(1),
            "the session that will receive the staging outcome still uses its subscription");
        TaskStore::transition_task(
            app.deps.store.as_ref(),
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::NeedsAttention),
            app.now(),
        )
        .await?;
        assert_eq!(app.tell_staging_outcomes().await, 0);
        assert!(!screen(&app, &id).contains("staging request sr_1"));
        TaskStore::transition_task(
            app.deps.store.as_ref(),
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
        // A human decision can arrive while Chromium is looking for requests in its tabs.
        let check_with_busy_browser = || {
            let start = Arc::new(tokio::sync::Notify::new());
            let check = tokio::spawn({
                let app = app.clone();
                let start = start.clone();
                async move {
                    start.notified().await;
                    app.tell_staging_outcomes().await
                }
            });
            let (entered, release) = browser.hold_lookup_for(check.id());
            start.notify_one();
            (check, entered, release)
        };
        let (check, entered, release) = check_with_busy_browser();
        tokio::time::timeout(Duration::from_secs(30), entered.notified()).await?;
        ask(&app, &id).await?;
        release.notify_one();
        assert_eq!(check.await?, 0, "the decision arrived during the browser read");
        assert_eq!(app.deps.store.untold_staging_requests().await?.len(), 1);
        assert_eq!(app.tell_staging_outcomes().await, 0, "an explicit decision is outstanding");
        app.deps.store.set_task_needs_you(&id, None, app.now()).await?;
        let (check, entered, _release) = check_with_busy_browser();
        tokio::time::timeout(Duration::from_secs(30), entered.notified()).await?;
        assert_eq!(tokio::time::timeout(Duration::from_secs(10), check).await??, 0,
            "a busy browser defers the outcome to another tick");
        assert_eq!(app.deps.store.untold_staging_requests().await?.len(), 1);
        wait_for("the outcome told", Duration::from_secs(30), || async {
            app.tell_staging_outcomes().await;
            let shown = screen(&app, &id);
            shown.contains("staging request sr_1 for revision 0123456789ab")
                && shown.contains("gate_failed")
                && shown.contains("ci-run-9")
        })
        .await?;
        assert!(
            app.deps.store.untold_staging_requests().await?.is_empty(),
            "told once"
        );
        app.deps.store.set_activity(&id, Some(Activity::Done), app.now()).await?;
        assert_eq!(app.capacity_view(&owner).await?.accounts[0].in_use_by, Some(0),
            "a finished task releases its subscription once its outcome has been delivered");
        // A deploy retry can finish while the old failure notification checks the browser.
        // The old notification must leave this request's new verdict for the next tick.
        use yulius_domain::staging::StagingState::{DeployFailed, Deploying, Ready, Verified};
        use yulius_app::ports::store_ext::StagingChange;
        let store = &app.deps.store;
        let retry = StagingRequestRecord {
            id: "sr_retry".into(),
            revision: "abcdef0123456789abcdef0123456789abcdef01".into(),
            state: DeployFailed,
            cd_run_id: Some("first-deploy".into()),
            note: Some("the first deploy failed".into()),
            told: false,
            ..store.staging_request("sr_1").await?.ok_or("staging request")?
        };
        assert!(store.insert_staging_request(&retry).await?);
        // A resize holds the real terminal writer. Check whether this pass defers before
        // a decision or retry can overtake the paste.
        let (entered_writer, free_writer) = pause_watch_or_queue(Point::TerminalResizeOwnsWriter, &id);
        let resize = {
            let app = app.clone();
            let id = id.clone();
            tokio::spawn(async move { app.resize_run(&id, 100, 40).await })
        };
        tokio::time::timeout(Duration::from_secs(30), entered_writer.notified()).await?;
        let mut writer_pass = {
            let app = app.clone();
            tokio::spawn(async move { app.tell_staging_outcomes().await })
        };
        let while_held = tokio::time::timeout(Duration::from_secs(5), &mut writer_pass).await;
        ask(&app, &id).await?;
        free_writer.notify_one();
        resize.await??;
        if while_held.is_err() {
            let sent = writer_pass.await?;
            return Err(format!("the pass waited for the writer and sent {sent} obsolete notifications").into());
        }
        assert_eq!(while_held??, 0);
        assert!(!screen(&app, &id).contains("first-deploy"));
        assert_eq!(app.tell_staging_outcomes().await, 0, "the late human decision still holds the failure");
        store.set_task_needs_you(&id, None, app.now()).await?;
        let (check, entered, release) = check_with_busy_browser();
        tokio::time::timeout(Duration::from_secs(30), entered.notified()).await?;
        let change = StagingChange::default();
        assert!(store.move_staging_request(&retry.id, DeployFailed, Ready, &change, app.now()).await?);
        assert!(store.move_staging_request(&retry.id, Ready, Deploying, &change, app.now()).await?);
        assert!(store.move_staging_request(&retry.id, Deploying, Verified, &StagingChange {
            cd_run_id: Some("retry-run".into()),
            note: Some("the retry verified".into()),
            ..StagingChange::default()
        }, app.now()).await?);
        release.notify_one();
        assert_eq!(check.await?, 0, "the previous failure cannot consume the retry's wake");
        assert!(!screen(&app, &id).contains("first-deploy"));
        assert_eq!(store.task_staging_wait(&id).await?, Some(Verified));
        wait_for("the verified retry told", Duration::from_secs(30), || async {
            app.tell_staging_outcomes().await;
            screen(&app, &id).contains("retry-run")
        }).await?;
        assert_eq!(store.task_staging_wait(&id).await?, None);
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
        crate::pr_watch::leave_verification_step(&app, &id).await?;
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
        app.deps
            .store
            .replace_steps(&id, &[], "person", app.now())
            .await?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Waiting), app.now())
            .await?;
        let mut late = app.deps.store.handoff("ho_1").await?.ok_or("handoff")?;
        let mut late_target = target.clone();
        late_target.id = TaskId::parse("t-api-late")?;
        late_target.number = 2;
        late_target.title = "Late finished handoff".into();
        late_target.status = TaskStatus::Closed;
        app.deps.store.insert_task(&late_target).await?;
        late.target_task = Some(late_target.id);
        late.id = "ho_finished".to_owned();
        late.args = r#"{"sandbox":"api","title":"Late finished handoff","prompt":"P"}"#.into();
        app.deps.store.insert_handoff(&late).await?;
        assert_eq!(app.tell_handoff_outcomes().await, 0);
        assert!(!screen(&app, &id).contains("/api #2"));
        assert!(app.deps.store.untold_handoffs().await?.is_empty());
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

#[test]
fn a_cold_parked_summary_reads_historical_spec_and_goal_evidence() -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let live = app(&data, 8, 1).await?;
        let (id, _) = answered(&live, &data).await?;
        parked(&live, &id).await?;
        live.deps
            .store
            .set_spec_status(&id, Some("in-progress"))
            .await?;
        let run = live
            .deps
            .store
            .attempts(&id)
            .await?
            .pop()
            .ok_or("parked run")?;
        std::fs::write(Path::new(&run.run_dir).join("parked-goal-unfinished"), [])?;
        let cold = app(&data, 8, 1).await?;
        let task = cold.deps.store.task(&id).await?.ok_or("parked task")?;
        assert_eq!(task.status, TaskStatus::Parked);
        let summary = cold.task_summary(&task).await?;
        assert!(summary.spec_unfinished);
        assert!(summary.goal_unfinished);
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_first_pr_watch_during_an_active_queue_decision_remembers_the_merge() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::Taken, None)
}

#[test]
fn a_queue_retry_keeps_the_already_observed_prs_stall_deadline() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::Taken, Some(1481))
}

#[test]
fn a_first_deferred_watch_of_a_replacement_pr_clears_the_previous_stall() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::Taken, Some(1480))
}

#[test]
fn a_free_pr_watch_overtaken_by_a_queue_decision_remembers_the_merge() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::TakenDuringRead, None)
}

#[test]
fn a_new_arm_that_merges_before_the_free_watch_returns_is_observed_once() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::MergedDuringRead, None)
}

#[test]
fn a_completed_queue_retry_preserves_its_new_reason_for_the_live_owner() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::KeptDuringRead, None)
}

#[test]
fn a_first_pr_watch_during_arm_restoration_remembers_the_merge() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::Restoring, None)
}

#[test]
fn a_saved_owner_defers_check_ins_before_its_arm_is_restored() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::SavedOwner, None)
}

#[test]
fn another_owners_saved_arm_allows_this_prs_check_in() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::SavedPeer, None)
}

#[test]
fn a_saved_arm_arriving_during_a_free_watch_defers_the_check_in() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::SavedDuringRead, None)
}

#[test]
fn an_armed_watch_rechecks_a_merge_recorded_before_dispatch() -> TestResult {
    first_watch_during_queue_decision(QueueWatch::RecordedMergeDuringRead, None)
}

enum QueueWatch {
    RecordedMergeDuringRead,
    Restoring,
    SavedOwner,
    SavedPeer,
    SavedDuringRead,
    KeptDuringRead,
    MergedDuringRead,
    Taken,
    TakenDuringRead,
}

fn first_watch_during_queue_decision(case: QueueWatch, previously_seen: Option<u64>) -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::delivery::DeliveryStage;

    let (_dir, data) = data_dir()?;
    runtime()?.block_on(async {
        const PRIOR_OBSERVATION: i64 = 1_800_000_000_000;
        const NEXT_OBSERVATION: i64 = PRIOR_OBSERVATION + 17_000;
        let clock = Arc::new(crate::support::TestClock(PRIOR_OBSERVATION.into()));
        let (app, github) = app_with_pulls_and_clock(&data, 8, 1, clock.clone(), |_| {}).await?;
        let (id, caller) = answered(&app, &data).await?;
        crate::pr_watch::leave_verification_step(&app, &id).await?;
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let worktree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        let head = crate::pr_watch::git(worktree, &["rev-parse", "HEAD"])?;
        open_pull_at(&app, &github, &id, previously_seen.unwrap_or(1481), &head).await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(1481),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let due = WatchBounds {
            check_in_after_ms: 0,
            stall_quiet_ms: 0,
            stall_after_ms: 0,
            ..WatchBounds::default()
        };
        let arms = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let notices = data.join(yulius_app::pr_watch::PR_NOTICES_FILE);
        let saved_arm = serde_json::to_vec(
            &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
        )?;
        if matches!(case, QueueWatch::RecordedMergeDuringRead) {
            std::fs::write(&arms, &saved_arm)?;
            app.restore_armed_merges().await;
            let (read, release) = pause_watch_or_queue(Point::LivePrFactsRead, &id);
            let watcher = Arc::clone(&app);
            let pass = tokio::spawn(async move { watcher.watch_prs(&due).await });
            tokio::time::timeout(Duration::from_secs(10), read.notified()).await?;
            let merged = DeliveryRecord {
                stage: DeliveryStage::MergedIntegration,
                ..open.clone()
            };
            assert!(
                app.deps
                    .store
                    .transition_delivery(&DeliveryStage::PrOpen, &merged)
                    .await?
            );
            release.notify_one();
            let news = pass.await?;
            assert!(
                news.is_empty(),
                "advice overtaken by recorded merge: {news:?}"
            );
        } else if matches!(case, QueueWatch::Restoring | QueueWatch::SavedOwner | QueueWatch::SavedPeer | QueueWatch::SavedDuringRead) {
            std::fs::write(&arms, b"[")?;
            app.restore_armed_merges().await;
            let first = match case {
                QueueWatch::SavedOwner => {
                    std::fs::write(&arms, &saved_arm)?;
                    app.watch_prs(&due).await
                }
                QueueWatch::SavedPeer => {
                    std::fs::write(&arms, serde_json::to_vec(&json!([{"task":"peer","head":head,"summary":"reviewed","armed_at":30}]))?)?;
                    app.watch_prs(&due).await
                }
                QueueWatch::SavedDuringRead => {
                    std::fs::write(&arms, b"[]")?;
                    let (read, release) = pause_watch_or_queue(Point::LivePrFactsRead, &id);
                    let watcher = Arc::clone(&app);
                    let pass = tokio::spawn(async move { watcher.watch_prs(&due).await });
                    tokio::time::timeout(Duration::from_secs(10), read.notified()).await?;
                    std::fs::write(&arms, &saved_arm)?;
                    release.notify_one();
                    pass.await?
                }
                _ => app.watch_prs(&due).await,
            };
            if matches!(case, QueueWatch::SavedPeer) {
                assert_eq!(first.len(), 1, "another task's saved arm held this owner: {first:?}");
                assert_eq!(first[0].0, id);
            } else {
                assert!(first.is_empty(), "check-in before arm restoration: {first:?}");
            }
        } else if matches!(case, QueueWatch::KeptDuringRead) {
            std::fs::write(&arms, &saved_arm)?;
            app.restore_armed_merges().await;
            let (mut green, job) = crate::pr_watch::run(
                &caller.sandbox,
                "green",
                &head,
                yulius_domain::ci::RunState::Passed,
                None,
                10,
            );
            green.reference = yulius_domain::ci::CiRef::PullRequest(1481);
            app.deps.store.insert_ci_run(&green, &[job]).await?;
            let (read, release) = pause_watch_or_queue(Point::LivePrFactsRead, &id);
            let watcher = Arc::clone(&app);
            let pass = tokio::spawn(async move { watcher.watch_prs(&due).await });
            tokio::time::timeout(Duration::from_secs(10), read.notified()).await?;
            let pulls = std::mem::take(&mut *github.pulls.lock().map_err(|_| "pull registry")?);
            app.merge_armed(&caller.sandbox).await;
            *github.pulls.lock().map_err(|_| "pull registry")? = pulls;
            release.notify_one();
            let outdated = pass.await?;
            assert!(
                outdated.is_empty(),
                "advice prepared before the retry: {outdated:?}"
            );
            let deferred: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
            assert_eq!(deferred[id.as_str()]["pr"], 1481);
            assert_eq!(deferred[id.as_str()]["progress"], "", "overtaken read recorded progress");
            let fresh = app.watch_prs(&WatchBounds::default()).await;
            assert_eq!(fresh.len(), 1, "{fresh:?}");
            assert!(
                fresh[0].1.contains("cannot read this task's pull request"),
                "{fresh:?}"
            );
            assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        } else if matches!(
            case,
            QueueWatch::TakenDuringRead | QueueWatch::MergedDuringRead
        ) {
            let (read, release_read) = pause_watch_or_queue(Point::LivePrFactsRead, &id);
            let (taken, release_queue) = pause_watch_or_queue(Point::FreeArmTaken, &id);
            let watcher = Arc::clone(&app);
            let pass = tokio::spawn(async move { watcher.watch_prs(&due).await });
            tokio::time::timeout(Duration::from_secs(10), read.notified()).await?;
            std::fs::write(&arms, &saved_arm)?;
            app.restore_armed_merges().await;
            let queue = Arc::clone(&app);
            let decision = tokio::spawn(async move { queue.merge_armed(&caller.sandbox).await });
            tokio::time::timeout(Duration::from_secs(10), taken.notified()).await?;
            let news = if matches!(case, QueueWatch::MergedDuringRead) {
                {
                    let mut pulls = github.pulls.lock().map_err(|_| "pull registry")?;
                    let pull = pulls
                        .iter_mut()
                        .find(|pr| pr.number == 1481)
                        .ok_or("hosted PR")?;
                    pull.merged = true;
                    pull.state = "closed".into();
                    pull.merge_commit_sha = Some(head.clone());
                }
                release_queue.notify_one();
                decision.await?;
                let recorded = app
                    .deps
                    .store
                    .delivery(&id)
                    .await?
                    .ok_or("queue merge outcome")?;
                assert_eq!(recorded.stage, DeliveryStage::MergedIntegration);
                release_read.notify_one();
                pass.await?
            } else {
                release_read.notify_one();
                let news = pass.await?;
                release_queue.notify_one();
                decision.await?;
                news
            };
            assert!(
                news.is_empty(),
                "stale free-arm news during the decision: {news:?}"
            );
        } else {
            std::fs::write(&arms, &saved_arm)?;
            app.restore_armed_merges().await;
            let prior = if let Some(number) = previously_seen {
                let old = DeliveryRecord {
                    pr_number: Some(number),
                    ..open.clone()
                };
                assert!(
                    app.deps
                        .store
                        .transition_delivery(&DeliveryStage::PrOpen, &old)
                        .await?
                );
                let (mut run, job) = crate::pr_watch::run(
                    &caller.sandbox,
                    "old",
                    &head,
                    yulius_domain::ci::RunState::Cancelled,
                    None,
                    1,
                );
                run.reference = yulius_domain::ci::CiRef::PullRequest(number);
                app.deps.store.insert_ci_run(&run, &[job]).await?;
                let news = app.watch_prs(&due).await;
                assert!(
                    app.pr_stalled(&id).is_some(),
                    "expected an existing stall: {news:?}"
                );
                let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
                assert!(
                    app.deps
                        .store
                        .transition_delivery(&DeliveryStage::PrOpen, &open)
                        .await?
                );
                if number != 1481 {
                    github.pulls.lock().map_err(|_| "fixture pull")?.clear();
                    open_pull_at(&app, &github, &id, 1481, &head).await?;
                }
                Some(saved[id.as_str()].clone())
            } else {
                None
            };
            clock
                .0
                .store(NEXT_OBSERVATION, std::sync::atomic::Ordering::SeqCst);
            let watch = Arc::clone(&app);
            let handle = tokio::runtime::Handle::current();
            let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let seen_in_hook = Arc::clone(&seen);
            yulius_app::race::on(Point::FreeArmTaken, Path::new(id.as_str()), move || {
                let news = tokio::task::block_in_place(|| handle.block_on(watch.watch_prs(&due)));
                assert!(
                    news.is_empty(),
                    "first watch interrupted the decision: {news:?}"
                );
                seen_in_hook.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            app.merge_armed(&caller.sandbox).await;
            if let Some(prior) = prior {
                let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
                let current = &saved[id.as_str()];
                if previously_seen == Some(1481) {
                    assert_eq!(current, &prior, "queue retry changed progress or the stall");
                    assert!(app.pr_stalled(&id).is_some());
                } else {
                    assert_eq!(current["pr"], 1481);
                    assert_eq!(current["progress"], "");
                    assert_eq!(prior["progress_ms"], PRIOR_OBSERVATION);
                    assert_eq!(current["progress_ms"], NEXT_OBSERVATION);
                    assert!(app.pr_stalled(&id).is_none());
                }
            }
            assert!(
                seen.load(std::sync::atomic::Ordering::SeqCst),
                "queue took the arm"
            );
        }
        if previously_seen.is_none() {
            let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
            let first_seen = saved[id.as_str()]["progress_ms"]
                .as_i64()
                .ok_or("first watch clock")?;
            assert!(
                (open.updated_at.0..=app.now().0).contains(&first_seen),
                "{first_seen}"
            );
        }
        // The integration merge is recorded between watch passes, before GitHub's PR read catches up.
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            ..open
        };
        if !matches!(case, QueueWatch::MergedDuringRead | QueueWatch::RecordedMergeDuringRead) {
            assert!(
                app.deps
                    .store
                    .transition_delivery(&DeliveryStage::PrOpen, &merged)
                    .await?
            );
        }
        let news = app.watch_prs(&WatchBounds::default()).await;
        let repeat = app.watch_prs(&WatchBounds::default()).await;
        app.stop_task_now(&id).await?;
        assert_eq!(
            news.len(),
            1,
            "merge after the first deferred watch: {news:?}"
        );
        assert_eq!(news[0].0, id);
        assert!(
            news[0].1.contains("Pull request #1481 is merged"),
            "{news:?}"
        );
        assert!(repeat.is_empty(), "{repeat:?}");
        Ok(())
    })
}

fn pause_watch_or_queue(
    point: Point,
    id: &TaskId,
) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    pause_at_key(point, id.as_str())
}

fn pause_at_key(point: Point, key: &str) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&entered);
    let gate = Arc::clone(&release);
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(point, Path::new(key), move || {
        signal.notify_one();
        let released = tokio::task::block_in_place(|| {
            handle.block_on(tokio::time::timeout(
                Duration::from_secs(15),
                gate.notified(),
            ))
        });
        assert!(
            released.is_ok(),
            "test did not release the watcher or queue"
        );
    });
    (entered, release)
}

#[test]
fn a_busy_terminal_writer_leaves_the_armed_notice_unheard() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::{CiRef, RunState};
    use yulius_domain::delivery::DeliveryStage;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        let head = crate::pr_watch::git(tree, &["rev-parse", "HEAD"])?;
        open_pull_at(&app, &github, &id, 1481, &head).await?;
        app.deps
            .store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: id.clone(),
                    stage: DeliveryStage::PrOpen,
                    pr_number: Some(1481),
                    updated_at: app.now(),
                },
            )
            .await?;
        let (mut run, job) = crate::pr_watch::run(
            &caller.sandbox,
            "held-writer-ci",
            &head,
            RunState::Failed,
            None,
            31,
        );
        run.reference = CiRef::PullRequest(1481);
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        app.restore_armed_merges().await;
        let session = app.runtime(&id);
        let last_input = session
            .last_input_ms
            .load(std::sync::atomic::Ordering::SeqCst);
        let (entered, release) = pause_watch_or_queue(Point::TerminalResizeOwnsWriter, &id);
        let resize = {
            let app = app.clone();
            let id = id.clone();
            tokio::spawn(async move { app.resize_run(&id, 100, 40).await })
        };
        tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
        let watcher = app.clone();
        let mut pass =
            tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        let while_busy = tokio::time::timeout(Duration::from_secs(5), &mut pass).await;
        ask(&app, &id).await?;
        release.notify_one();
        resize.await??;
        let news = match while_busy {
            Ok(news) => news?,
            Err(_) => {
                let stale = pass.await?;
                return Err(
                    format!("notice waited behind the resize and then typed: {stale:?}").into(),
                );
            }
        };
        assert_eq!(
            session
                .last_input_ms
                .load(std::sync::atomic::Ordering::SeqCst),
            last_input
        );
        assert!(!screen(&app, &id).contains("held-writer-ci"));
        assert!(news.is_empty(), "notice while writer occupied: {news:?}");
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
            .await?;
        let current = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(current.len(), 1, "{current:?}");
        assert!(current[0].1.contains("held-writer-ci"));
        wait_for(
            "the agent received the CI notice",
            Duration::from_secs(10),
            || async { screen(&app, &id).contains("held-writer-ci") },
        )
        .await?;
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
