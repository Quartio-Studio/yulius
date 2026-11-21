//! Codex as a second harness with real processes (SPEC-503): supervisor → detached
//! `yulius ptyhost` → fake agent writing a Codex CLI 0.169 shaped rollout. A Codex task runs on a
//! Codex account in its own Codex home, its rollout is found and watched, its usage reaches its
//! account, it moves to another Codex account at a usage limit by a native resume, a restart
//! reattaches it, and the credentials Codex refreshed are kept. No test calls OpenAI.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::supervision::{
    build_full, runtime, screen_contains, seed_records, seed_repo, task, wait_for,
};
use crate::support;
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, LaunchKind, TaskMode, TaskOrigin};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

/// A synthetic Codex `auth.json` refreshed at `last_refresh`.
fn codex_auth(last_refresh: &str) -> String {
    format!(
        r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"h.e30.s","access_token":"SYNTHETIC-access-{last_refresh}","refresh_token":"SYNTHETIC-refresh","account_id":"acc-synthetic"}},"last_refresh":"{last_refresh}"}}"#
    )
}

/// Connects a synthetic Codex sign-in for tests that create Codex tasks through other interfaces.
pub(super) async fn connect_codex(app: &App) -> TestResult {
    add_codex_account(app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
    Ok(())
}

pub(super) async fn add_codex_account(
    app: &App,
    id: &str,
    auth: &str,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::OpenAi,
            label: format!("Codex {id}"),
            credential: Some(app.deps.crypto.seal(auth.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(account)
}

pub(super) async fn new_codex_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    new_codex_task_in(
        app,
        sandbox,
        user,
        title,
        yulius_app::model::TaskMode::Prompt,
    )
    .await
}

async fn new_codex_task_in(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    mode: yulius_app::model::TaskMode,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::Codex,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode,
            supertask: false,
        })
        .await?;
    Ok(record.id)
}

async fn stored_credential(
    app: &App,
    account: &AccountId,
) -> Result<String, Box<dyn std::error::Error>> {
    let sealed = app
        .deps
        .store
        .account(account)
        .await?
        .ok_or("account")?
        .credential
        .ok_or("credential")?;
    Ok(String::from_utf8(app.deps.crypto.open(&sealed)?)?)
}

/// An RFC 3339 time `ms_ago` milliseconds before now (a refresh Codex just made).
fn ago(ms_ago: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    yulius_app::auth::rfc3339(Timestamp(now - ms_ago))
}

/// The output tokens stored for `attempt`.
fn saved_output(data: &Path, attempt: &str) -> Option<i64> {
    let db = rusqlite::Connection::open(data.join("yulius.db")).ok()?;
    db.query_row(
        "SELECT output_tokens FROM metrics WHERE attempt_id = ?1",
        [attempt],
        |row| row.get(0),
    )
    .ok()
    .flatten()
}

fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

async fn restart_fixture(
    data: &Path,
) -> Result<(Arc<App>, SandboxId, UserId, TaskId, String), Box<dyn std::error::Error>> {
    restart_fixture_in(data, TaskMode::Prompt).await
}

async fn restart_fixture_in(
    data: &Path,
    mode: TaskMode,
) -> Result<(Arc<App>, SandboxId, UserId, TaskId, String), Box<dyn std::error::Error>> {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let agent = FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        resume_prompts: true,
        interrupts: true,
        ..Default::default()
    };
    let app = build_full(data, Some(&fake), agent, 1)?;
    let (sandbox, user) = seed_records(&app).await?;
    seed_repo(data, &sandbox)?;
    connect_codex(&app).await?;
    let id = new_codex_task_in(&app, &sandbox, &user, "Original request", mode).await?;
    wait_for(
        "the original Codex conversation",
        Duration::from_secs(30),
        || async {
            app.deps
                .store
                .provider_sessions(&id)
                .await
                .is_ok_and(|rows| !rows.is_empty())
                && screen_contains(&app, &id, "ECHO: Original request").await
        },
    )
    .await?;
    let session = app
        .deps
        .store
        .provider_sessions(&id)
        .await?
        .pop()
        .ok_or("session")?
        .session_id;
    Ok((app, sandbox, user, id, session))
}

#[test]
fn a_codex_restart_checks_the_harness_and_history_of_legacy_records() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let legacy = new_codex_task(&app, &sandbox, &user, "Legacy conversation").await?;
        assert_eq!(task(&app, &legacy).await?.status, TaskStatus::Queued);
        support::stop_tasks(&app, &[&legacy, &id]).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let refused = async |task_id: &TaskId| {
            matches!(
                app.continue_task(&ctx, task_id.as_str(), &Default::default()).await,
                Err(yulius_app::error::AppError::Conflict(_))
            )
        };
        db.execute("UPDATE attempts SET harness = 'claude' WHERE task_id = ?1", [id.as_str()])?;
        assert!(refused(&id).await);
        db.execute("UPDATE attempts SET harness = 'codex', provider_session_id = NULL WHERE task_id = ?1", [id.as_str()])?;
        db.execute("UPDATE provider_sessions SET harness = 'claude', last_seen = (SELECT started_at FROM attempts WHERE task_id = ?1) WHERE task_id = ?1", [id.as_str()])?;
        assert!(refused(&id).await);

        // Imported history can have a session entry and no local attempt.
        db.execute("UPDATE provider_sessions SET task_id = ?1 WHERE task_id = ?2", [legacy.as_str(), id.as_str()])?;
        assert!(refused(&legacy).await);
        db.execute("UPDATE provider_sessions SET harness = 'codex' WHERE task_id = ?1", [legacy.as_str()])?;
        let stopped = task(&app, &legacy).await?.status;
        let store = &app.deps.store;
        assert!(store.transition_task(&legacy, Some(stopped), TaskStatus::Parked, None, app.now()).await?);
        assert!(refused(&legacy).await, "a parked session needs its attempt");
        assert!(store.transition_task(&legacy, Some(TaskStatus::Parked), stopped, None, app.now()).await?);
        let resumed = app.continue_task(&ctx, legacy.as_str(), &Default::default()).await?;
        assert_eq!(resumed.activity.as_deref(), Some("working"));
        let run = store.live_attempt(&legacy).await?.ok_or("legacy restart")?;
        assert_eq!(run.launch_kind, LaunchKind::Resume);
        support::stop_tasks(&app, &[&legacy]).await
    })
}

#[test]
fn moving_a_finished_codex_goal_task_preserves_its_idle_activity() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture_in(&data, TaskMode::Goal).await?;
        let ctx = admin_ctx(&app, &user).await?;
        app.close_task_and_wait(&id).await?;
        let opened = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(opened.activity.as_deref(), Some("waiting"));
        let original = AccountId::parse("a-codex")?;
        let alternate =
            add_codex_account(&app, "alternate", &codex_auth("2026-11-01T10:00:00Z")).await?;
        // The same idle conversation is available for a follow-up, or marked finished.
        let mut moved_activities = Vec::new();
        for (account, idle) in [(&alternate, Activity::Waiting), (&original, Activity::Done)] {
            if idle == Activity::Done {
                app.deps
                    .store
                    .set_activity(&id, Some(idle), app.now())
                    .await?;
            }
            app.switch_task_for_person(&ctx, id.as_str(), Some(account.as_str()))
                .await?;
            let run = app.deps.store.live_attempt(&id).await?.ok_or("moved run")?;
            assert_eq!(run.account_id.as_ref(), Some(account));
            assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
            let launch = std::fs::read_to_string(Path::new(&run.run_dir).join("fake-launch.txt"))?;
            assert!(launch.contains("prompt: None"), "{launch}");
            let listed = app
                .list_tasks(&ctx, sandbox.as_str(), false)
                .await?
                .into_iter()
                .find(|task| task.id == id.as_str())
                .ok_or("moved task in the list")?;
            assert_eq!(listed.status, "running");
            moved_activities.push(listed.activity);
            assert_eq!(listed.working_since, None);
        }
        app.close_task_for_person(&ctx, id.as_str()).await?;
        wait_for(
            "the moved task to close",
            Duration::from_secs(30),
            || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|task| task.status == TaskStatus::Closed)
            },
        )
        .await?;
        assert_eq!(
            moved_activities,
            [Some("waiting".into()), Some("done".into())]
        );
        Ok(())
    })
}

#[test]
fn moving_an_idle_codex_session_with_a_pending_reviewer_starts_a_turn() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        app.close_task_and_wait(&id).await?;
        let opened = app.continue_task(&ctx, id.as_str(), &Default::default()).await?;
        assert_eq!(opened.activity.as_deref(), Some("waiting"));
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute(
            "INSERT INTO review_rounds (id, task_id, round, agent, model, reason, subagent_id, started_at, cut_off)
             VALUES ('open-round', ?1, 1, 'reviewer', 'fable', 'round', 'fixture-reviewer', 0, 0)",
            [id.as_str()],
        )?;
        app.cut_off_review_for_test(&id, "fixture-reviewer", None).await;
        assert_eq!(app.review_recovery_for_test(&id).await.0.as_deref(), Some("fixture-reviewer"));
        let other = add_codex_account(&app, "alternate", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let moved = app.switch_task(&ctx, id.as_str(), &yulius_contracts::api::tasks::ContinueTask {
            account_id: Some(other.to_string()), ..Default::default()
        }).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("moved reviewer")?;
        let mode = std::fs::read_to_string(Path::new(&run.run_dir).join("fake-launch.txt"))?;
        assert!(mode.contains("Resume that reviewer now"), "{mode}");
        assert_eq!(moved.activity.as_deref(), Some("working"));
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn continuing_a_stopped_codex_task_submits_one_turn_when_its_slot_opens() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        app.send_input(&id, b"/work 60000 Unfinished turn\r")
            .await?;
        wait_for("the turn being stopped", Duration::from_secs(30), || {
            screen_contains(&app, &id, "WORKING")
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        // SessionStart recorded the attempt ID before an outage cut off the lineage write.
        rusqlite::Connection::open(data.join("yulius.db"))?.execute(
            "DELETE FROM provider_sessions WHERE task_id = ?1",
            [id.as_str()],
        )?;
        let blocker = new_codex_task(&app, &sandbox, &user, "Occupy the slot").await?;
        wait_for("the occupied slot", Duration::from_secs(30), || {
            screen_contains(&app, &blocker, "ECHO: Occupy the slot")
        })
        .await?;
        let queued = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(queued.status, "queued");
        let kept = app
            .deps
            .store
            .queued_start(&id)
            .await?
            .ok_or("queued restart")?;
        let prompt = kept.prompt.ok_or("Continue queued an empty Codex resume")?;
        support::stop_tasks(&app, &[&blocker]).await?;
        wait_for("the resumed Codex answer", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: Continue")
        })
        .await?;
        let run = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("resumed run")?;
        assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        let sessions = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex/sessions");
        let path = app
            .deps
            .harness
            .find_codex_rollout(&sessions, &session)?
            .ok_or("rollout")?;
        let history = std::fs::read_to_string(path)?;
        assert_eq!(
            history
                .lines()
                .filter(|line| line.contains(r#""role":"user""#) && line.contains(&prompt))
                .count(),
            1
        );
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn reopening_a_closed_codex_conversation_reports_waiting_until_input() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        app.close_task_and_wait(&id).await?;
        let reopened = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(reopened.status, "running");
        assert_eq!(reopened.activity.as_deref(), Some("waiting"));
        let run = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("reopened run")?;
        assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        app.send_input(&id, b"/work 60000 Next turn\r").await?;
        wait_for("the new turn", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "WORKING").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(yulius_domain::task::Activity::Working))
        })
        .await?;
        app.send_input(&id, &[3]).await?;
        wait_for("the interrupted turn", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "INTERRUPTED").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(yulius_domain::task::Activity::Waiting))
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn continuing_a_codex_task_stopped_before_launch_delivers_its_original_request() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, blocker, _) = restart_fixture(&data).await?;
        let id = new_codex_task(&app, &sandbox, &user, "Never launched").await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
        support::stop_tasks(&app, &[&id]).await?;
        support::stop_tasks(&app, &[&blocker]).await?;
        let ctx = admin_ctx(&app, &user).await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        wait_for("the full original request", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: Never launched: answer with an echo")
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn a_stopped_codex_id_without_a_rollout_restarts_with_its_original_request() -> TestResult {
    restart_before_rollout(CapacityRestart::Automatic, QueuedRequest::Original)
}

#[test]
fn a_stopped_codex_id_without_a_rollout_moves_with_its_original_request() -> TestResult {
    restart_before_rollout(CapacityRestart::Move, QueuedRequest::Original)
}

#[test]
fn an_overtaken_codex_move_preserves_the_replacement_native_resume() -> TestResult {
    restart_before_rollout(
        CapacityRestart::CancelledMove {
            already_running: false,
        },
        QueuedRequest::Original,
    )
}

#[test]
fn an_overtaken_codex_move_reports_cancellation_after_the_replacement_runs() -> TestResult {
    restart_before_rollout(
        CapacityRestart::CancelledMove {
            already_running: true,
        },
        QueuedRequest::Original,
    )
}

#[test]
fn a_queued_goal_fresh_launch_keeps_its_request_after_capacity_returns() -> TestResult {
    restart_before_rollout(CapacityRestart::Automatic, QueuedRequest::GoalFresh)
}

#[test]
fn a_queued_goal_fresh_launch_keeps_its_request_on_a_subscription_move() -> TestResult {
    restart_before_rollout(CapacityRestart::Move, QueuedRequest::GoalFresh)
}

#[test]
fn a_queued_goal_resume_keeps_its_instruction_after_capacity_returns() -> TestResult {
    restart_before_rollout(CapacityRestart::Automatic, QueuedRequest::GoalResume)
}

#[test]
fn an_overtaken_codex_move_from_a_process_free_legacy_goal_hold_is_cancelled() -> TestResult {
    restart_before_rollout(
        CapacityRestart::CancelledMove {
            already_running: false,
        },
        QueuedRequest::LegacyGoal,
    )
}

#[test]
fn an_overtaken_codex_move_keeps_a_replacement_live_before_pause_classification() -> TestResult {
    restart_before_rollout(
        CapacityRestart::CancelledMove {
            already_running: true,
        },
        QueuedRequest::LegacyGoal,
    )
}

#[test]
fn a_queued_goal_with_oversized_spec_diagnostics_keeps_an_actionable_instruction() -> TestResult {
    restart_before_rollout(
        CapacityRestart::Automatic,
        QueuedRequest::OversizedGoalResume,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueuedRequest {
    Original,
    GoalFresh,
    GoalResume,
    OversizedGoalResume,
    LegacyGoal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapacityRestart {
    Automatic,
    Move,
    CancelledMove { already_running: bool },
}

fn restart_before_rollout(restart: CapacityRestart, request: QueuedRequest) -> TestResult {
    use yulius_app::model::{PauseKind, QueuedStart, QueuedStartKind};
    let goal_prompt = "Continue the authorized goal. Verify staging";
    let mode = if request == QueuedRequest::Original {
        TaskMode::Prompt
    } else {
        TaskMode::Goal
    };
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture_in(&data, mode).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let sessions = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex/sessions");
        let rollout = app
            .deps
            .harness
            .find_codex_rollout(&sessions, &session)?
            .ok_or("rollout")?;
        // Reproduce the persisted state after SessionStart and before the first rollout write.
        let saved = data.join("rollout.fixture");
        std::fs::rename(&rollout, &saved)?;
        let moved_sessions = data.join("sessions.fixture");
        std::fs::rename(&sessions, &moved_sessions)?;
        std::fs::write(&sessions, b"not a directory")?;
        let refused = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await;
        assert!(
            refused.is_err(),
            "an unreadable rollout directory started fresh"
        );
        assert!(matches!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(_)
        ));
        std::fs::remove_file(&sessions)?;
        std::fs::rename(moved_sessions, &sessions)?;
        let blocker = new_codex_task(&app, &sandbox, &user, "Occupy the slot").await?;
        wait_for("the occupied slot", Duration::from_secs(30), || {
            screen_contains(&app, &blocker, "ECHO: Occupy the slot")
        })
        .await?;
        if request == QueuedRequest::Original || request == QueuedRequest::LegacyGoal {
            let queued = app
                .continue_task(&ctx, id.as_str(), &Default::default())
                .await?;
            assert_eq!(queued.status, "queued");
            let kept = app
                .deps
                .store
                .queued_start(&id)
                .await?
                .ok_or("queued restart")?;
            assert_eq!(kept.kind, QueuedStartKind::Fresh);
            assert_eq!(kept.prompt, None);
        } else {
            // Restore the durable admission made for an authorized goal while the slot was full.
            let kind = if request == QueuedRequest::GoalFresh {
                QueuedStartKind::GoalFresh
            } else {
                QueuedStartKind::GoalResume
            };
            let prompt = if request == QueuedRequest::OversizedGoalResume {
                let spec = format!("## Plan\n{}", "- [ ] repeated step\n".repeat(2000));
                let report = yulius_domain::spec_format::validate_spec(&spec);
                let instruction = format!(
                    "Continue the authorized goal. {}",
                    report.agent_message("docs/specs/goal.md")
                );
                assert!(instruction.len() > yulius_app::capacity::CONTINUATION_MAX);
                instruction
            } else {
                goal_prompt.to_owned()
            };
            assert!(
                app.deps
                    .store
                    .queue_task(
                        &id,
                        task(&app, &id).await?.status,
                        &QueuedStart {
                            kind,
                            prompt: Some(prompt),
                            account_id: None
                        },
                        app.now()
                    )
                    .await?
            );
        }

        let account = AccountId::parse("a-codex")?;
        let usage = yulius_domain::usage::AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..Default::default()
        };
        app.deps
            .store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&yulius_app::usage_json::to_json(&usage)),
                None,
                app.now(),
            )
            .await?;
        support::stop_tasks(&app, &[&blocker]).await?;
        wait_for(
            "the admitted restart waiting for capacity",
            Duration::from_secs(30),
            || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
            },
        )
        .await?;
        // Even a late first write cannot turn the accepted fresh start into an idle resume.
        std::fs::rename(saved, rollout)?;
        let replacement =
            add_codex_account(&app, "replacement", &codex_auth("2026-11-01T10:00:00Z")).await?;
        if request == QueuedRequest::LegacyGoal {
            // An older launch persisted a turn hold even though it never started a process.
            assert!(
                app.deps
                    .store
                    .update_open_pause(&id, PauseKind::Turn, Some(goal_prompt))
                    .await?
            );
            assert!(app.deps.store.live_attempt(&id).await?.is_none());
        }
        if let CapacityRestart::CancelledMove { already_running } = restart {
            let before_classification = request == QueuedRequest::LegacyGoal && already_running;
            let point = if before_classification {
                yulius_app::race::Point::MovePauseRead
            } else {
                yulius_app::race::Point::SwitchPrepared
            };
            let (prepared, release) = support::hold_point(point, id.as_str());
            let moving = {
                let (app, id, account) = (Arc::clone(&app), id.clone(), replacement.clone());
                tokio::spawn(async move {
                    app.switch_task(
                        &ctx,
                        id.as_str(),
                        &yulius_contracts::api::tasks::ContinueTask {
                            account_id: Some(account.to_string()),
                            ..Default::default()
                        },
                    )
                    .await
                })
            };
            support::point_reached(prepared).await?;
            support::stop_tasks(&app, &[&id]).await?;
            let store = &app.deps.store;
            let limited = yulius_app::usage_json::to_json(&usage);
            store
                .set_account_state(
                    &replacement,
                    AccountStatus::Limited,
                    Some(&limited),
                    None,
                    app.now(),
                )
                .await?;
            app.continue_task(
                &admin_ctx(&app, &user).await?,
                id.as_str(),
                &Default::default(),
            )
            .await?;
            let next = store.open_pause(&id).await?.ok_or("new resume")?;
            assert_eq!(next.kind, yulius_app::model::PauseKind::Start);
            assert!(next.continuation.is_some());
            let available =
                yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage::default());
            store
                .set_account_state(
                    &replacement,
                    AccountStatus::Ok,
                    Some(&available),
                    None,
                    app.now(),
                )
                .await?;
            let replacement_run = if already_running {
                let _ = app.resume_round().await;
                Some(
                    store
                        .live_attempt(&id)
                        .await?
                        .ok_or("replacement already running")?
                        .id,
                )
            } else {
                None
            };
            release.send(())?;
            let moved = tokio::time::timeout(Duration::from_secs(60), moving).await??;
            assert!(matches!(
                moved,
                Err(yulius_app::error::AppError::Conflict(_))
            ));
            if !before_classification {
                assert!(
                    store
                        .switches(&id)
                        .await?
                        .iter()
                        .any(|switch| switch.reason == "manual" && switch.outcome == "cancelled")
                );
            }
            if !already_running {
                assert_eq!(
                    store.open_pause(&id).await?,
                    Some(next),
                    "the cancelled Move consumed the replacement"
                );
                let _ = app.resume_round().await;
            }
            let run = store.live_attempt(&id).await?.ok_or("new continuation")?;
            if let Some(replacement_run) = replacement_run {
                assert_eq!(run.id, replacement_run);
            }
            assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
            return support::stop_tasks(&app, &[&id]).await;
        }
        if restart == CapacityRestart::Move {
            app.switch_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some(replacement.to_string()),
                    ..Default::default()
                },
            )
            .await?;
        } else {
            let _ = app.resume_round().await;
        }
        let run = app.deps.store.live_attempt(&id).await?.ok_or("fresh run")?;
        let (kind, expected) = match request {
            QueuedRequest::GoalFresh => (LaunchKind::New, goal_prompt),
            QueuedRequest::GoalResume => (LaunchKind::Resume, goal_prompt),
            QueuedRequest::OversizedGoalResume => (LaunchKind::Resume, "spec_check"),
            _ => (
                LaunchKind::New,
                "ECHO: Original request: answer with an echo",
            ),
        };
        assert_eq!(run.launch_kind, kind);
        if kind == LaunchKind::Resume {
            assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        }
        wait_for("the saved launch prompt", Duration::from_secs(30), || {
            screen_contains(&app, &id, expected)
        })
        .await?;
        if mode == TaskMode::Goal {
            assert_eq!(task(&app, &id).await?.gate_blocks, 1);
        }
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn retrying_a_failed_codex_restart_resumes_the_same_conversation() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute_batch("CREATE TRIGGER reject_launch BEFORE INSERT ON attempts BEGIN SELECT RAISE(FAIL, 'injected launch failure'); END")?;
        let failed = app.continue_task(&ctx, id.as_str(), &Default::default()).await;
        assert!(failed.is_err());
        assert!(matches!(task(&app, &id).await?.status, TaskStatus::Failed(_)));
        db.execute_batch("DROP TRIGGER reject_launch")?;
        app.continue_task(&ctx, id.as_str(), &Default::default()).await?;
        wait_for("the retried continuation", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: Continue")
        }).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("retried run")?;
        assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn moving_idle_working_and_held_codex_turns_selects_their_own_prompts() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let original = AccountId::parse("a-codex")?;
        let alternate =
            add_codex_account(&app, "alternate", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let move_to = async |account: &AccountId| -> Result<String, Box<dyn std::error::Error>> {
            app.switch_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some(account.to_string()),
                    ..Default::default()
                },
            )
            .await?;
            let run = app.deps.store.live_attempt(&id).await?.ok_or("moved run")?;
            Ok(std::fs::read_to_string(
                Path::new(&run.run_dir).join("fake-launch.txt"),
            )?)
        };
        app.close_task_and_wait(&id).await?;
        let opened = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(opened.activity.as_deref(), Some("waiting"));
        let reopened = move_to(&alternate).await?;
        assert!(reopened.contains("prompt: None"), "{reopened}");
        app.send_input(&id, b"/work 60000 Active turn\r").await?;
        wait_for("the active Codex turn", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "WORKING").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(yulius_domain::task::Activity::Working))
        })
        .await?;
        let continued = move_to(&original).await?;
        assert!(
            continued.contains("Continue the task exactly where you left off."),
            "{continued}"
        );
        assert_eq!(
            task(&app, &id).await?.activity,
            Some(yulius_domain::task::Activity::Working)
        );

        let record = task(&app, &id).await?;
        let worktree = Path::new(record.worktree_path.as_deref().ok_or("worktree")?);
        std::fs::write(worktree.join("feature.rs"), "pub fn feature() {}\n")?;
        let staged = std::process::Command::new("git")
            .current_dir(worktree)
            .args(["add", "feature.rs"])
            .status()?;
        assert!(staged.success() && app.spec_required(&record).await?);
        app.pause_task(
            &id,
            yulius_app::model::PauseKind::Turn,
            Some(original.clone()),
            None,
        )
        .await?;
        let held = move_to(&alternate).await?;
        assert!(held.contains("This task needs a spec"), "{held}");
        let prompt = "The operator's held follow-up";
        app.pause_task(
            &id,
            yulius_app::model::PauseKind::Prompt,
            Some(alternate.clone()),
            Some(prompt.to_owned()),
        )
        .await?;
        let submitted = move_to(&original).await?;
        assert!(submitted.contains(prompt), "{submitted}");
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn a_manual_move_of_a_live_held_goal_keeps_its_turn_and_spends_one_attempt() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, session) = restart_fixture_in(&data, TaskMode::Goal).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let alternate =
            add_codex_account(&app, "alternate", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let task_before = task(&app, &id).await?;
        let worktree = Path::new(task_before.worktree_path.as_deref().ok_or("worktree")?);
        std::fs::write(worktree.join("feature.rs"), "pub fn feature() {}\n")?;
        assert!(
            std::process::Command::new("git")
                .current_dir(worktree)
                .args(["add", "feature.rs"])
                .status()?
                .success()
        );
        let store = &app.deps.store;
        let run = store.live_attempt(&id).await?.ok_or("held run")?;
        app.pause_task(
            &id,
            yulius_app::model::PauseKind::Turn,
            run.account_id,
            None,
        )
        .await?;
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some(alternate.to_string()),
                ..Default::default()
            },
        )
        .await?;
        let replacement = store.live_attempt(&id).await?.ok_or("replacement run")?;
        assert_ne!(replacement.id, run.id);
        assert_eq!(replacement.account_id, Some(alternate));
        assert_eq!(
            replacement.provider_session_id.as_deref(),
            Some(session.as_str())
        );
        let launch = Path::new(&replacement.run_dir).join("fake-launch.txt");
        wait_for("the moved held goal", Duration::from_secs(30), || async {
            std::fs::read_to_string(&launch).is_ok_and(|text| {
                text.contains("Continue the authorized goal.") && text.contains("needs a spec")
            })
        })
        .await?;
        assert_eq!(
            task(&app, &id).await?.gate_blocks,
            task_before.gate_blocks + 1
        );
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn process_free_capacity_retries_the_original_opening_and_resumes_a_later_prompt() -> TestResult {
    use yulius_app::model::PauseKind;
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, _, id, _) = restart_fixture(&data).await?;
        let store = &app.deps.store;
        let held = "Continue with the owner's follow-up";
        for (opening, kind) in [(true, LaunchKind::New), (false, LaunchKind::Resume)] {
            let run = store.live_attempt(&id).await?.ok_or("prior run")?;
            support::stop_tasks(&app, &[&id]).await?;
            if opening {
                std::fs::write(Path::new(&run.run_dir).join("opening-pending"), b"entered")?;
            }
            let stopped = task(&app, &id).await?.status;
            assert!(
                store
                    .transition_task(
                        &id,
                        Some(stopped),
                        TaskStatus::WaitingCapacity,
                        None,
                        app.now()
                    )
                    .await?
            );
            app.pause_task(&id, PauseKind::Prompt, None, Some(held.into()))
                .await?;
            app.resume_round().await;
            let resumed = store.live_attempt(&id).await?.ok_or("resumed hold")?;
            assert_eq!(resumed.launch_kind, kind);
            let expected = if opening {
                "ECHO: Original request: answer with an echo"
            } else {
                held
            };
            wait_for("the resumed prompt", Duration::from_secs(30), || {
                screen_contains(&app, &id, expected)
            })
            .await?;
        }
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn moving_a_codex_restart_waiting_for_capacity_keeps_its_continuation() -> TestResult {
    move_waiting_restart(false)
}

#[test]
fn moving_a_codex_restart_into_the_session_queue_keeps_the_chosen_account() -> TestResult {
    move_waiting_restart(true)
}

fn move_waiting_restart(queue: bool) -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let account = AccountId::parse("a-codex")?;
        let usage = yulius_domain::usage::AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..Default::default()
        };
        app.deps
            .store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&yulius_app::usage_json::to_json(&usage)),
                None,
                app.now(),
            )
            .await?;
        let waiting = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(waiting.status, "waiting_capacity");
        let replacement =
            add_codex_account(&app, "replacement", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let blocker = if queue {
            let blocker = new_codex_task(&app, &sandbox, &user, "Occupy the slot").await?;
            wait_for("the occupied slot", Duration::from_secs(30), || {
                screen_contains(&app, &blocker, "ECHO: Occupy the slot")
            })
            .await?;
            Some(blocker)
        } else {
            None
        };
        let moved = app
            .switch_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some(replacement.to_string()),
                    ..Default::default()
                },
            )
            .await?;
        if let Some(blocker) = &blocker {
            assert_eq!(moved.status, "queued");
            assert!(
                app.deps
                    .store
                    .switches(&id)
                    .await?
                    .iter()
                    .any(|switch| switch.reason == "manual" && switch.outcome == "waiting")
            );
            let queued = app
                .deps
                .store
                .queued_start(&id)
                .await?
                .ok_or("queued move")?;
            assert_eq!(queued.account_id.as_ref(), Some(&replacement));
            assert!(queued.prompt.is_some());
            support::stop_tasks(&app, &[blocker]).await?;
            wait_for("the queued move", Duration::from_secs(30), || {
                screen_contains(&app, &id, "ECHO: Continue")
            })
            .await?;
        } else {
            assert_eq!(moved.status, "running");
            assert!(
                app.deps
                    .store
                    .switches(&id)
                    .await?
                    .iter()
                    .any(|switch| switch.reason == "manual" && switch.outcome == "completed")
            );
        }
        let run = app.deps.store.live_attempt(&id).await?.ok_or("moved run")?;
        assert_eq!(run.account_id, Some(replacement));
        assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        let launch = std::fs::read_to_string(Path::new(&run.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("prompt: Some("),
            "move dropped the capacity continuation"
        );
        wait_for("the moved continuation", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: Continue")
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn stopping_an_admitted_capacity_restart_prevents_its_codex_turn() -> TestResult {
    stop_during_capacity_restart(false)
}

#[test]
fn a_cancelled_capacity_restart_cannot_claim_a_later_codex_pause() -> TestResult {
    stop_during_capacity_restart(true)
}

fn stop_during_capacity_restart(continue_again: bool) -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let account = AccountId::parse("a-codex")?;
        let limited = yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..Default::default()
        });
        let available =
            yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage::default());
        let store = &app.deps.store;
        store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&limited),
                None,
                app.now(),
            )
            .await?;
        let waiting = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        assert_eq!(waiting.status, "waiting_capacity");
        store
            .set_account_state(
                &account,
                AccountStatus::Ok,
                Some(&available),
                None,
                app.now(),
            )
            .await?;
        let (at, go) =
            support::hold_point(yulius_app::race::Point::CapacityStartReady, id.as_str());
        let worker = Arc::clone(&app);
        let recovering = tokio::spawn(async move { worker.resume_round().await });
        support::point_reached(at).await?;
        support::stop_tasks(&app, &[&id]).await?;
        if continue_again {
            store
                .set_account_state(
                    &account,
                    AccountStatus::Limited,
                    Some(&limited),
                    None,
                    app.now(),
                )
                .await?;
            let next = app
                .continue_task(&ctx, id.as_str(), &Default::default())
                .await?;
            assert_eq!(next.status, "waiting_capacity");
            store
                .set_account_state(
                    &account,
                    AccountStatus::Ok,
                    Some(&available),
                    None,
                    app.now(),
                )
                .await?;
        }
        go.send(())?;
        tokio::time::timeout(Duration::from_secs(60), recovering).await??;
        assert_eq!(
            store.attempts(&id).await?.len(),
            1,
            "cancelled recovery launched Codex"
        );
        let current = task(&app, &id).await?;
        if continue_again {
            assert_eq!(current.status, TaskStatus::WaitingCapacity);
            let _ = app.resume_round().await;
            wait_for(
                "the newly requested continuation",
                Duration::from_secs(30),
                || screen_contains(&app, &id, "ECHO: Continue"),
            )
            .await?;
            support::stop_tasks(&app, &[&id]).await?;
        } else {
            assert!(matches!(current.status, TaskStatus::Stopped(_)));
        }
        Ok(())
    })
}

#[test]
fn stop_winning_the_capacity_launch_claim_preserves_the_next_continue() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let account = AccountId::parse("a-codex")?;
        let limited = yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..Default::default()
        });
        let available =
            yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage::default());
        let store = &app.deps.store;
        store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&limited),
                None,
                app.now(),
            )
            .await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        store
            .set_account_state(
                &account,
                AccountStatus::Ok,
                Some(&available),
                None,
                app.now(),
            )
            .await?;
        let (checked, claim) =
            support::hold_point(yulius_app::race::Point::StartClaimReady, id.as_str());
        let (released, finish) =
            support::hold_point(yulius_app::race::Point::StartLockReleased, id.as_str());
        let worker = Arc::clone(&app);
        let round = tokio::spawn(async move { worker.resume_round().await });
        support::point_reached(checked).await?;
        support::stop_tasks(&app, &[&id]).await?;
        claim.send(())?;
        support::point_reached(released).await?;
        store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&limited),
                None,
                app.now(),
            )
            .await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        let replacement = store.open_pause(&id).await?.ok_or("replacement pause")?;
        finish.send(())?;
        tokio::time::timeout(Duration::from_secs(60), round).await??;
        assert_eq!(store.open_pause(&id).await?, Some(replacement));
        assert_eq!(store.attempts(&id).await?.len(), 1);
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn a_failed_fresh_codex_spawn_can_retry_its_original_request() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let sessions = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex/sessions");
        let path = app
            .deps
            .harness
            .find_codex_rollout(&sessions, &session)?
            .ok_or("rollout")?;
        std::fs::remove_file(path)?;
        let failing = App::new(
            app.config.clone(),
            yulius_app::context::Deps {
                session_host: Arc::new(yulius::wiring::pending::UnavailableSessionHost),
                ..app.deps.clone()
            },
        );
        assert!(
            failing
                .continue_task(&ctx, id.as_str(), &Default::default())
                .await
                .is_err()
        );
        let failed = app
            .deps
            .store
            .attempts(&id)
            .await?
            .pop()
            .ok_or("failed attempt")?;
        assert_eq!(failed.launch_kind, LaunchKind::New);
        assert_eq!(failed.provider_session_id, None);
        assert_eq!(failed.exit_reason.as_deref(), Some("spawn_failed"));
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        wait_for(
            "the retried original request",
            Duration::from_secs(30),
            || screen_contains(&app, &id, "ECHO: Original request: answer with an echo"),
        )
        .await?;
        support::stop_tasks(&app, &[&id]).await
    })
}

#[test]
fn a_stale_capacity_cleanup_leaves_the_new_codex_pause_and_request() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, _, user, id, _) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let account = AccountId::parse("a-codex")?;
        let limited = yulius_app::usage_json::to_json(&yulius_domain::usage::AccountUsage {
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..Default::default()
        });
        let store = &app.deps.store;
        store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&limited),
                None,
                app.now(),
            )
            .await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        let (observed, read_task) =
            support::hold_point(yulius_app::race::Point::CapacityPauseObserved, id.as_str());
        let (cleaning, finish) =
            support::hold_point(yulius_app::race::Point::CapacityStaleCleanup, id.as_str());
        let worker = Arc::clone(&app);
        let round = tokio::spawn(async move { worker.resume_round().await });
        support::point_reached(observed).await?;
        support::stop_tasks(&app, &[&id]).await?;
        read_task.send(())?;
        support::point_reached(cleaning).await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        let replacement = store.open_pause(&id).await?.ok_or("replacement pause")?;
        let request = serde_json::from_value(serde_json::json!({
            "action": "Sign a Codex subscription in again or add one in Subscriptions",
            "at": app.now().0,
        }))?;
        store
            .set_task_needs_you(&id, Some(&request), app.now())
            .await?;
        finish.send(())?;
        tokio::time::timeout(Duration::from_secs(60), round).await??;
        assert_eq!(store.open_pause(&id).await?, Some(replacement));
        assert_eq!(store.task_needs_you(&id).await?, Some(request));
        // An interruption can leave the observed row open after the task changes state.
        store
            .transition_task(
                &id,
                Some(TaskStatus::WaitingCapacity),
                TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled),
                None,
                app.now(),
            )
            .await?;
        let _ = app.resume_round().await;
        assert!(store.open_pause(&id).await?.is_none());
        assert!(store.task_needs_you(&id).await?.is_none());
        Ok(())
    })
}

#[test]
fn a_stopped_codex_task_with_only_stale_lineage_cannot_resume_the_wrong_session() -> TestResult {
    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture(&data).await?;
        let ctx = admin_ctx(&app, &user).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let set_id = "UPDATE attempts SET provider_session_id = ?1 WHERE task_id = ?2";
        db.execute(set_id, rusqlite::params![None::<String>, id.as_str()])?;
        for stale_time in [0, i64::MAX] {
            db.execute(
                "UPDATE provider_sessions SET session_id = 'old-session', first_seen = 0,
                 last_seen = ?1 WHERE task_id = ?2",
                rusqlite::params![stale_time, id.as_str()],
            )?;
            let refused = app
                .continue_task(&ctx, id.as_str(), &Default::default())
                .await;
            assert!(
                refused.is_err(),
                "Continue resumed stale history at {stale_time}"
            );
            assert!(matches!(
                task(&app, &id).await?.status,
                TaskStatus::Stopped(_)
            ));
        }
        // A different task already owns the attempt's recorded ID. Repair may not claim it.
        let other = new_codex_task(&app, &sandbox, &user, "Another conversation").await?;
        wait_for("the other rollout", Duration::from_secs(30), || async {
            app.deps
                .store
                .provider_sessions(&other)
                .await
                .is_ok_and(|rows| !rows.is_empty())
        })
        .await?;
        let other_session = app
            .deps
            .store
            .provider_sessions(&other)
            .await?
            .pop()
            .ok_or("other session")?
            .session_id;
        support::stop_tasks(&app, &[&other]).await?;
        db.execute(set_id, [other_session.as_str(), id.as_str()])?;
        let refused = app
            .continue_task(&ctx, id.as_str(), &Default::default())
            .await;
        assert!(
            refused.is_err(),
            "Continue accepted an ID owned by another task"
        );
        db.execute(
            "UPDATE provider_sessions SET last_seen = 0 WHERE task_id = ?1",
            [id.as_str()],
        )?;
        db.execute(set_id, [session.as_str(), id.as_str()])?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        let run = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("recovered run")?;
        assert_eq!(run.provider_session_id.as_deref(), Some(session.as_str()));
        support::stop_tasks(&app, &[&id]).await
    })
}

/// SPEC-503 CDX-002, CDX-003, CDX-004, CDX-006, AC-503-1/3/5: a Codex task runs Codex on a Codex
/// account (never the Claude one) in `<sandbox>/codex/<account>`, whose `auth.json`, `config.toml`
/// and `hooks.json` are private and whose `sessions` is the sandbox's; its session is learnt from
/// the rollout; its turns, usage and rate limits are read; a restart reattaches the same process
/// and reads the rollout again; and when the run ends, the credentials Codex refreshed meanwhile
/// replace the stored ones.
#[test]
fn a_codex_task_runs_in_its_home_survives_a_restart_and_keeps_refreshed_credentials() -> TestResult
{
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (task_id, account, child_pid, home) = first.block_on(async {
        let app = build_full(&data, Some(&fake), agent.clone(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex echo").await?;
        wait_for("the fake agent banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        assert_eq!(attempt.harness, Harness::Codex);
        assert_eq!(
            attempt.account_id.as_ref(),
            Some(&account),
            "a Codex account"
        );
        assert_eq!(attempt.provider_session_id, None, "Codex names its session");

        let sandbox_dir = data.join("sandboxes").join(sandbox.as_str());
        let home = sandbox_dir.join("codex").join(account.as_str());
        let auth = std::fs::read_to_string(home.join("auth.json"))?;
        assert_eq!(auth, codex_auth("2026-11-01T10:00:00Z"));
        for file in ["auth.json", "config.toml", "hooks.json"] {
            let mode = std::fs::metadata(home.join(file))?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file}");
        }
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        let config = std::fs::read_to_string(home.join("config.toml"))?;
        assert!(
            config.contains(&format!(
                "[projects.\"{worktree}\"]\ntrust_level = \"trusted\""
            )),
            "{config}"
        );
        assert_eq!(
            std::fs::read_link(home.join("sessions"))?,
            PathBuf::from("../sessions")
        );
        let hooks: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("hooks.json"))?)?;
        assert!(
            hooks["hooks"]["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.ends_with("hook codex Stop")),
            "{hooks}"
        );

        app.send_input(&id, b"hello codex\r").await?;
        wait_for("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello codex")
        })
        .await?;
        // The rollout is in the sandbox's shared sessions, found by the id Codex gave it.
        let session = attempt.id.as_str().to_owned();
        wait_for(
            "the session learnt from the rollout",
            Duration::from_secs(30),
            || async {
                app.deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|a| a.provider_session_id)
                    .as_deref()
                    == Some(session.as_str())
            },
        )
        .await?;
        // The lineage records it in the same watch round, right after the attempt.
        wait_for(
            "the session in the lineage",
            Duration::from_secs(10),
            || async {
                app.deps
                    .store
                    .provider_sessions(&id)
                    .await
                    .unwrap_or_default()
                    .iter()
                    .any(|l| l.session_id == session && l.harness == Harness::Codex)
            },
        )
        .await?;
        assert!(
            sandbox_dir
                .join("codex/sessions/2026/11/09")
                .join(format!("rollout-2026-11-09T00-00-00-{session}.jsonl"))
                .is_file()
        );
        // Its usage and the account's rate limits come from the rollout.
        wait_for("the run's tokens", Duration::from_secs(30), || async {
            saved_output(&data, attempt.id.as_str()).is_some_and(|n| n > 0)
        })
        .await?;
        wait_for("the account's window", Duration::from_secs(30), || async {
            app.deps
                .store
                .account(&account)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.usage_json)
                .is_some_and(|json| {
                    yulius_app::usage_json::from_json(&json)
                        .primary
                        .and_then(|w| w.used_percent)
                        == Some(10.0)
                })
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>((id, account, attempt.child_pid, home))
    })?;
    // The server "crashes"; the run goes on in its host.
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        app.reconcile().await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&task_id)
            .await?
            .ok_or("attempt lost on restart")?;
        assert_eq!(attempt.child_pid, child_pid, "same process, not respawned");
        // The start's sweep of left-behind sign-ins keeps the one the reattached run uses.
        assert!(
            home.join("auth.json").is_file(),
            "the sweep removed the live run's sign-in"
        );
        app.send_input(&task_id, b"again\r").await?;
        wait_for("the echo after restart", Duration::from_secs(10), || {
            screen_contains(&app, &task_id, "ECHO: again")
        })
        .await?;
        // Codex refreshed its tokens during the run: the newer file is kept when the run ends,
        // and the account, limited meanwhile, keeps its state.
        let refreshed = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &refreshed)?;
        app.deps
            .store
            .set_account_state(&account, AccountStatus::Limited, None, None, app.now())
            .await?;
        app.send_input(&task_id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &task_id)
                .await
                .is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        wait_for(
            "the written-back credentials",
            Duration::from_secs(10),
            || async {
                stored_credential(&app, &account)
                    .await
                    .is_ok_and(|stored| stored == refreshed)
            },
        )
        .await?;
        let stored = app.deps.store.account(&account).await?.ok_or("account")?;
        assert_eq!(
            stored.status,
            AccountStatus::Limited,
            "the account keeps its state"
        );
        // No run of the account is left in the sandbox: its credential does not stay there.
        wait_for(
            "no auth.json left in the sandbox",
            Duration::from_secs(30),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        assert!(home.join("config.toml").exists());
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006: the Codex home's `auth.json` is the agent's to write, so only a genuine
/// refresh goes back into the vault. A newer one another run of the account left in the sandbox
/// is taken at the next launch instead of being overwritten with the vault's older copy; one the
/// agent forged (another ChatGPT account) is never taken.
#[test]
fn only_a_genuine_codex_refresh_reaches_the_vault() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        // Another run of the account refreshed its tokens and is still going: its file is newer.
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        std::fs::create_dir_all(&home)?;
        let left = codex_auth(&ago(120_000));
        std::fs::write(home.join("auth.json"), &left)?;
        // What Yulius knows of the subscription besides its tokens (slice C1 review): a refresh
        // of the same sign-in changes none of it, and never writes the status it read before.
        {
            let record = app.deps.store.account(&account).await?.ok_or("account")?;
            let sealed = record.credential.ok_or("credential")?;
            app.deps
                .store
                .set_account_email(&account, &sealed, Some("dev@example.com"), None, app.now())
                .await?;
            app.deps
                .store
                .set_account_state(
                    &account,
                    AccountStatus::Ok,
                    None,
                    Some("usage: read later"),
                    app.now(),
                )
                .await?;
        }
        let id = new_codex_task(&app, &sandbox, &user, "Codex refresh").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, left);
        assert_eq!(stored_credential(&app, &account).await?, left);
        let record = app.deps.store.account(&account).await?.ok_or("account")?;
        assert_eq!(record.email.as_deref(), Some("dev@example.com"));
        assert_eq!(record.last_error.as_deref(), Some("usage: read later"));
        // The agent writes a sign-in of its own: it is not taken when the run ends.
        let forged = codex_auth(&ago(1_000)).replace("acc-synthetic", "acc-attacker");
        std::fs::write(home.join("auth.json"), &forged)?;
        // What the run's write-back decided is read once it is done (slice C1 review).
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        yulius_app::race::on(
            yulius_app::race::Point::CodexWriteBackDone,
            Path::new(id.as_str()),
            move || {
                let _ = done_tx.send(());
            },
        );
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        assert_eq!(stored_credential(&app, &account).await?, left);
        // A file that did not reach the vault is not removed: were it a genuine refresh that
        // could not be stored, it is the only copy of the rotated tokens.
        assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, forged);
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// An administrator's context for `user`.
async fn admin_ctx(
    app: &App,
    user: &UserId,
) -> Result<yulius_app::auth::AuthContext, Box<dyn std::error::Error>> {
    Ok(yulius_app::auth::AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// SPEC-503 CLI-002, AC-503-6: a Codex task starts the Codex CLI Yulius provides by its absolute
/// path, never `codex` from a `PATH` that may not have it.
#[test]
fn a_codex_task_starts_the_provided_codex_cli() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let dir = data.join("codex-cli").join("0.169.0-linux-x64");
    let program = dir.join("bin/codex");
    let agent = FakeAgentOptions {
        provided_codex: Some(yulius_app::ports::harness::ProvidedCli {
            program: program.clone(),
            dir,
        }),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex provided").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains(&format!("program=Some({:?})", program.to_string_lossy())),
            "{launch}"
        );
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CLI-002, AC-503-6 (Linux): a contained Codex run sees the provided Codex CLI's
/// directory, bound read-only at its own path, though the rest of the data directory stays hidden.
/// Skipped with a message where bubblewrap cannot create namespaces.
#[cfg(target_os = "linux")]
#[test]
fn a_contained_codex_run_sees_the_provided_codex_cli() -> TestResult {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.as_ref().is_ok_and(|out| out.status.success()) {
        eprintln!(
            "SKIPPED a_contained_codex_run_sees_the_provided_codex_cli: bwrap cannot create namespaces"
        );
        return Ok(());
    }
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let (_dir, data) = temp()?;
    let dir = data.join("codex-cli").join("0.169.0-linux-x64");
    std::fs::create_dir_all(dir.join("bin"))?;
    std::fs::write(dir.join("bin/codex"), b"provided")?;
    let agent = FakeAgentOptions {
        provided_codex: Some(yulius_app::ports::harness::ProvidedCli {
            program: dir.join("bin/codex"),
            dir: dir.clone(),
        }),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = crate::supervision::build_tuned(&data, Some(&fake), agent, move |c| {
            c.contained = true;
            c.contained_readonly = vec![fake_dir];
        })?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex contained").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        for (path, expected) in [
            (dir.join("bin/codex"), "READ codex OK"),
            (data.join("yulius.db"), "READ yulius.db ERR"),
        ] {
            app.send_input(&id, format!("/read {}\r", path.display()).as_bytes())
                .await?;
            wait_for(expected, Duration::from_secs(10), || {
                screen_contains(&app, &id, expected)
            })
            .await?;
        }
        crate::support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CLI-002: a Codex CLI that cannot be provided fails the start (its reason goes to the
/// terminal's "Could not start" notice) before anything is spawned, instead of a run that dies
/// with `codex: not found`.
#[test]
fn a_codex_start_without_a_codex_cli_says_why() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let agent = FakeAgentOptions {
        codex_unavailable: Some("the Codex CLI 0.169.0 could not be installed (offline)".into()),
        codex_gate: Some(Arc::clone(&gate)),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex offline").await?;
        // Watched before the start may go on, so what it says is heard.
        let mut watch = app.runtime(&id).hub.watch();
        gate.add_permits(8);
        wait_for("the failed start", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| matches!(t.status, TaskStatus::Failed(_)))
        })
        .await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none(), "no run");
        assert!(
            app.deps.store.attempts(&id).await?.is_empty(),
            "nothing was spawned"
        );
        // The terminal says why, never `codex: not found`.
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n
                    == "Could not start: the Codex CLI 0.169.0 could not be installed (offline)"),
            "{said:?}"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-009, AC-503-2: a Codex task at its usage limit moves to another Codex account by
/// a native resume of the same session, never to the Claude account, and the limited account is
/// marked; Codex does not go on by itself, so the resume asks it to.
#[test]
fn a_codex_task_at_its_limit_moves_to_another_codex_account() -> TestResult {
    codex_rotation_uses_task_effort(None)
}

#[test]
fn a_codex_rotation_uses_the_max_effort_chosen_after_launch() -> TestResult {
    codex_rotation_uses_task_effort(Some("max"))
}

fn codex_rotation_uses_task_effort(effort_override: Option<&str>) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        let one = add_codex_account(&app, "a-codex-1", &auth).await?;
        let two = add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex rotation").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        if let Some(effort) = effort_override {
            let ctx = admin_ctx(&app, &user).await?;
            app.set_task_model(&ctx, id.as_str(), None, Some(effort))
                .await?;
        }
        app.send_input(&id, b"hello\r").await?;
        wait_for(
            "the switch to another account",
            Duration::from_secs(90),
            || async {
                let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                attempts.len() >= 2 && attempts.last().is_some_and(|a| !a.state.is_terminal())
            },
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (first, second) = (&attempts[0], &attempts[1]);
        let accounts = [Some(one.clone()), Some(two.clone())];
        assert!(accounts.contains(&first.account_id), "{first:?}");
        assert!(accounts.contains(&second.account_id), "{second:?}");
        assert_ne!(first.account_id, second.account_id, "another Codex account");
        assert_eq!(second.harness, Harness::Codex, "the task keeps its harness");
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert!(first.provider_session_id.is_some());
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same session"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        let initial = std::fs::read_to_string(Path::new(&first.run_dir).join("fake-launch.txt"))?;
        assert!(
            initial
                .lines()
                .any(|line| line == r#"-c model_reasoning_effort="high""#),
            "{initial}"
        );
        let resumed_effort = effort_override.unwrap_or("high");
        let effort_arg = format!(r#"-c model_reasoning_effort="{resumed_effort}""#);
        assert!(launch.lines().any(|line| line == effort_arg), "{launch}");
        assert!(
            launch.contains("Resume {") && launch.contains("continue_interrupted: true"),
            "{launch}"
        );
        wait_for("the resumed run", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        let limited = app
            .deps
            .store
            .account(first.account_id.as_ref().ok_or("first account")?)
            .await?
            .ok_or("the first account")?;
        assert_eq!(limited.status, AccountStatus::Limited);
        let claude = app
            .deps
            .store
            .account(&AccountId::parse("a-claude")?)
            .await?
            .ok_or("claude")?;
        assert_eq!(
            claude.status,
            AccountStatus::Ok,
            "the Claude account is untouched"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-009: with no Codex account left with capacity, a Codex task waits instead of
/// running on the Claude account.
#[test]
fn a_codex_task_waits_while_no_codex_account_has_capacity() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let full = yulius_domain::usage::AccountUsage {
            primary: Some(yulius_domain::usage::UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(4_000_000_000_000)),
            }),
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..yulius_domain::usage::AccountUsage::default()
        };
        app.deps
            .store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&yulius_app::usage_json::to_json(&full)),
                None,
                app.now(),
            )
            .await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex waits").await?;
        wait_for("the pause", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
        })
        .await?;
        assert!(
            app.deps.store.attempts(&id).await?.is_empty(),
            "no run on the Claude account"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The notices said so far on a watch of the task's terminal.
fn notices(watch: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
    said
}

/// SPEC-503 CDX-013, AC-503-7, with SPEC-447 (MOV-001, MOV-003, MOV-006): a Codex task moves only
/// between Codex subscriptions. A person's move to the Claude subscription is refused naming why;
/// removing its Codex subscription moves it to the other Codex one in the same session, never to
/// the Claude one that has room; removing that one too leaves it waiting, asking for a Codex
/// subscription.
#[test]
fn a_codex_task_moves_only_between_codex_subscriptions() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        add_codex_account(&app, "a-codex-1", &auth).await?;
        add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex moves").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?;
        let on = first[0].account_id.clone().ok_or("the first account")?;
        let other = if on.as_str() == "a-codex-1" {
            "a-codex-2"
        } else {
            "a-codex-1"
        };

        let refused = app
            .switch_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some("a-claude".into()),
                    ..Default::default()
                },
            )
            .await;
        match refused {
            Err(yulius_app::error::AppError::Capacity(text)) => {
                assert!(text.contains("is not a Codex subscription"), "{text}");
            }
            other => return Err(format!("expected a refusal, got {other:?}").into()),
        }
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "nothing moved"
        );

        let mut watch = app.runtime(&id).hub.watch();
        app.delete_account(&ctx, on.as_str()).await?;
        let mut said = Vec::new();
        wait_for(
            "the move to the other Codex subscription",
            Duration::from_secs(60),
            || {
                said.extend(notices(&mut watch));
                let app = app.clone();
                let id = id.clone();
                async move {
                    let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                    attempts.len() == 2 && !attempts[1].state.is_terminal()
                }
            },
        )
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some(other)
        );
        assert_eq!(
            attempts[1].harness,
            Harness::Codex,
            "the task keeps its harness"
        );
        assert_eq!(
            attempts[1].provider_session_id,
            attempts[0].provider_session_id
        );
        wait_for("the removal's notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("was removed: this session continues on Codex"));
            async move { done }
        })
        .await?;
        assert!(
            !said.iter().any(|n| n.contains("Claude")),
            "a Codex task's notices never speak of Claude: {said:?}"
        );

        app.delete_account(&ctx, other).await?;
        // The task waits, and then asks for a subscription (recorded after the wait begins).
        wait_for(
            "the wait and its request",
            Duration::from_secs(60),
            || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
                    && app
                        .deps
                        .store
                        .task_needs_you(&id)
                        .await
                        .is_ok_and(|r| r.is_some())
            },
        )
        .await?;
        let runs = app.deps.store.attempts(&id).await?;
        assert!(
            runs.iter()
                .all(|a| a.account_id.as_ref().map(AccountId::as_str) != Some("a-claude")),
            "never on the Claude subscription: {runs:?}"
        );
        let request = app
            .deps
            .store
            .task_needs_you(&id)
            .await?
            .ok_or("Needs you")?;
        assert!(
            request.action.contains("Codex subscription"),
            "{}",
            request.action
        );
        wait_for("the stranded notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("No Codex subscription is left"));
            async move { done }
        })
        .await?;
        assert!(
            !said.iter().any(|n| n.contains("Claude")),
            "a Codex task's notices never speak of Claude: {said:?}"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-013 and CDX-006: Continue on a Claude subscription is refused up front for a
/// Codex task, which keeps resting instead of failing; and a Codex start whose subscription is
/// removed just after it was chosen (SPEC-447's race point) starts on the other Codex one without
/// leaving the removed one's credentials in the sandbox.
#[test]
fn a_codex_start_that_loses_its_subscription_leaves_no_credentials() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let auth = codex_auth("2026-11-01T10:00:00Z");
        add_codex_account(&app, "a-codex-1", &auth).await?;
        add_codex_account(&app, "a-codex-2", &auth).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex race").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let on = app.deps.store.attempts(&id).await?[0]
            .account_id
            .clone()
            .ok_or("the first account")?;
        let other = if on.as_str() == "a-codex-1" {
            "a-codex-2"
        } else {
            "a-codex-1"
        };
        support::stop_tasks(&app, &[&id]).await?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(on.as_str());
        wait_for(
            "the run's credentials removed",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;

        let resting = task(&app, &id).await?.status;
        let refused = app
            .continue_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some("a-claude".into()),
                    ..Default::default()
                },
            )
            .await;
        match refused {
            Err(yulius_app::error::AppError::Capacity(text)) => {
                assert!(text.contains("is not a Codex subscription"), "{text}");
            }
            other => return Err(format!("expected a refusal, got {other:?}").into()),
        }
        assert_eq!(
            task(&app, &id).await?.status,
            resting,
            "the task still rests"
        );
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "nothing started"
        );

        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::AccountChosen,
            Path::new(id.as_str()),
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(60));
            },
        );
        let continuing = {
            let (app, ctx, id, on) = (app.clone(), ctx.clone(), id.clone(), on.clone());
            tokio::spawn(async move {
                app.continue_task(
                    &ctx,
                    id.as_str(),
                    &yulius_contracts::api::tasks::ContinueTask {
                        account_id: Some(on.as_str().to_owned()),
                        ..Default::default()
                    },
                )
                .await
            })
        };
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(60)))
            .await?
            .map_err(|_| "the launch never chose its subscription")?;
        app.delete_account(&ctx, on.as_str()).await?;
        go_tx.send(())?;
        continuing.await??;
        wait_for(
            "the run on the other Codex subscription",
            Duration::from_secs(60),
            || async {
                app.deps
                    .store
                    .live_attempt(&id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|a| a.account_id.as_ref().map(AccountId::as_str) == Some(other))
            },
        )
        .await?;
        wait_for(
            "the removed subscription's credentials gone",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006: when the account is gone by the run's end (removed or
/// signed out meanwhile), its credentials do not stay in the sandbox for later tasks to read.
#[test]
fn a_gone_accounts_codex_credentials_leave_the_sandbox() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex gone").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        // Codex refreshed meanwhile, so the file differs from what the vault held.
        std::fs::write(home.join("auth.json"), codex_auth(&ago(1_000)))?;
        app.deps.store.delete_account(&account).await?;
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        wait_for(
            "the credentials removed",
            Duration::from_secs(10),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C1 review): a Codex run Yulius records as lost (here its processes died
/// with the server, and the next server finds them gone) still gets its write-back: the refreshed
/// `auth.json` reaches the vault and the file leaves the sandbox, as after a run that ended.
#[test]
fn a_lost_codex_run_keeps_its_refresh_and_leaves_no_credentials() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let first = runtime()?;
    let (task_id, account, pids, home) = first.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task(&app, &sandbox, &user, "Codex lost").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        // The host first, so it records no exit of the agent: the next server finds the run lost.
        let pids: Vec<i64> = [attempt.ptyhost_pid, attempt.child_pid]
            .into_iter()
            .flatten()
            .map(i64::from)
            .collect();
        Ok::<_, Box<dyn std::error::Error>>((id, account, pids, home))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    // Codex refreshed the sign-in during the run; then the run and its host died.
    let refreshed = codex_auth(&ago(60_000));
    std::fs::write(home.join("auth.json"), &refreshed)?;
    for pid in &pids {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    // Gone, or a zombie nobody reaped yet: no process runs.
    let alive = |pid: &i64| {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z'))
        })
    };
    while pids.iter().any(alive) {
        if std::time::Instant::now() >= deadline {
            return Err("the run's processes did not end".into());
        }
        std::thread::yield_now();
    }
    let second = runtime()?;
    second.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        app.reconcile().await?;
        wait_for("the task rests", Duration::from_secs(30), || async {
            task(&app, &task_id)
                .await
                .is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        wait_for(
            "the refresh in the vault and no auth.json left",
            Duration::from_secs(30),
            || async {
                !home.join("auth.json").exists()
                    && stored_credential(&app, &account)
                        .await
                        .is_ok_and(|stored| stored == refreshed)
            },
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C1 review): the file stays while another run of the subscription lives
/// in the sandbox, and goes once the last one ended: two Codex tasks of one sandbox on one
/// subscription, the usual case.
#[test]
fn the_file_stays_while_another_run_of_the_subscription_lives() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        let first = new_codex_task(&app, &sandbox, &user, "Codex one").await?;
        let second = new_codex_task(&app, &sandbox, &user, "Codex two").await?;
        for id in [&first, &second] {
            wait_for("the banner", Duration::from_secs(30), || {
                screen_contains(&app, id, "FAKE-AGENT READY")
            })
            .await?;
        }
        // The first run refreshed the sign-in and ends: its refresh reaches the vault.
        let refreshed = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &refreshed)?;
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        yulius_app::race::on(
            yulius_app::race::Point::CodexWriteBackDone,
            Path::new(first.as_str()),
            move || {
                let _ = done_tx.send(());
            },
        );
        app.send_input(&first, b"/exit\r").await?;
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        assert_eq!(stored_credential(&app, &account).await?, refreshed);
        // The second run still uses the file: the first run's write-back left it.
        assert!(
            home.join("auth.json").exists(),
            "auth.json removed under a live run of the subscription"
        );
        assert!(task(&app, &second).await?.status.is_live());
        app.send_input(&second, b"/exit\r").await?;
        wait_for(
            "no auth.json once the last run ended",
            Duration::from_secs(30),
            || async { !home.join("auth.json").exists() },
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 (slice C1 review): a Codex task in goal mode, the default for a task from the API and
/// every automation's, gets its whole request as its first message: no `/goal`, which Codex takes
/// only typed, and no goal for Yulius to watch.
#[test]
fn a_codex_goal_task_gets_its_request_as_its_first_message() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let agent = FakeAgentOptions {
        codex_gate: Some(Arc::clone(&gate)),
        ..FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_codex_task_in(
            &app,
            &sandbox,
            &user,
            "Codex goal",
            yulius_app::model::TaskMode::Goal,
        )
        .await?;
        // Watched before the start may go on, so what its launch says is heard.
        let mut watch = app.runtime(&id).hub.watch();
        gate.add_permits(8);
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        // Its default is no Claude model of the fallback's: nothing says one is used up.
        let said = notices(&mut watch);
        assert!(
            !said
                .iter()
                .any(|n| n.contains("used up on every subscription")),
            "{said:?}"
        );
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let run_dir = Path::new(&attempt.run_dir);
        let launch = std::fs::read_to_string(run_dir.join("fake-launch.txt"))?;
        assert!(
            launch.contains("Codex goal: answer with an echo"),
            "{launch}"
        );
        assert!(
            launch.lines().any(|line| line == "-m gpt-6.2-astra"),
            "{launch}"
        );
        assert!(
            launch
                .lines()
                .any(|line| line == r#"-c model_reasoning_effort="high""#),
            "{launch}"
        );
        assert!(!launch.contains("/goal"), "{launch}");
        for marker in ["launch-goal-pending", "typed-goal-pending"] {
            assert!(!run_dir.join(marker).exists(), "{marker}");
        }
        // Without a model or effort of its own it runs GPT-6.2 Astra at high effort, recorded for
        // the run and named for the task, never a Claude model (SPEC-511 CXD-002, CDX-013).
        assert_eq!(attempt.model.as_deref(), Some("gpt-6.2-astra"));
        let ctx = admin_ctx(&app, &user).await?;
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(detail.model, "gpt-6.2-astra");
        assert!(detail.model_default);
        assert_eq!(detail.effort.as_deref(), Some("high"));
        assert!(detail.effort_default);
        // The pool weighs the task by the model its run was launched with, so a subscription
        // it is at the limit of is left out (SPEC-511 review): one family for both.
        let record = task(&app, &id).await?;
        let input = app.rotation_input(&record, Some(account.clone())).await?;
        assert_eq!(input.running, Some(input.family));
        assert_eq!(input.family, yulius_domain::usage::ModelFamily::Any);
        // Its turns end it as a prompt-mode task's do: with every Codex subscription used up, a
        // turn's Stop is not held for capacity as a goal's would be, so the next line is read.
        let full = yulius_domain::usage::AccountUsage {
            primary: Some(yulius_domain::usage::UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(4_000_000_000_000)),
            }),
            limit_reached: true,
            limit_resets_at: Some(Timestamp(4_000_000_000_000)),
            ..yulius_domain::usage::AccountUsage::default()
        };
        app.deps
            .store
            .set_account_state(
                &account,
                AccountStatus::Limited,
                Some(&yulius_app::usage_json::to_json(&full)),
                None,
                app.now(),
            )
            .await?;
        app.send_input(&id, b"first-after-full\r").await?;
        wait_for("its echo", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: first-after-full")
        })
        .await?;
        app.send_input(&id, b"second-after-full\r").await?;
        wait_for("the next line read", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: second-after-full")
        })
        .await?;
        let status = task(&app, &id).await?.status;
        assert_ne!(
            status,
            TaskStatus::WaitingCapacity,
            "paused at its Stop like a goal"
        );
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 (slice C1 review): a Codex task takes no Claude model later either: setting one is
/// refused as at creation, since the run would fail at start; going back to the default works.
#[test]
fn a_codex_task_refuses_a_claude_model_set_later() -> TestResult {
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, None, FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let record = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Codex later: answer with an echo".into(),
                harness: Harness::Codex,
                title: Some("Codex later".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: false,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
            })
            .await?;
        let ctx = admin_ctx(&app, &user).await?;
        let refused = app
            .set_task_model(&ctx, record.id.as_str(), Some("claude-opus-5-5"), None)
            .await;
        assert!(
            matches!(refused, Err(yulius_app::error::AppError::Invalid(ref why)) if why.contains("OpenAI model")),
            "{refused:?}"
        );
        assert_eq!(task(&app, &record.id).await?.model, None);
        app.set_task_model(&ctx, record.id.as_str(), Some("gpt-synthetic"), None)
            .await?;
        assert_eq!(
            task(&app, &record.id).await?.model.as_deref(),
            Some("gpt-synthetic")
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C1 review): a sign-in made between the read of a run's refresh and its
/// write wins: the refresh, of the sign-in before, is not stored over it.
#[test]
fn a_sign_in_made_meanwhile_wins_over_a_runs_refresh() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        let id = new_codex_task(&app, &sandbox, &user, "Codex race").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        std::fs::write(home.join("auth.json"), codex_auth(&ago(60_000)))?;
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::CodexRefreshTaken,
            Path::new(account.as_str()),
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(60));
            },
        );
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        yulius_app::race::on(
            yulius_app::race::Point::CodexWriteBackDone,
            Path::new(id.as_str()),
            move || {
                let _ = done_tx.send(());
            },
        );
        app.send_input(&id, b"/exit\r").await?;
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        // An administrator signs the subscription in again right then.
        let signed_in = codex_auth(&ago(1_000)).replace("acc-synthetic", "acc-new");
        let sealed = app.deps.crypto.seal(signed_in.as_bytes())?;
        app.deps
            .store
            .set_account_credential(&account, &sealed, AccountStatus::Ok, app.now())
            .await?;
        let _ = go_tx.send(());
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        assert_eq!(stored_credential(&app, &account).await?, signed_in);
        // The run's file holds the replaced sign-in's tokens, which are never taken: it does
        // not stay in the sandbox (slice C1 review round 3).
        assert!(
            !home.join("auth.json").exists(),
            "the superseded auth.json stayed"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C2 review): another run of the subscription (in another sandbox)
/// stored its refresh between this run's read and its write. This run's refresh, the newer of the
/// same sign-in, is still kept: losing the swap to an older refresh is no reason to drop it.
#[test]
fn a_newer_refresh_is_kept_after_an_older_one_was_stored_first() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        let id = new_codex_task(&app, &sandbox, &user, "Codex newer").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let newer = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &newer)?;
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::CodexRefreshTaken,
            Path::new(account.as_str()),
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(60));
            },
        );
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        yulius_app::race::on(
            yulius_app::race::Point::CodexWriteBackDone,
            Path::new(id.as_str()),
            move || {
                let _ = done_tx.send(());
            },
        );
        app.send_input(&id, b"/exit\r").await?;
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        // The other run's refresh, older than this run's, is stored right then.
        let older = codex_auth(&ago(120_000));
        let previous = app
            .deps
            .store
            .account(&account)
            .await?
            .and_then(|a| a.credential)
            .ok_or("no credential")?;
        let sealed = app.deps.crypto.seal(older.as_bytes())?;
        assert!(
            app.deps
                .store
                .refresh_account_credential(&account, &previous, &sealed, app.now())
                .await?
        );
        let _ = go_tx.send(());
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        assert_eq!(stored_credential(&app, &account).await?, newer);
        assert!(
            !home.join("auth.json").exists(),
            "the kept auth.json stayed"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C2 review round 2): the swap was lost and what the account holds now
/// cannot be told (a credential that does not open, as after a failed read): the run's file may
/// be the only copy of rotated tokens, so it stays, as when the swap itself fails.
#[test]
fn a_refresh_stays_when_what_the_account_holds_cannot_be_read() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        seed_repo(&data, &sandbox)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        let id = new_codex_task(&app, &sandbox, &user, "Codex unread").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let newer = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &newer)?;
        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::CodexRefreshTaken,
            Path::new(account.as_str()),
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(Duration::from_secs(60));
            },
        );
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        yulius_app::race::on(
            yulius_app::race::Point::CodexWriteBackDone,
            Path::new(id.as_str()),
            move || {
                let _ = done_tx.send(());
            },
        );
        app.send_input(&id, b"/exit\r").await?;
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        // Right then the account holds a credential this server cannot open.
        let previous = app
            .deps
            .store
            .account(&account)
            .await?
            .and_then(|a| a.credential)
            .ok_or("no credential")?;
        // Sealed in the right format, but its last byte altered: it does not open.
        let mut unreadable = app.deps.crypto.seal(codex_auth(&ago(1_000)).as_bytes())?;
        if let Some(last) = unreadable.0.last_mut() {
            *last ^= 0xff;
        }
        assert!(app.deps.crypto.open(&unreadable).is_err());
        assert!(
            app.deps
                .store
                .refresh_account_credential(&account, &previous, &unreadable, app.now())
                .await?
        );
        let _ = go_tx.send(());
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        assert_eq!(
            std::fs::read_to_string(home.join("auth.json"))?,
            newer,
            "the refresh that could not be stored is not left as it was"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-503 CDX-006 (slice C1 review round 3): a server that stopped between a run's end and the
/// removal of its `auth.json` left the file in the sandbox; the next start's reconciliation keeps
/// a genuine refresh in it and removes it, since no run of the subscription is left there.
#[test]
fn a_start_sweeps_an_auth_json_no_run_uses() -> TestResult {
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, None, FakeAgentOptions::default(), 8)?;
        let (sandbox, _user) = seed_records(&app).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());
        std::fs::create_dir_all(&home)?;
        let refreshed = codex_auth(&ago(60_000));
        std::fs::write(home.join("auth.json"), &refreshed)?;
        app.reconcile().await?;
        assert_eq!(stored_credential(&app, &account).await?, refreshed);
        assert!(
            !home.join("auth.json").exists(),
            "auth.json left after the start"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Runs a Codex task to its end: started, `refreshed` written into its Codex home by "Codex"
/// when given, then `/exit`.
async fn run_codex_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    home: &Path,
    seen: &str,
    refreshed: Option<&str>,
) -> TestResult {
    let id = new_codex_task(app, sandbox, user, title).await?;
    wait_for("the banner", Duration::from_secs(30), || {
        screen_contains(app, &id, "FAKE-AGENT READY")
    })
    .await?;
    assert_eq!(std::fs::read_to_string(home.join("auth.json"))?, seen);
    if let Some(refreshed) = refreshed {
        std::fs::write(home.join("auth.json"), refreshed)?;
    }
    app.send_input(&id, b"/exit\r").await?;
    wait_for("the run's end", Duration::from_secs(30), || async {
        task(app, &id).await.is_ok_and(|t| !t.status.is_live())
    })
    .await?;
    Ok(())
}

/// SPEC-503 CDX-011, CDX-006: a sign-in imported as an `auth.json` (from `codex login` on a
/// laptop) is what a Codex task's home gets; the tokens Codex refreshes during the run go back
/// into the account; and a later import replaces them for the next run.
#[test]
fn an_imported_codex_sign_in_reaches_the_run_and_its_refresh_is_kept() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let ctx = admin_ctx(&app, &user).await?;
        let imported = codex_auth(&ago(3_600_000));
        let created = app
            .create_account(
                &ctx,
                yulius_contracts::api::admin::CreateAccount {
                    provider: "openai".into(),
                    label: "Codex laptop".into(),
                    token: None,
                    api_key: None,
                    auth_json: Some(imported.clone()),
                },
            )
            .await?;
        assert_eq!(created.login_id, None, "no sign-in runs");
        let account = AccountId::parse(&created.account.id)?;
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex")
            .join(account.as_str());

        let refreshed = codex_auth(&ago(60_000));
        run_codex_task(
            &app,
            &sandbox,
            &user,
            "Codex import",
            &home,
            &imported,
            Some(&refreshed),
        )
        .await?;
        wait_for(
            "the written-back credentials",
            Duration::from_secs(10),
            || async {
                stored_credential(&app, &account)
                    .await
                    .is_ok_and(|stored| stored == refreshed)
            },
        )
        .await?;

        // Signed in again on the laptop and imported: the next run gets the new file.
        let again = codex_auth(&ago(1_000)).replace("SYNTHETIC-refresh", "SYNTHETIC-refresh-2");
        app.import_account_auth(
            &ctx,
            account.as_str(),
            yulius_contracts::api::admin::ImportAccountAuth {
                auth_json: again.clone(),
            },
        )
        .await?;
        assert_eq!(stored_credential(&app, &account).await?, again);
        run_codex_task(&app, &sandbox, &user, "Codex reimport", &home, &again, None).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Marks `account` limited until `resets_at` (100 % of its 5-hour window).
async fn limit_until(
    app: &App,
    account: &AccountId,
    resets_at: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let full = yulius_domain::usage::AccountUsage {
        primary: Some(yulius_domain::usage::UsageWindow {
            used_percent: Some(100.0),
            window_minutes: Some(300),
            resets_at: Some(Timestamp(resets_at)),
        }),
        limit_reached: true,
        limit_resets_at: Some(Timestamp(resets_at)),
        ..yulius_domain::usage::AccountUsage::default()
    };
    app.deps
        .store
        .set_account_state(
            account,
            AccountStatus::Limited,
            Some(&yulius_app::usage_json::to_json(&full)),
            None,
            app.now(),
        )
        .await?;
    Ok(())
}

#[test]
fn new_and_resumed_codex_sessions_choose_the_subscription_with_more_quota() -> TestResult {
    use yulius_domain::usage::{AccountUsage, UsageWindow};

    let (_dir, data) = temp()?;
    runtime()?.block_on(async {
        let (app, sandbox, user, id, session) = restart_fixture(&data).await?;
        support::stop_tasks(&app, &[&id]).await?;
        let original = AccountId::parse("a-codex")?;
        let roomy = add_codex_account(&app, "roomy", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let now = app.now();
        // The nearly spent subscription used to win because it resets much sooner.
        for (account, used_percent, hours) in [(&original, 93.0, 1), (&roomy, 2.0, 168)] {
            let usage = AccountUsage {
                secondary: Some(UsageWindow {
                    used_percent: Some(used_percent),
                    window_minutes: Some(10_080),
                    resets_at: Some(now.plus_millis(hours * 3_600_000)),
                }),
                observed_at: Some(now),
                ..AccountUsage::default()
            };
            app.deps
                .store
                .set_account_state(
                    account,
                    AccountStatus::Ok,
                    Some(&yulius_app::usage_json::to_json(&usage)),
                    None,
                    now,
                )
                .await?;
        }
        let fresh = new_codex_task(&app, &sandbox, &user, "New task").await?;
        wait_for("the new Codex run", Duration::from_secs(30), || async {
            task(&app, &fresh)
                .await
                .is_ok_and(|task| task.status == TaskStatus::Running)
        })
        .await?;
        let fresh_run = app
            .deps
            .store
            .live_attempt(&fresh)
            .await?
            .ok_or("new run")?;
        support::stop_tasks(&app, &[&fresh]).await?;

        let ctx = admin_ctx(&app, &user).await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        let resumed = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("resumed run")?;
        support::stop_tasks(&app, &[&id]).await?;
        assert_eq!(
            resumed.provider_session_id.as_deref(),
            Some(session.as_str())
        );
        assert_eq!(
            [fresh_run.account_id.as_ref(), resumed.account_id.as_ref()],
            [Some(&roomy), Some(&roomy)],
        );
        Ok(())
    })
}

/// SPEC-503 CDX-010: the capacity view (the Dashboard's Subscriptions card) names each
/// subscription's provider, and with nothing available it resumes when the first subscription
/// of any provider does, a Codex one included.
#[test]
fn the_capacity_view_names_providers_and_resumes_with_codex_too() -> TestResult {
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, None, FakeAgentOptions::default(), 8)?;
        let (_sandbox, user) = seed_records(&app).await?;
        let ctx = admin_ctx(&app, &user).await?;
        let codex = add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let soon = app.now().0 + 3_600_000;
        limit_until(&app, &AccountId::parse("a-claude")?, soon + 3_600_000).await?;
        limit_until(&app, &codex, soon).await?;
        let capacity = app.capacity_view(&ctx).await?;
        let provider = |id: &str| {
            capacity
                .accounts
                .iter()
                .find(|a| a.id.as_deref() == Some(id))
                .map(|a| a.provider.clone())
        };
        assert_eq!(provider("a-claude").as_deref(), Some("anthropic"));
        assert_eq!(provider("a-codex").as_deref(), Some("openai"));
        assert_eq!(
            capacity.resumes_at.as_deref(),
            Some(yulius_app::auth::rfc3339(Timestamp(soon)).as_str()),
            "{capacity:?}"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The terminal in #517 was erased by the CLI as it exited. Writing to that parked
/// task continues the rollout that produced its answer.
#[test]
fn parked_codex_keeps_the_answer_and_resumes_the_rollout_with_the_next_prompt() -> TestResult {
    use crate::first_message::{hooked, start_with};
    use crate::pool_supervision::{ScriptedUsage, seed};
    use crate::task_message::superagent;
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            answers_first_message: true,
            resume_prompts: true,
            interrupts: true,
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
        let (sandbox, user) = seed(&app, &data, &["claude-only"]).await?;
        add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let id = new_codex_task(&app, &sandbox, &user, "First turn").await?;
        wait_for(
            "the answered Codex turn",
            Duration::from_secs(30),
            || async {
                screen_contains(&app, &id, "ECHO: First turn").await
                    && task(&app, &id)
                        .await
                        .is_ok_and(|task| task.activity == Some(Activity::Waiting))
            },
        )
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("live run")?;
        let rollout_id = run.provider_session_id.ok_or("rollout id")?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute(
            "UPDATE attempts SET provider_session_id = NULL WHERE id = ?1",
            [run.id.as_str()],
        )?;
        assert_eq!(app.park_idle_sessions_now().await, 0);
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        db.execute(
            "UPDATE attempts SET provider_session_id = ?1 WHERE id = ?2",
            [rollout_id.as_str(), run.id.as_str()],
        )?;
        let mut viewer = app.runtime(&id).hub.watch();
        wait_for("parking", Duration::from_secs(30), || async {
            app.park_idle_sessions_now().await;
            task(&app, &id)
                .await
                .is_ok_and(|task| task.status == TaskStatus::Parked)
        })
        .await?;
        // A browser attached across the park learns the grid before redrawing it.
        let mut saw_state = false;
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), viewer.recv()).await??;
            match frame {
                yulius_app::hub::HubFrame::State => saw_state = true,
                yulius_app::hub::HubFrame::Snapshot { bytes, .. }
                    if String::from_utf8_lossy(&bytes).contains("Yulius parked") =>
                {
                    assert!(
                        saw_state,
                        "parked snapshot arrived before the terminal state"
                    );
                    break;
                }
                _ => {}
            }
        }
        let parked_screen = app.runtime(&id).hub.screen_text();
        let parked_size = app.runtime(&id).hub.size();
        assert!(
            parked_screen.contains("ECHO: First turn"),
            "{parked_screen}"
        );
        assert!(!parked_screen.contains("codex resume"), "{parked_screen}");
        assert_eq!(
            parked_screen.matches("Yulius parked").count(),
            1,
            "{parked_screen}"
        );
        let first = app
            .deps
            .store
            .attempts(&id)
            .await?
            .pop()
            .ok_or("first run")?;
        let session = first.provider_session_id.ok_or("rollout id")?;
        // A restart between the attempt's ID write and the lineage write.
        rusqlite::Connection::open(data.join("yulius.db"))?.execute(
            "DELETE FROM provider_sessions WHERE task_id = ?1",
            [id.as_str()],
        )?;
        // A new server has only the saved screen and the recorded attempt.
        let (app, _) = start_with(
            &data,
            &fake,
            agent.clone(),
            0,
            Arc::new(ScriptedUsage::default()),
            |_| {},
        )
        .await?;
        let ctx = admin_ctx(&app, &user).await?;
        let terminal = app.terminal_open(&ctx, id.as_str(), Some((60, 16))).await?;
        assert_eq!(app.runtime(&id).hub.screen_text(), parked_screen);
        drop(terminal);
        let remote = yulius_app::remote::RemoteCaller {
            ctx,
            token_id: "screen-test".into(),
            scope: yulius_domain::permission::TokenScope::Read,
            login: "admin".into(),
            superagent: None,
        };
        let request = || serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "task_screen",
                "arguments": {"project": "acme/pool", "task_id": id.as_str()},
                "_meta": {"yulius/structured": true}
            }
        }));
        let result = app
            .remote_mcp_handle(&remote, request()?)
            .await
            .and_then(|reply| reply.result)
            .ok_or("task_screen")?;
        let shown = support::tool_text(&result);
        assert_eq!(shown["screen"], parked_screen.trim_end());
        assert_eq!(shown["last_run"]["exit_reason"], "parked");
        // Another restart: the message arrives before any viewer has loaded the capture.
        let (app, _) = start_with(
            &data, &fake, agent, 0, Arc::new(ScriptedUsage::default()), |_| {},
        ).await?;
        let caller = superagent(&sandbox, &user, "admin");
        let prompt = "Second turn: answer with an echo";
        db.execute(
            "UPDATE attempts SET provider_session_id = NULL WHERE id = ?1",
            [first.id.as_str()],
        )?;
        let args = serde_json::json!({"task_id":id.as_str(), "text": prompt});
        let Err(error) = app.tool_task_message(&caller, &args).await else {
            return Err("resumed without a recorded rollout".into());
        };
        assert!(error.to_string().contains("no recorded rollout"), "{error}");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Parked);
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        // A lineage entry from before the parked attempt cannot identify that attempt.
        db.execute(
            "INSERT INTO provider_sessions (id, sandbox_id, task_id, harness, session_id, origin, first_seen, last_seen)
             VALUES ('old-entry', ?1, ?2, 'codex', 'old-session', 'native', 0, 0)",
            [sandbox.as_str(), id.as_str()],
        )?;
        let Err(error) = app.tool_task_message(&caller, &args).await else {
            return Err("resumed the stale rollout".into());
        };
        assert!(error.to_string().contains("no recorded rollout"), "{error}");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Parked);
        db.execute(
            "UPDATE attempts SET provider_session_id = ?1 WHERE id = ?2",
            [session.as_str(), first.id.as_str()],
        )?;
        let (reached, at) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::RunRecorded,
            Path::new(id.as_str()),
            move || {
                let _ = reached.send(());
                let _ = wait.recv_timeout(Duration::from_secs(30));
            },
        );
        let message = {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move {
                app.tool_task_message(
                    &caller,
                    &serde_json::json!({"task_id":id.as_str(), "text": prompt}),
                ).await
            })
        };
        tokio::task::spawn_blocking(move || at.recv_timeout(Duration::from_secs(30))).await??;
        let cold_remote = app.remote_mcp_handle(&remote, request()?).await
            .and_then(|reply| reply.result).ok_or("cold task_screen")?;
        let cold_remote = support::tool_text(&cold_remote);
        let ctx = admin_ctx(&app, &user).await?;
        let terminal = app.terminal_open(&ctx, id.as_str(), Some((22, 5))).await?;
        let saved = app.runtime(&id).hub.screen_text();
        let parked_state = app.terminal_state(&id).await?;
        let _ = go.send(());
        let answer = message.await??;
        assert_eq!(cold_remote["screen"], parked_screen.trim_end());
        assert_eq!(cold_remote["screen_from"], "last run");
        assert_eq!(saved, parked_screen);
        assert_eq!(parked_state.size, parked_size);
        assert!(parked_state.parked_screen);
        drop(terminal);
        assert_eq!(answer["delivered"], "resumed");
        wait_for(
            "the answer in the resumed rollout",
            Duration::from_secs(30),
            || screen_contains(&app, &id, "Second turn: answer with an echo"),
        )
        .await?;
        assert!(!app.terminal_state(&id).await?.parked_screen);
        let second = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("resumed run")?;
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            second.provider_session_id.as_deref(),
            Some(session.as_str())
        );
        let home = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex/a-codex/sessions");
        let rollout = app
            .deps
            .harness
            .find_codex_rollout(&home, &session)?
            .ok_or("rollout")?;
        let history = std::fs::read_to_string(rollout)?;
        assert!(history.contains("First turn") && history.contains(prompt));
        crate::pool_supervision::stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A temporary failure reading lineage in a capacity round keeps the parked
/// continuation for a later round. Starting fresh here would discard its context.
#[test]
fn a_parked_codex_capacity_retry_keeps_its_rollout_and_prompt_on_a_lineage_error() -> TestResult {
    use crate::first_message::{hooked, start_with};
    use crate::pool_supervision::{ScriptedUsage, seed};
    use crate::task_message::superagent;
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            answers_first_message: true,
            resume_prompts: true,
            interrupts: true,
            ..hooked()
        };
        let (app, _) = start_with(
            &data,
            &fake,
            agent,
            0,
            Arc::new(ScriptedUsage::default()),
            |config| config.park_after_ms = 1,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data, &["claude-only"]).await?;
        let account =
            add_codex_account(&app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
        let id = new_codex_task(&app, &sandbox, &user, "Before capacity").await?;
        wait_for("Codex's first answer", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "ECHO: Before capacity").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|row| row.activity == Some(Activity::Waiting))
        })
        .await?;
        let first = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("first attempt")?;
        let session = first.provider_session_id.ok_or("first rollout")?;
        wait_for(
            "parked before the capacity wait",
            Duration::from_secs(30),
            || async {
                app.park_idle_sessions_now().await;
                task(&app, &id)
                    .await
                    .is_ok_and(|row| row.status == TaskStatus::Parked)
            },
        )
        .await?;
        wait_for("the captured screen", Duration::from_secs(10), || {
            screen_contains(&app, &id, "Yulius parked")
        })
        .await?;
        let parked_screen = app.runtime(&id).hub.screen_text();
        // The sole account is removed after selection and before the host spawns.
        let (chosen, chosen_rx) = std::sync::mpsc::channel();
        let (removed, removed_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::AccountChosen,
            Path::new(id.as_str()),
            move || {
                let _ = chosen.send(());
                let _ = removed_rx.recv_timeout(Duration::from_secs(30));
            },
        );
        let prompt = "Continue after the capacity wait";
        let message = {
            let (app, id) = (app.clone(), id.clone());
            let sender = superagent(&sandbox, &user, "admin");
            tokio::spawn(async move {
                app.tool_task_message(
                    &sender,
                    &serde_json::json!({"task_id":id.as_str(),"text":prompt}),
                )
                .await
            })
        };
        tokio::task::spawn_blocking(move || chosen_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        app.delete_account(&admin_ctx(&app, &user).await?, account.as_str())
            .await?;
        removed.send(())?;
        message.await??;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1].exit_reason.as_deref(), Some("account_removed"));
        let caller = superagent(&sandbox, &user, "admin");
        crate::parking::assert_cold_screen(&data, &id, &caller, &parked_screen).await?;
        let pause = app
            .deps
            .store
            .open_pause(&id)
            .await?
            .ok_or("capacity wait")?;
        assert!(
            pause
                .continuation
                .as_deref()
                .is_some_and(|text| text.contains(prompt))
        );
        add_codex_account(&app, "replacement", &codex_auth("2026-11-01T10:00:00Z")).await?;

        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute(
            "UPDATE provider_sessions SET harness = 'unreadable' WHERE task_id = ?1",
            [id.as_str()],
        )?;
        assert!(app.deps.store.provider_sessions(&id).await.is_err());
        app.resume_round().await;
        let held = task(&app, &id).await?;
        let pending = app.deps.store.open_pause(&id).await?;
        db.execute(
            "UPDATE provider_sessions SET harness = 'codex' WHERE task_id = ?1",
            [id.as_str()],
        )?;
        assert_eq!(held.status, TaskStatus::WaitingCapacity);
        assert_eq!(pending.as_ref().map(|p| &p.id), Some(&pause.id));
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 2);

        let (recorded, recorded_rx) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::RunRecorded,
            Path::new(id.as_str()),
            move || {
                let _ = recorded.send(());
                let _ = wait.recv_timeout(Duration::from_secs(30));
            },
        );
        let resumed = {
            let app = app.clone();
            tokio::spawn(async move { app.resume_round().await })
        };
        tokio::task::spawn_blocking(move || recorded_rx.recv_timeout(Duration::from_secs(30)))
            .await??;
        let visible = crate::parking::assert_cold_screen(&data, &id, &caller, &parked_screen).await;
        go.send(())?;
        resumed.await?;
        visible?;
        wait_for(
            "the held next prompt in the original rollout",
            Duration::from_secs(30),
            || screen_contains(&app, &id, prompt),
        )
        .await?;
        let second = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("resumed attempt")?;
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            second.provider_session_id.as_deref(),
            Some(session.as_str())
        );
        crate::pool_supervision::stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
