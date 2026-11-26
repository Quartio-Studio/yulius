//! The subscription pool with real processes: supervisor → detached `yulius ptyhost` → fake
//! agent, a scripted usage probe and real git worktrees (SPEC-001 ROT-004/010, AC-ROT-9..11,
//! AC-ROT-14; SPEC-002 ADR-17). The fake agent has no hooks, so its turn boundary is the end of
//! a turn in its transcript; hook holds are covered by `subscription_pool.rs`. MCP servers reach
//! every run on every subscription (MCP-004, AC-MCP-3).

use crate::support;
use crate::support::eventually;
use crate::support::live::pool::{
    HOUR, ScriptedUsage, add_account, build, build_served_tuned, build_with, build_with_github,
    echoed, has_status, hold_pool_move, new_task, now, reading, refresh, runtime, screen, seed,
    stop, task, temp, usage,
};

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use support::agents::{FakeAgentOptions, limit_resume_problems};
use support::{FakeGitHub, TestResult};
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountStatus, LaunchKind, PauseKind, TaskOrigin, UserRecord};
use yulius_contracts::api::mcp::McpServerInput;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::crypto::MasterKeyCrypto;

/// AC-ROT-9: a new task starts on the subscription whose capacity expires first.
#[test]
fn a_new_task_starts_on_the_capacity_that_expires_first() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-far", "b-soon"]).await?;
        // a-far: 90 % of its week left over 6 days; b-soon: 40 % left, resetting in 8 hours.
        probe.set("a-far", usage(5.0, 2 * HOUR, 8.0, 150));
        probe.set("b-soon", usage(5.0, 2 * HOUR, 58.0, 8));
        refresh(&app, &user, "a-far").await?;
        refresh(&app, &user, "b-soon").await?;
        let id = new_task(&app, &sandbox, &user, "Placement").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-soon"),
            "use it or lose it"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10, AC-ROT-14 (one subscription): its 5-hour window reaches 98 %; at the next turn
/// boundary the task pauses with its process alive, and the agent receives nothing. When the
/// window resets the task continues in place — same run, still nothing typed.
#[test]
fn one_exhausted_subscription_pauses_at_the_boundary_and_resumes_in_place() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Pause").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        probe.set("a-only", usage(98.5, HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Running,
            "never mid-turn"
        );
        app.send_input(&id, b"first turn\r").await?;
        eventually("the pause at the boundary", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Turn);
        let live = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("process kept")?;
        // Nothing reached the agent beyond what the user typed.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(echoed(&app, &id), ["first turn"]);

        // The window resets: the loop releases the task in place.
        probe.set("a-only", usage(0.0, 5 * HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        let _ = app.resume_round().await;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let after = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(after.id, live.id, "the same run continues");
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(echoed(&app, &id), ["first turn"], "still nothing typed");
        app.send_input(&id, b"second turn\r").await?;
        eventually("the next turn", Duration::from_secs(10), || {
            let app = app.clone();
            let id = id.clone();
            async move { echoed(&app, &id).len() == 2 }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn legacy_goal_hold(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    goal_hold_for(app, sandbox, user, Harness::ClaudeCode).await
}

async fn goal_hold_for(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    harness: Harness,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let task = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: "Check delivery".into(),
            harness,
            title: Some("Verification".into()),
            workflow_id: None,
            steps: vec!["Verify dev".into()],
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: false,
            mode: yulius_app::model::TaskMode::Goal,
            supertask: false,
        })
        .await?;
    let id = &task.id;
    app.deps
        .store
        .transition_task(
            id,
            Some(task.status),
            TaskStatus::WaitingCapacity,
            None,
            app.now(),
        )
        .await?;
    app.pause_task(id, PauseKind::Turn, None, None).await?;
    assert!(app.deps.store.live_attempt(id).await?.is_none());
    Ok(task.id)
}

/// A persisted turn hold written before turn continuations were carried. There is no
/// surviving process to poll Stop after capacity returns.
#[test]
fn a_goal_continuation_recovers_a_process_free_capacity_hold() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake, Arc::new(ScriptedUsage::default()))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = &legacy_goal_hold(&app, &sandbox, &user).await?;
        app.resume_round().await;
        let run = app
            .deps
            .store
            .live_attempt(id)
            .await?
            .ok_or("resumed run")?;
        eventually("the resumed launch", Duration::from_secs(30), || async {
            Path::new(&run.run_dir).join("fake-launch.txt").is_file()
        })
        .await?;
        let launch = std::fs::read_to_string(Path::new(&run.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("Continue the authorized goal.") && launch.contains("Verify dev"),
            "{launch}"
        );
        assert_eq!(app.deps.store.task(id).await?.ok_or("task")?.gate_blocks, 1);

        // The next held turn has a live process. Move it to the newly available
        // subscription and charge the instruction delivered to the replacement.
        let first = run.account_id.ok_or("first subscription")?;
        let second = add_account(&app, "b-second").await?;
        let limited = serde_json::json!({"primary":{"used_percent":98.0, "window_minutes":300,
            "resets_at":app.now().plus_millis(HOUR).0}})
        .to_string();
        app.deps
            .store
            .set_account_state(&first, AccountStatus::Ok, Some(&limited), None, app.now())
            .await?;
        app.deps
            .store
            .set_account_state(
                &second,
                AccountStatus::Ok,
                Some(r#"{"primary":{"used_percent":5.0,"window_minutes":300}}"#),
                None,
                app.now(),
            )
            .await?;
        app.pause_task(id, PauseKind::Turn, Some(first), None)
            .await?;
        Box::pin(app.resume_round()).await;
        let moved = app
            .deps
            .store
            .live_attempt(id)
            .await?
            .ok_or("replacement")?;
        assert_eq!(moved.account_id, Some(second));
        let replacement =
            std::fs::read_to_string(Path::new(&moved.run_dir).join("fake-launch.txt"))?;
        assert!(
            replacement.contains("Continue the authorized goal.")
                && replacement.contains("Verify dev"),
            "the replacement must receive the unfinished opening turn: {replacement}"
        );
        assert_eq!(app.deps.store.task(id).await?.ok_or("task")?.gate_blocks, 2);
        stop(&app, id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Admission can queue a held turn after capacity returns. The queued start preserves
/// the verification prompt and resumes the previous session when the task has one.
#[test]
fn a_goal_continuation_queued_after_a_hold_preserves_session_lineage() -> TestResult {
    use yulius_app::model::{ProviderSessionRecord, QueuedStartKind};

    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_one_slot(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            FakeAgentOptions::default(),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let holder = slot_holder(&app, &sandbox, &user).await?;
        for kind in [QueuedStartKind::Fresh, QueuedStartKind::Resume] {
            let id = legacy_goal_hold(&app, &sandbox, &user).await?;
            if kind == QueuedStartKind::Resume {
                app.deps
                    .store
                    .upsert_provider_session(&ProviderSessionRecord {
                        id: "verified-session".into(),
                        sandbox_id: sandbox.clone(),
                        task_id: id.clone(),
                        harness: Harness::ClaudeCode,
                        session_id: "prior-session".into(),
                        transcript_ref: None,
                        origin: "imported".into(),
                        first_seen: app.now(),
                        last_seen: app.now(),
                    })
                    .await?;
            }
            app.resume_round().await;
            let queued = app
                .deps
                .store
                .queued_start(&id)
                .await?
                .ok_or("queued continuation")?;
            assert_eq!(
                queued.kind.as_str(),
                if kind == QueuedStartKind::Fresh {
                    "goal_fresh"
                } else {
                    "goal_resume"
                }
            );
            let prompt = queued.prompt.ok_or("verification prompt")?;
            assert!(
                prompt.contains("Continue the authorized goal.") && prompt.contains("Verify dev")
            );
            stop(&app, &id).await?;
        }
        let id = legacy_goal_hold(&app, &sandbox, &user).await?;
        let owner = "Continue the authorized goal. Check the revised fixture";
        assert!(
            app.deps
                .store
                .update_open_pause(&id, PauseKind::Prompt, Some(owner))
                .await?
        );
        app.resume_round().await;
        let queued = app
            .deps
            .store
            .queued_start(&id)
            .await?
            .ok_or("queued owner prompt")?;
        assert_eq!(queued.kind, QueuedStartKind::Fresh);
        assert_eq!(queued.prompt.as_deref(), Some(owner));
        stop(&app, &id).await?;
        stop(&app, &holder).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_manual_move_delivers_saved_fresh_and_resumed_goals_across_harnesses() -> TestResult {
    use yulius_app::model::ProviderSessionRecord;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async {
        let app = build(&data, &fake, Arc::new(ScriptedUsage::default()))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let google = crate::support::antigravity::add_google_account(
            &app,
            "a-google",
            &crate::support::antigravity::api_key()?,
        )
        .await?;
        let store = &app.deps.store;
        let ctx = AuthContext {
            user: store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        for (harness, account) in [
            (Harness::ClaudeCode, AccountId::parse("a-first")?),
            (Harness::Antigravity, google),
        ] {
            for (pause_kind, launch_kind) in [
                (PauseKind::GoalFreshStart, LaunchKind::New),
                (PauseKind::GoalResumeStart, LaunchKind::Resume),
            ] {
                let id = goal_hold_for(&app, &sandbox, &user, harness).await?;
                let prompt = "Continue the authorized goal. Verify the accepted revision";
                assert!(
                    store
                        .update_open_pause(&id, pause_kind, Some(prompt))
                        .await?
                );
                store
                    .upsert_provider_session(&ProviderSessionRecord {
                        id: format!("session-{id}"),
                        sandbox_id: sandbox.clone(),
                        task_id: id.clone(),
                        harness,
                        session_id: format!("prior-{id}"),
                        transcript_ref: None,
                        origin: "imported".into(),
                        first_seen: app.now(),
                        last_seen: app.now(),
                    })
                    .await?;
                app.switch_task(
                    &ctx,
                    id.as_str(),
                    &yulius_contracts::api::tasks::ContinueTask {
                        account_id: Some(account.to_string()),
                        ..Default::default()
                    },
                )
                .await?;
                let run = store.live_attempt(&id).await?.ok_or("replacement run")?;
                assert_eq!(run.launch_kind, launch_kind, "{harness:?}");
                let launch = Path::new(&run.run_dir).join("fake-launch.txt");
                eventually(
                    "the moved goal instruction",
                    Duration::from_secs(30),
                    || async {
                        std::fs::read_to_string(&launch).is_ok_and(|text| text.contains(prompt))
                    },
                )
                .await?;
                assert_eq!(task(&app, &id).await?.gate_blocks, 1);
                stop(&app, &id).await?;
            }
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A native goal Stop authenticated before a subscription change arrives after the
/// replacement has submitted its opening turn. Its verdict deadline belongs to the old run.
#[test]
fn a_late_native_goal_stop_cannot_settle_a_replacement_turn() -> TestResult {
    check_replaced_goal_stop(yulius_app::race::Point::StopClean)
}

#[test]
fn a_claimed_native_goal_verdict_cannot_settle_a_replacement_turn() -> TestResult {
    check_replaced_goal_stop(yulius_app::race::Point::GoalVerdictReady)
}

fn check_replaced_goal_stop(point: yulius_app::race::Point) -> TestResult {
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            FakeAgentOptions::default(),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = start_verification_goal(&app, sandbox, user).await?;
        let limit = Duration::from_secs(30);
        eventually("the first goal run", limit, || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("first run")?;
        bind_goal_hook(&app, &data, &first.id, "test-old-goal-stop")?;
        let (arrived, parked) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        yulius_app::race::on(point, Path::new(id.as_str()), move || {
            let _ = arrived.send(());
            let _ = tokio::task::block_in_place(|| blocked.recv_timeout(limit));
        });
        let old_stop = tokio::spawn({
            let app = app.clone();
            async move {
                app.hook_event(
                    "test-old-goal-stop",
                    "claude",
                    "Stop",
                    &Default::default(),
                    None,
                )
                .await
            }
        });
        tokio::time::timeout(limit, parked).await??;
        let prompt = "Continue the authorized goal. Verify dev";
        app.switch_with(
            &id,
            Some(AccountId::parse("b-second")?),
            "Capacity",
            yulius_app::supervisor::RunStart::GoalResume {
                prompt: prompt.into(),
            },
        )
        .await?;
        let replacement = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("replacement run")?;
        assert_ne!(first.id, replacement.id);
        bind_goal_hook(&app, &data, &replacement.id, "test-replacement-submit")?;
        app.hook_event(
            "test-replacement-submit",
            "claude",
            "UserPromptSubmit",
            &yulius_contracts::hooks::HookInput {
                prompt: Some(prompt.into()),
                ..Default::default()
            },
            None,
        )
        .await?;
        assert_eq!(task(&app, &id).await?.activity, Some(Activity::Working));
        release.send(())?;
        old_stop.await??;
        let ended = eventually(
            "an old Stop settling the replacement",
            yulius_app::finish::GOAL_VERDICT_WAIT + Duration::from_secs(2),
            || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|task| task.activity != Some(Activity::Working))
            },
        )
        .await;
        let current = task(&app, &id).await?;
        assert!(
            ended.is_err(),
            "the old Stop changed the replacement to {:?}",
            current.activity
        );
        assert_eq!(current.activity, Some(Activity::Working));
        assert_eq!(current.gate_blocks, 1);
        stop(&app, &id).await
    }))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn start_verification_goal(
    app: &Arc<App>,
    sandbox: SandboxId,
    user: UserId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    Ok(app
        .create_task_internal(NewTask {
            sandbox,
            created_by: user,
            prompt: "Check delivery".into(),
            harness: Harness::ClaudeCode,
            title: Some("Verify checkout".into()),
            workflow_id: None,
            steps: vec!["Verify dev".into()],
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: yulius_app::model::TaskMode::Goal,
            supertask: false,
        })
        .await?
        .id)
}

fn bind_goal_hook(
    app: &App,
    data: &Path,
    attempt: &yulius_domain::ids::AttemptId,
    token: &str,
) -> TestResult {
    rusqlite::Connection::open(data.join("yulius.db"))?.execute(
        "UPDATE attempts SET token_hash = ?1 WHERE id = ?2",
        rusqlite::params![
            app.deps.crypto.sha256_hex(token.as_bytes()),
            attempt.as_str()
        ],
    )?;
    Ok(())
}

async fn write_native_goal(
    app: &Arc<App>,
    data: &Path,
    id: &TaskId,
    attempt: &yulius_app::model::AttemptRecord,
    met: bool,
) -> TestResult {
    use std::io::Write;
    let task = task(app, id).await?;
    let path = app.deps.harness.claude_transcript(
        &data
            .join("sandboxes")
            .join(task.sandbox_id.as_str())
            .join("home"),
        Path::new(&task.worktree_path.ok_or("worktree")?),
        attempt.provider_session_id.as_deref().ok_or("session")?,
    );
    std::fs::create_dir_all(path.parent().ok_or("transcript directory")?)?;
    let mut transcript = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    writeln!(
        transcript,
        "{}",
        serde_json::json!({"type":"attachment","attachment":{"type":"goal_status","met":met,"sentinel":!met,"condition":"Verify dev"}})
    )?;
    Ok(())
}

#[test]
fn a_delayed_native_met_cannot_complete_a_replacement_goal() -> TestResult {
    check_delayed_native_met(true)
}

#[test]
fn a_native_met_after_its_verdict_timeout_completes_the_goal() -> TestResult {
    check_delayed_native_met(false)
}

fn check_delayed_native_met(replace: bool) -> TestResult {
    use yulius_app::race::{self, Point};
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async {
        let app = build_with(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            FakeAgentOptions::default(),
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = start_verification_goal(&app, sandbox, user).await?;
        let limit = Duration::from_secs(35);
        eventually("first run", limit, || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("first run")?;
        bind_goal_hook(&app, &data, &first.id, "test-first-stop")?;
        app.hook_event(
            "test-first-stop",
            "claude",
            "Stop",
            &Default::default(),
            None,
        )
        .await?;
        let (arrived, parked) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let (applied, finished) = tokio::sync::oneshot::channel();
        race::on(
            Point::NativeVerdictRead,
            Path::new(first.id.as_str()),
            move || {
                let _ = arrived.send(());
                let _ = tokio::task::block_in_place(|| blocked.recv_timeout(limit));
            },
        );
        race::on(
            Point::NativeVerdictChecked,
            Path::new(first.id.as_str()),
            move || {
                let _ = applied.send(());
            },
        );
        write_native_goal(&app, &data, &id, &first, true).await?;
        tokio::time::timeout(Duration::from_secs(12), parked).await??;
        if replace {
            let prompt = "Continue the authorized goal. Verify dev";
            app.switch_with(
                &id,
                Some(AccountId::parse("b-second")?),
                "Capacity",
                yulius_app::supervisor::RunStart::GoalFresh {
                    prompt: prompt.into(),
                },
            )
            .await?;
            let second = app
                .deps
                .store
                .live_attempt(&id)
                .await?
                .ok_or("replacement run")?;
            assert_ne!(first.id, second.id);
            assert_ne!(first.provider_session_id, second.provider_session_id);
            bind_goal_hook(&app, &data, &second.id, "test-replacement-stop")?;
            app.hook_event(
                "test-replacement-stop",
                "claude",
                "UserPromptSubmit",
                &yulius_contracts::hooks::HookInput {
                    prompt: Some(prompt.into()),
                    ..Default::default()
                },
                None,
            )
            .await?;
            app.hook_event(
                "test-replacement-stop",
                "claude",
                "Stop",
                &Default::default(),
                None,
            )
            .await?;
            write_native_goal(&app, &data, &id, &second, false).await?;
            eventually("replacement's own open goal", limit, || async {
                app.goal_is_open(&id)
            })
            .await?;
        } else {
            eventually("timeout settlement", limit, || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|task| task.activity == Some(Activity::Waiting))
            })
            .await?;
        }
        release.send(())?;
        tokio::time::timeout(limit, finished).await??;
        if replace {
            app.hook_event(
                "test-replacement-stop",
                "claude",
                "Stop",
                &Default::default(),
                None,
            )
            .await?;
            assert_eq!(
                task(&app, &id).await?.activity,
                Some(Activity::Working),
                "the replacement is still evaluating its own goal"
            );
        } else {
            assert_eq!(
                task(&app, &id).await?.activity,
                Some(Activity::Done),
                "the live goal's Met must settle it after the timeout"
            );
        }
        stop(&app, &id).await
    }))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A gate prompt carried to another process spends one of the goal's bounded attempts.
#[test]
fn a_goal_continuation_delivered_by_a_move_counts_once() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake, Arc::new(ScriptedUsage::default()))?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = app.create_task_internal(NewTask {
            sandbox, created_by: user, prompt: "Verify dev".into(), harness: Harness::ClaudeCode,
            title: Some("Verification".into()), workflow_id: None, steps: vec!["Build checkout".into(), "Verify dev".into()],
            model: None, effort: None, origin: TaskOrigin::Ui, automation_run: None, start: true,
            mode: yulius_app::model::TaskMode::Goal, supertask: false,
        }).await?.id;
        eventually("the run", Duration::from_secs(30), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        }).await?;
        app.deps.store.update_task(&id, &yulius_app::ports::store::TaskFieldUpdate {
            gate_blocks: Some(2),
            ..Default::default()
        }, app.now()).await?;
        let prompt = "Continue the authorized goal. The next unfinished step is \"Verify dev\". Record the evidence and update the step when its work is complete.";
        let step = app.deps.store.steps(&id).await?.into_iter().next().ok_or("step")?;
        let (recorded, receiving) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        yulius_app::race::on(yulius_app::race::Point::RunRecorded, Path::new(id.as_str()), move || {
            let _ = recorded.send(());
            let _ = tokio::task::block_in_place(|| blocked.recv_timeout(Duration::from_secs(30)));
        });
        let progress = async {
            receiving.await?;
            app.deps.store.update_step(&step.id, "done", Some("Verified"), "worker", app.now()).await?;
            assert_eq!(task(&app, &id).await?.gate_blocks, 0);
            release.send(())?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let account = AccountId::parse("b-second")?;
        let (moved, progressed) = tokio::join!(tokio::spawn({
            let app = app.clone();
            let id = id.clone();
            async move { app.switch_with(&id, Some(account), "Capacity",
                yulius_app::supervisor::RunStart::GoalResume { prompt: prompt.to_owned() }).await }
        }), progress);
        moved??;
        progressed?;
        let second = app.deps.store.live_attempt(&id).await?.ok_or("resumed run")?;
        eventually("the resumed launch", Duration::from_secs(30), || async {
            std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))
                .is_ok_and(|launch| launch.contains("Verify dev"))
        }).await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 1, "the attempt follows the completed step");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn check_held_owner_prompt(
    app: &Arc<App>,
    id: &TaskId,
    ctx: &AuthContext,
    token: &str,
) -> TestResult {
    // An accepted viewer instruction held for capacity belongs to the new work even
    // when it is delivered by a replacement process with an empty prompt box.
    let store = &app.deps.store;
    store
        .update_task(
            id,
            &yulius_app::ports::store::TaskFieldUpdate {
                gate_blocks: Some(3),
                ..Default::default()
            },
            app.now(),
        )
        .await?;
    let limited = serde_json::json!({"primary": {"used_percent": 98.0,
            "window_minutes": 300, "resets_at": app.now().plus_millis(HOUR).0}})
    .to_string();
    for account in ["a-first", "b-second"] {
        store
            .set_account_state(
                &AccountId::parse(account)?,
                AccountStatus::Ok,
                Some(&limited),
                None,
                app.now(),
            )
            .await?;
    }
    let fresh = "Continue the authorized goal. Verify the corrected revision";
    let terminal = app.terminal_open(ctx, id.as_str(), None).await?;
    app.terminal_input(ctx, &terminal, format!("{fresh}\r").as_bytes(), false)
        .await?;
    assert!(matches!(
        app.hook_event(
            token,
            "claude",
            "UserPromptSubmit",
            &yulius_contracts::hooks::HookInput {
                prompt: Some(fresh.into()),
                invocation: Some(19),
                ..Default::default()
            },
            None
        )
        .await?,
        yulius_app::hooks::HookReply::Hold { .. }
    ));
    store
        .set_account_state(
            &AccountId::parse("a-first")?,
            AccountStatus::Ok,
            Some(r#"{"primary":{"used_percent":5.0,"window_minutes":300}}"#),
            None,
            app.now(),
        )
        .await?;
    Box::pin(app.resume_round()).await;
    eventually(
        "the new owner instruction on the returned subscription",
        Duration::from_secs(30),
        || async {
            store
                .live_attempt(id)
                .await
                .ok()
                .flatten()
                .is_some_and(|run| {
                    run.account_id
                        .as_ref()
                        .is_some_and(|account| account.as_str() == "a-first")
                        && std::fs::read_to_string(Path::new(&run.run_dir).join("fake-launch.txt"))
                            .is_ok_and(|launch| launch.contains(fresh))
                })
        },
    )
    .await?;
    assert_eq!(
        task(app, id).await?.gate_blocks,
        0,
        "the held owner instruction"
    );
    Ok(())
}

/// Moving an already-working goal through the owner's Switch action carries its turn.
#[test]
fn a_goal_continuation_budget_survives_a_public_subscription_switch() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(Box::pin(async {
        let app = build(&data, &fake, Arc::new(ScriptedUsage::default()))?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = legacy_goal_hold(&app, &sandbox, &user).await?;
        Box::pin(app.resume_round()).await;
        eventually("the run", Duration::from_secs(30), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        app.deps
            .store
            .set_activity(&id, Some(yulius_domain::task::Activity::Working), app.now())
            .await?;
        app.deps
            .store
            .update_task(
                &id,
                &yulius_app::ports::store::TaskFieldUpdate {
                    gate_blocks: Some(2),
                    ..Default::default()
                },
                app.now(),
            )
            .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        Box::pin(app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        ))
        .await?;
        let moved = app.deps.store.live_attempt(&id).await?.ok_or("moved run")?;
        let prompt = "Continue the task exactly where you left off.";
        let launch = std::fs::read_to_string(Path::new(&moved.run_dir).join("fake-launch.txt"))?;
        assert!(launch.contains(prompt), "{launch}");
        // Only this synthetic attempt's hook credential, as in the parking fixtures.
        let token = "test-moved-goal-submit";
        rusqlite::Connection::open(data.join("yulius.db"))?.execute(
            "UPDATE attempts SET token_hash = ?1 WHERE id = ?2",
            rusqlite::params![
                app.deps.crypto.sha256_hex(token.as_bytes()),
                moved.id.as_str()
            ],
        )?;
        let submit = |prompt: &str| yulius_contracts::hooks::HookInput {
            prompt: Some(prompt.into()),
            ..Default::default()
        };
        app.hook_event(token, "claude", "UserPromptSubmit", &submit(prompt), None)
            .await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 2);
        // The resumed session finishes its opening turn before Yulius types its native goal.
        app.hook_event(token, "claude", "Stop", &Default::default(), None)
            .await?;
        app.hook_event(token, "claude", "SessionStart", &Default::default(), None)
            .await?;
        let transcript = app.deps.harness.claude_transcript(
            &data.join("sandboxes").join(sandbox.as_str()).join("home"),
            Path::new(&task(&app, &id).await?.worktree_path.ok_or("worktree")?),
            moved.provider_session_id.as_deref().ok_or("session")?,
        );
        let restored = || {
            std::fs::read_to_string(&transcript)
                .ok()?
                .lines()
                .rev()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find_map(|record| {
                    record["attachment"]["condition"]
                        .as_str()
                        .map(|condition| format!("/goal {condition}"))
                })
        };
        eventually(
            "the restored native goal",
            Duration::from_secs(30),
            || async { screen(&app, &id).contains("GOAL SET") && restored().is_some() },
        )
        .await?;
        let command = restored().ok_or("goal command")?;
        app.hook_event(token, "claude", "UserPromptSubmit", &submit(&command), None)
            .await?;
        assert_eq!(
            task(&app, &id).await?.gate_blocks,
            2,
            "automatic native goal restoration"
        );
        let terminal = app.terminal_open(&ctx, id.as_str(), None).await?;
        Box::pin(app.terminal_input(&ctx, &terminal, prompt.as_bytes(), false)).await?;
        app.hook_event(token, "claude", "UserPromptSubmit", &submit(&command), None)
            .await?;
        assert!(app.has_draft(&id));
        assert_eq!(task(&app, &id).await?.gate_blocks, 2);
        Box::pin(app.terminal_input(&ctx, &terminal, b"\r", false)).await?;
        app.hook_event(token, "claude", "UserPromptSubmit", &submit(prompt), None)
            .await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 0);
        Box::pin(check_held_owner_prompt(&app, &id, &ctx, token)).await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_goal_continuation_budget_resets_when_an_owners_message_is_delivered() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake, Arc::new(ScriptedUsage::default()))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = legacy_goal_hold(&app, &sandbox, &user).await?;
        app.resume_round().await;
        eventually("the run", Duration::from_secs(30), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let token = "test-owner-message";
        rusqlite::Connection::open(data.join("yulius.db"))?.execute(
            "UPDATE attempts SET token_hash = ?1 WHERE id = ?2",
            rusqlite::params![
                app.deps.crypto.sha256_hex(token.as_bytes()),
                attempt.id.as_str()
            ],
        )?;
        app.hook_event(token, "claude", "SessionStart", &Default::default(), None)
            .await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 1);
        let caller = crate::support::task_message::superagent(&sandbox, &user, "admin");
        let sent = app
            .tool_task_message(
                &caller,
                &serde_json::json!({
                    "task_id": id.as_str(), "text": "Verify the corrected revision"
                }),
            )
            .await?;
        assert_eq!(sent["delivered"], "sent");
        assert_eq!(task(&app, &id).await?.gate_blocks, 0);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-11: every subscription exhausted → new tasks wait before starting (no process); a
/// subscription added later resumes them oldest first, one stagger apart, on it.
#[test]
fn a_new_subscription_resumes_waiting_tasks_oldest_first_and_staggered() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-full"]).await?;
        probe.set("a-full", usage(40.0, 3 * HOUR, 99.0, 50));
        refresh(&app, &user, "a-full").await?;
        let first = new_task(&app, &sandbox, &user, "First").await?;
        eventually("the first pause", Duration::from_secs(30), || {
            has_status(&app, &first, TaskStatus::WaitingCapacity)
        })
        .await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        eventually("the second pause", Duration::from_secs(30), || {
            has_status(&app, &second, TaskStatus::WaitingCapacity)
        })
        .await?;
        for id in [&first, &second] {
            assert!(app.deps.store.attempts(id).await?.is_empty(), "no process");
            let pause = app.deps.store.open_pause(id).await?.ok_or("pause")?;
            assert_eq!(pause.kind, PauseKind::Start);
        }
        let waiting = app.task_summary(&task(&app, &first).await?).await?;
        assert!(waiting.resumes_at.is_some(), "the weekly reset is known");

        // A subscription arrives.
        add_account(&app, "b-new").await?;
        probe.set("b-new", usage(0.0, 5 * HOUR, 5.0, 160));
        refresh(&app, &user, "b-new").await?;
        let _ = app.resume_round().await;
        for id in [&first, &second] {
            eventually("the resumed run", Duration::from_secs(30), || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("FAKE-AGENT READY") }
            })
            .await?;
        }
        let one = app.deps.store.attempts(&first).await?;
        let two = app.deps.store.attempts(&second).await?;
        assert_eq!(
            one[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-new")
        );
        assert_eq!(
            two[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-new")
        );
        let gap = two[0].started_at.millis_since(one[0].started_at);
        assert!(gap >= 1_400, "staggered, oldest first: {gap} ms");
        for id in [&first, &second] {
            stop(&app, id).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-11: a task paused on an exhausted subscription resumes on another one whose window
/// reset: a native resume of the same session there, with no prompt.
#[test]
fn a_paused_task_resumes_on_the_subscription_with_capacity_by_native_resume() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Move").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(
            first.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        probe.set("a-first", usage(98.0, 4 * HOUR, 30.0, 100));
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        app.send_input(&id, b"work\r").await?;
        eventually("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;

        // b-second's window resets; a-first is still at 98 %.
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let second = &attempts[1];
        assert_eq!(
            second.account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            second.provider_session_id, first.provider_session_id,
            "same session"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(launch.contains("prompt: None"), "no prompt: {launch}");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

fn mcp_input(name: &str, transport: &str) -> McpServerInput {
    McpServerInput {
        name: name.into(),
        transport: transport.into(),
        command: (transport == "stdio").then(|| "npx".into()),
        args: vec!["-y".into(), format!("@example/{name}-mcp")],
        url: (transport == "http").then(|| format!("https://mcp.{name}.test/mcp")),
        env: None,
        headers: None,
        enabled: None,
        scope: None,
        audience: None,
        sandbox: None,
    }
}

/// The run's own MCP configuration (`--mcp-config`) and the names of its secret variables.
fn run_mcp(
    run_dir: &str,
) -> Result<(serde_json::Value, String, Vec<String>), Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(Path::new(run_dir).join("mcp.json"))?;
    let launch: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        Path::new(run_dir).join("launch.json"),
    )?)?;
    let secrets = launch["secret_env_names"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((serde_json::from_str(&text)?, text, secrets))
}

/// AC-MCP-3 (MCP-004): MCP servers configured in Yulius reach every run, whichever subscription
/// it runs on. Each run gets its own `mcp.json` (`--mcp-config`, strict) with the configured
/// servers and their secrets only as `${VAR}` references, and the values in the run's
/// environment. After the task moves to another subscription by native resume, the new run has
/// the same servers, plus one added in between.
#[test]
fn mcp_servers_reach_every_run_on_every_subscription() -> TestResult {
    const QUARTIO_TOKEN: &str = "synthetic-quartio-token";
    const APIFY_TOKEN: &str = "synthetic-apify-token";
    const LINEAR_KEY: &str = "synthetic-linear-key-1";
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec![
                "YULIUS_MCP_QUARTIO_TOKEN".into(),
                "YULIUS_MCP_APIFY_APIFY_TOKEN".into(),
                "YULIUS_MCP_LINEAR_LINEAR_KEY".into(),
            ],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let mut quartio = mcp_input("quartio", "http");
        quartio.scope = Some("instance".into());
        quartio.headers = Some(BTreeMap::from([(
            "Authorization".into(),
            format!("Bearer {QUARTIO_TOKEN}"),
        )]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &quartio)
            .await?;
        let mut apify = mcp_input("apify", "stdio");
        apify.env = Some(BTreeMap::from([("APIFY_TOKEN".into(), APIFY_TOKEN.into())]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &apify)
            .await?;

        let id = new_task(&app, &sandbox, &user, "Tools").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV YULIUS_MCP_LINEAR_LINEAR_KEY=") }
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(
            first.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        let (mcp, text, secrets) = run_mcp(&first.run_dir)?;
        let servers = mcp["mcpServers"].as_object().ok_or("mcpServers")?;
        let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["apify", "quartio", "yulius"]);
        assert_eq!(servers["quartio"]["url"], "https://mcp.quartio.test/mcp");
        assert_eq!(
            servers["quartio"]["headers"]["Authorization"],
            "Bearer ${YULIUS_MCP_QUARTIO_TOKEN}"
        );
        assert_eq!(servers["apify"]["command"], "npx");
        assert_eq!(
            servers["apify"]["env"]["APIFY_TOKEN"],
            "${YULIUS_MCP_APIFY_APIFY_TOKEN}"
        );
        for secret in [QUARTIO_TOKEN, APIFY_TOKEN] {
            assert!(!text.contains(secret), "the MCP file holds no secret value");
        }
        for name in ["YULIUS_MCP_QUARTIO_TOKEN", "YULIUS_MCP_APIFY_APIFY_TOKEN"] {
            assert!(secrets.iter().any(|n| n == name), "{name} in {secrets:?}");
        }
        let shown = screen(&app, &id);
        assert!(
            shown.contains(&format!(
                "ENV YULIUS_MCP_QUARTIO_TOKEN=set len={}",
                QUARTIO_TOKEN.len()
            )),
            "{shown}"
        );
        assert!(
            shown.contains(&format!(
                "ENV YULIUS_MCP_APIFY_APIFY_TOKEN=set len={}",
                APIFY_TOKEN.len()
            )),
            "{shown}"
        );
        assert!(
            shown.contains("ENV YULIUS_MCP_LINEAR_LINEAR_KEY=unset"),
            "{shown}"
        );

        // A server added while the task runs; then the task moves to b-second.
        let mut linear = mcp_input("linear", "stdio");
        linear.env = Some(BTreeMap::from([("LINEAR_KEY".into(), LINEAR_KEY.into())]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &linear)
            .await?;
        probe.set("a-first", usage(98.0, 4 * HOUR, 30.0, 100));
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        app.send_input(&id, b"work\r").await?;
        eventually("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let second = app.deps.store.attempts(&id).await?.remove(1);
        assert_eq!(
            second.account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        let (mcp, text, secrets) = run_mcp(&second.run_dir)?;
        let servers = mcp["mcpServers"].as_object().ok_or("mcpServers")?;
        let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["apify", "linear", "quartio", "yulius"]);
        assert_eq!(
            servers["linear"]["env"]["LINEAR_KEY"],
            "${YULIUS_MCP_LINEAR_LINEAR_KEY}"
        );
        assert_eq!(
            servers["quartio"]["headers"]["Authorization"],
            "Bearer ${YULIUS_MCP_QUARTIO_TOKEN}"
        );
        assert!(!text.contains(LINEAR_KEY) && !text.contains(APIFY_TOKEN));
        for name in [
            "YULIUS_MCP_QUARTIO_TOKEN",
            "YULIUS_MCP_APIFY_APIFY_TOKEN",
            "YULIUS_MCP_LINEAR_LINEAR_KEY",
        ] {
            assert!(secrets.iter().any(|n| n == name), "{name} in {secrets:?}");
        }
        eventually("the new run's environment", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move {
                screen(&app, &id).contains(&format!(
                    "ENV YULIUS_MCP_LINEAR_LINEAR_KEY=set len={}",
                    LINEAR_KEY.len()
                ))
            }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The server names in a run's own MCP configuration.
fn run_servers(run_dir: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (mcp, _, _) = run_mcp(run_dir)?;
    let mut names: Vec<String> = mcp["mcpServers"]
        .as_object()
        .ok_or("mcpServers")?
        .keys()
        .cloned()
        .collect();
    names.sort_unstable();
    Ok(names)
}

/// Bob: a second person, who may write the sandbox's repository (the stand-in for GitHub of the
/// test says so) and has a GitHub token.
async fn second_person(
    app: &App,
    admin: &UserId,
) -> Result<UserRecord, Box<dyn std::error::Error>> {
    let bob = UserRecord {
        id: UserId::parse("u-bob")?,
        github_login: Some("bob".into()),
        display_name: "Bob".into(),
        is_superadmin: false,
        is_instance_admin: false,
        ..app.deps.store.user(admin).await?.ok_or("admin")?
    };
    app.deps.store.insert_user(&bob).await?;
    app.deps
        .store
        .save_github_token(
            &yulius_app::model::GitHubTokenRecord {
                user_id: bob.id.clone(),
                access: app.deps.crypto.seal(b"token-bob")?,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            app.now(),
        )
        .await?;
    Ok(bob)
}

fn ctx_of(user: UserRecord) -> AuthContext {
    AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
        token: None,
    }
}

/// The administrator's personal MCP server `fleet` (its input and id) and the sandbox's `docs`.
async fn personal_and_shared_servers(
    app: &Arc<App>,
    admin: &AuthContext,
    sandbox: &SandboxId,
) -> Result<(McpServerInput, String), Box<dyn std::error::Error>> {
    let mut fleet = mcp_input("fleet", "http");
    fleet.scope = Some("instance".into());
    fleet.audience = Some("personal".into());
    let fleet_id = app
        .create_mcp_server(admin, sandbox.as_str(), &fleet)
        .await?
        .id;
    app.create_mcp_server(admin, sandbox.as_str(), &mcp_input("docs", "http"))
        .await?;
    Ok((fleet, fleet_id))
}

/// SPEC-483 RST-001: Yulius restarts while a run holds its owner's personal MCP server. The new
/// server still knows it (the run directory says whose servers the run was started with): only
/// the owner messages the session, and a hand-over restarts it with the new owner's servers,
/// as before the restart (SPEC-332 MSC-009).
#[test]
fn a_hand_over_after_a_restart_still_restarts_a_run_with_personal_servers() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = || FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let build = || {
        let github = Arc::new(FakeGitHub::default());
        github.set_perm("bob", "acme/pool", support::perms("write"));
        build_with_github(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(),
            github,
        )
    };
    let first = runtime()?;
    let (sandbox, admin, bob, id, run) = first.block_on(async {
        let app = build()?;
        let (sandbox, admin) = seed(&app, &data, &["a-first"]).await?;
        let bob = second_person(&app, &admin).await?;
        let admin_ctx = ctx_of(app.deps.store.user(&admin).await?.ok_or("admin")?);
        let (fleet, fleet_id) = personal_and_shared_servers(&app, &admin_ctx, &sandbox).await?;
        let id = new_task(&app, &sandbox, &admin, "Mine").await?;
        eventually("the run", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(run_servers(&run.run_dir)?, ["docs", "fleet", "yulius"]);
        // Turned off meanwhile, the server is only in the live run: after the restart nothing
        // but the run's own record says that the run holds it.
        let mut off = fleet;
        off.enabled = Some(false);
        app.update_mcp_server(&admin_ctx, &fleet_id, &off).await?;
        Ok::<_, Box<dyn std::error::Error>>((sandbox, admin, bob, id, run))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build()?;
        app.reconcile().await?;
        assert_eq!(
            app.deps.store.live_attempt(&id).await?.map(|a| a.id),
            Some(run.id.clone()),
            "the same run, reattached"
        );
        // Bob does not message the administrator's session (SPEC-490 OWN-004), which holds
        // the administrator's server.
        let caller = yulius_app::coordination::ProjectCaller {
            sandbox: sandbox.clone(),
            user: bob.id.clone(),
            task: None,
            channel: yulius_app::coordination::Channel::Superagent,
            actor: "cockpit:bob".into(),
            detail: serde_json::json!({"via": "cockpit", "shell": "sh-test"}),
        };
        let refused = app
            .tool_task_message(&caller, &serde_json::json!({"task_id": id.as_str(), "text": "hi"}))
            .await;
        assert!(
            matches!(&refused, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("only they steer it")),
            "{refused:?}"
        );
        // Control passes to Bob: the run is resumed without the administrator's server.
        let admin_ctx = ctx_of(app.deps.store.user(&admin).await?.ok_or("admin")?);
        app.request_task_control(&ctx_of(bob.clone()), id.as_str())
            .await?;
        app.accept_task_control(&admin_ctx, id.as_str()).await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let resumed = app.deps.store.attempts(&id).await?.remove(1);
        assert_eq!(run_servers(&resumed.run_dir)?, ["docs", "yulius"]);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 RST-001 (review): the old server stopped in the middle of a hand-over's restart of
/// the run, so the reattached run holds the servers of someone who is no longer the task's
/// holder (here the run's record names Bob while the task is the administrator's). The new
/// server restarts the run at once.
#[test]
fn a_reattached_run_with_another_persons_servers_is_restarted() -> TestResult {
    reattach_for_new_owner(false)
}

/// Only a refusal that interrupted the session lets hand-over replace a revoked subscription.
/// A normal hand-over keeps its existing preflight refusal, even with another account.
#[test]
fn a_reattached_handover_without_a_reported_limit_keeps_its_preflight_refusal() -> TestResult {
    reattach_for_new_owner(true)
}

fn reattach_for_new_owner(revoked: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let agent = || FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let build = || {
        build_with_github(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent(),
            Arc::new(FakeGitHub::default()),
        )
    };
    let first = runtime()?;
    let id = first.block_on(async {
        let app = build()?;
        let (sandbox, admin) = seed(&app, &data, &["a-first"]).await?;
        second_person(&app, &admin).await?;
        let admin_ctx = ctx_of(app.deps.store.user(&admin).await?.ok_or("admin")?);
        personal_and_shared_servers(&app, &admin_ctx, &sandbox).await?;
        let id = new_task(&app, &sandbox, &admin, "Mine").await?;
        eventually("the run", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        if revoked {
            crate::support::subscription_moves::ready(&app, &id).await?;
            app.send_input(&id, b"first turn\r").await?;
            crate::support::usage_credits::first_turn_transcript(&data).await?;
        }
        let mark = Path::new(&run.run_dir).join("personal-mcp-of");
        assert_eq!(std::fs::read_to_string(&mark)?, admin.as_str());
        std::fs::write(&mark, "u-bob")?;
        Ok::<_, Box<dyn std::error::Error>>(id)
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build()?;
        if revoked {
            add_account(&app, "b-second").await?;
            app.deps
                .store
                .set_account_state(
                    &AccountId::parse("a-first")?,
                    AccountStatus::Relogin,
                    None,
                    None,
                    app.now(),
                )
                .await?;
        }
        app.reconcile().await?;
        eventually("the restarted run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            task(&app, &id)
                .await
                .is_ok_and(|task| task.status.is_resting())
                || (attempts.len() == 2
                    && attempts[1].state == yulius_domain::task::AttemptState::Live)
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let status = task(&app, &id).await?.status;
        stop(&app, &id).await?;
        if revoked {
            assert_eq!(
                status,
                TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled)
            );
            assert_eq!(
                attempts.len(),
                1,
                "a normal hand-over cannot move off a revoked account"
            );
        } else {
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 RST-001 (review): the reattached run that holds another person's servers sits at a
/// usage limit. How it is resumed is read from its transcript, which the new server replays
/// first: the restart waits for that and resumes the run past the limit (the interrupted turn
/// continued, on a subscription with room), not into the limit's error.
#[test]
fn a_reattached_run_at_a_limit_is_restarted_past_it() -> TestResult {
    reattach_at_limit(LimitRestartRace::Natural)
}

/// A pool move delayed after replay belongs to the old attempt. If the hand-over already
/// replaced it, processing the old limit must leave the replacement running.
#[test]
fn a_delayed_limit_move_leaves_the_reattached_runs_replacement_running() -> TestResult {
    reattach_at_limit(LimitRestartRace::LateLimit)
}

/// Startup's first capacity round sees the persisted pause while its hand-over prepares
/// the restart. It must leave that pause to the hand-over.
#[test]
fn a_capacity_round_leaves_the_reattached_runs_hand_over_to_finish() -> TestResult {
    reattach_at_limit(LimitRestartRace::CapacityRound)
}

/// The recorded refusal makes the old subscription unusable before the hand-over.
#[test]
fn a_reattached_refused_subscription_restarts_on_a_working_one() -> TestResult {
    reattach_at_limit(LimitRestartRace::AccessLost)
}

/// The ownership restart reports why a session switched inside the CLI resumes on Opus.
#[test]
fn a_reattached_switched_model_limit_is_said_at_the_ownership_restart() -> TestResult {
    reattach_at_limit(LimitRestartRace::SwitchedModel)
}

/// A required hand-over with no usable subscription ends the prior owner's host.
#[test]
fn a_reattached_limit_without_a_usable_subscription_fails_closed() -> TestResult {
    reattach_at_limit(LimitRestartRace::NoCapacity)
}

/// A refusal with no reset still belongs to the pool's retry history when the hand-over
/// moves the task. After its next subscription spends the retry, both refusals count.
#[test]
fn a_reattached_blind_limit_stays_out_of_the_pool_after_the_retry() -> TestResult {
    reattach_at_limit(LimitRestartRace::BlindRetry)
}

enum LimitRestartRace {
    Natural,
    LateLimit,
    CapacityRound,
    AccessLost,
    SwitchedModel,
    NoCapacity,
    BlindRetry,
}

fn reattach_at_limit(order: LimitRestartRace) -> TestResult {
    use yulius_app::race::Point;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let build = || {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: match order {
                LimitRestartRace::SwitchedModel => None,
                LimitRestartRace::NoCapacity | LimitRestartRace::BlindRetry => Some(0),
                _ => Some(1),
            },
            credits_out: matches!(
                order,
                LimitRestartRace::NoCapacity | LimitRestartRace::BlindRetry
            ),
            access_lost: matches!(order, LimitRestartRace::AccessLost),
            ..FakeAgentOptions::default()
        };
        build_with_github(
            &data,
            &fake,
            probe.clone(),
            agent,
            Arc::new(FakeGitHub::default()),
        )
    };
    let first = runtime()?;
    let (admin, id) = first.block_on(async {
        let app = build()?;
        let accounts: &[&str] = if matches!(order, LimitRestartRace::BlindRetry) {
            &["a-first", "b-second", "c-later"]
        } else {
            &["a-first", "b-second"]
        };
        let (sandbox, admin) = seed(&app, &data, accounts).await?;
        if matches!(order, LimitRestartRace::BlindRetry) {
            probe.set("c-later", usage(99.0, 4 * HOUR, 30.0, 100));
            reading(&app, "c-later").await?;
        }
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &admin, "b-second").await?;
        second_person(&app, &admin).await?;
        let admin_ctx = ctx_of(app.deps.store.user(&admin).await?.ok_or("admin")?);
        personal_and_shared_servers(&app, &admin_ctx, &sandbox).await?;
        let id = new_task(&app, &sandbox, &admin, "Mine").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        if matches!(order, LimitRestartRace::SwitchedModel) {
            crate::support::usage_credits::append_switched_fable_limit(&app, &data).await?;
        }
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        std::fs::write(Path::new(&run.run_dir).join("personal-mcp-of"), "u-bob")?;
        Ok::<_, Box<dyn std::error::Error>>((admin, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    // Under a controlled delay the hand-over must still progress with a single async worker.
    let second = if matches!(
        order,
        LimitRestartRace::LateLimit
            | LimitRestartRace::CapacityRound
            | LimitRestartRace::BlindRetry
    ) {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?
    } else {
        runtime()?
    };
    second.block_on(async {
        let app = build()?;
        if !matches!(order, LimitRestartRace::NoCapacity) {
            probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
            refresh(&app, &admin, "b-second").await?;
        }
        let delayed = matches!(
            order,
            LimitRestartRace::LateLimit | LimitRestartRace::BlindRetry
        )
        .then(|| {
            let (ready, go) = hold_pool_move(Point::LimitMoveReady, &id);
            (ready, go, limit_move_finished(&id))
        });
        let capacity = matches!(order, LimitRestartRace::CapacityRound)
            .then(|| hold_pool_move(Point::SwitchPrepared, &id));
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        let after_round = if let Some((ready, go)) = capacity {
            support::point_reached(ready).await?;
            let retry = app.resume_round().await;
            let status = task(&app, &id).await?.status;
            go.send(())?;
            Some((status, retry))
        } else {
            None
        };
        eventually("the hand-over restart", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|task| task.status.is_resting())
                || app.deps.store.switches(&id).await.is_ok_and(|switches| {
                    switches.iter().any(|s| {
                        s.reason == "handover"
                            && matches!(s.outcome.as_str(), "completed" | "waiting")
                    })
                })
        })
        .await?;
        if let Some((ready, go, finished)) = delayed {
            support::point_reached(ready).await?;
            go.send(())?;
            support::point_reached(finished).await?;
        }
        let status = task(&app, &id).await?.status;
        let attempts = app.deps.store.attempts(&id).await?;
        if matches!(order, LimitRestartRace::NoCapacity) {
            stop(&app, &id).await?;
            assert_eq!(
                status,
                TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled)
            );
            assert_eq!(attempts.len(), 1);
            eventually(
                "the previous owner's host to exit",
                Duration::from_secs(30),
                || async {
                    app.deps
                        .store
                        .attempt(&attempts[0].id)
                        .await
                        .is_ok_and(|run| run.is_some_and(|run| run.ended_at.is_some()))
                },
            )
            .await?;
            return Ok(());
        }
        let resumed = attempts.last().ok_or("no attempt after the hand-over")?;
        let run_dir = Path::new(&resumed.run_dir);
        let owner = std::fs::read_to_string(run_dir.join("personal-mcp-of"));
        let problems = limit_resume_problems(run_dir, &data);
        let after_retries = if matches!(order, LimitRestartRace::BlindRetry) {
            Some(spend_blind_retry(&app, &id, &probe).await?)
        } else {
            None
        };
        stop(&app, &id).await?;
        if let Some(result) = after_retries {
            assert_eq!(
                result,
                (3, TaskStatus::WaitingCapacity),
                "both refusing subscriptions stay out"
            );
        }
        if let Some((status, retry)) = after_round {
            assert_eq!(
                status,
                TaskStatus::WaitingCapacity,
                "the hand-over owns the pause"
            );
            assert_eq!(
                retry,
                Duration::from_secs(1),
                "retry the deferred pause promptly"
            );
        }
        assert_eq!(
            status,
            TaskStatus::Running,
            "the resumed task keeps running"
        );
        assert_eq!(attempts.len(), 2, "one replacement for the limited run");
        assert_eq!(resumed.state, yulius_domain::task::AttemptState::Live);
        assert_eq!(
            resumed.account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(owner?, admin.as_str());
        if matches!(order, LimitRestartRace::SwitchedModel) {
            let said = crate::support::subscription_moves::notices(&mut watch);
            assert!(said.iter().any(|n| n ==
                "Fable is used up on this subscription: this session goes on with Opus 5.5."),
                "{said:?}");
        }
        assert!(problems.is_empty(), "{problems:?}");
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

fn limit_move_finished(id: &TaskId) -> std::sync::mpsc::Receiver<()> {
    let (done, finished) = std::sync::mpsc::channel();
    yulius_app::race::on(
        yulius_app::race::Point::LimitMoveFinished,
        Path::new(id.as_str()),
        move || {
            let _ = done.send(());
        },
    );
    finished
}

/// Spend the replacement's retry, then refresh the old account while a third account
/// has a known reset. The number of attempts and the task's status show what the pool did.
async fn spend_blind_retry(
    app: &Arc<App>,
    id: &TaskId,
    probe: &ScriptedUsage,
) -> Result<(usize, TaskStatus), Box<dyn std::error::Error>> {
    for seq in 2..=3 {
        crate::support::subscription_moves::live_run(app, id, seq).await?;
        crate::support::subscription_moves::ready(app, id).await?;
        let finished = limit_move_finished(id);
        app.send_input(id, b"again\r").await?;
        support::point_reached(finished).await?;
        eventually(
            "the pause after another refusal",
            Duration::from_secs(30),
            || has_status(app, id, TaskStatus::WaitingCapacity),
        )
        .await?;
        if seq == 2 {
            reading(app, "b-second").await?;
            app.resume_round().await;
        }
    }
    probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
    reading(app, "a-first").await?;
    app.resume_round().await;
    Ok((
        app.deps.store.attempts(id).await?.len(),
        task(app, id).await?.status,
    ))
}

/// SPEC-332 MSC-004, MSC-009: a person's task run gets their personal MCP server, a task an
/// automation of theirs started does not, and a hand-over restarts a live run holding the previous
/// owner's personal server, resuming it with the new owner's servers.
#[test]
fn personal_mcp_servers_reach_only_their_holders_runs() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let github = Arc::new(FakeGitHub::default());
        github.set_perm("bob", "acme/pool", support::perms("write"));
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with_github(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent,
            github,
        )?;
        let (sandbox, admin) = seed(&app, &data, &["a-first"]).await?;
        let bob = second_person(&app, &admin).await?;
        let admin_ctx = ctx_of(app.deps.store.user(&admin).await?.ok_or("admin")?);
        let ctx = ctx_of;
        let (fleet, fleet_id) = personal_and_shared_servers(&app, &admin_ctx, &sandbox).await?;

        let id = new_task(&app, &sandbox, &admin, "Mine").await?;
        // Running, not only spawned: control waits while a launch settles its servers.
        eventually("the run", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(run_servers(&first.run_dir)?, ["docs", "fleet", "yulius"]);

        let automated = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: admin.clone(),
                prompt: "Nightly: answer with an echo".into(),
                harness: Harness::ClaudeCode,
                title: Some("Nightly".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Automation,
                automation_run: None,
                start: true,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
            })
            .await?
            .id;
        // Its launch files, not only its attempt: the attempt is recorded before the spawn writes
        // them (#1084).
        eventually("the automation's run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = automated.clone();
            async move {
                app.deps
                    .store
                    .live_attempt(&id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|run| run_servers(&run.run_dir).is_ok())
            }
        })
        .await?;
        let nightly = app
            .deps
            .store
            .live_attempt(&automated)
            .await?
            .ok_or("run")?;
        assert_eq!(run_servers(&nightly.run_dir)?, ["docs", "yulius"]);
        stop(&app, &automated).await?;

        // Turned off meanwhile, the server is still in the live run: only its owner messages it.
        let mut off = fleet.clone();
        off.enabled = Some(false);
        app.update_mcp_server(&admin_ctx, &fleet_id, &off).await?;
        let caller = yulius_app::coordination::ProjectCaller {
            sandbox: sandbox.clone(),
            user: bob.id.clone(),
            task: None,
            channel: yulius_app::coordination::Channel::Superagent,
            actor: "cockpit:bob".into(),
            detail: serde_json::json!({"via": "cockpit", "shell": "sh-test"}),
        };
        let refused = app
            .tool_task_message(&caller, &serde_json::json!({"task_id": id.as_str(), "text": "hi"}))
            .await;
        assert!(
            matches!(&refused, Err(yulius_app::error::AppError::Forbidden(m)) if m.contains("only they steer it")),
            "{refused:?}"
        );
        let mut on = fleet.clone();
        on.enabled = Some(true);
        app.update_mcp_server(&admin_ctx, &fleet_id, &on).await?;

        // Control passes to bob: the run holding the admin's server is resumed without it, on
        // the same subscription, with nothing typed.
        app.request_task_control(&ctx(bob.clone()), id.as_str())
            .await?;
        app.accept_task_control(&admin_ctx, id.as_str()).await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let second = app.deps.store.attempts(&id).await?.remove(1);
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(second.account_id, first.account_id);
        assert_eq!(run_servers(&second.run_dir)?, ["docs", "yulius"]);
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(launch.contains("prompt: None"), "nothing typed: {launch}");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Review of SPEC-332 (round 3): a hand-over whose restart of a run holding the previous owner's
/// personal MCP server is refused stops that run (fail closed); control still passes.
#[test]
fn a_hand_over_stops_a_run_holding_personal_mcp_servers_it_cannot_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let github = Arc::new(FakeGitHub::default());
        github.set_perm("bob", "acme/pool", support::perms("write"));
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with_github(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent,
            github,
        )?;
        let (sandbox, admin) = seed(&app, &data, &["a-first"]).await?;
        let bob = UserRecord {
            id: UserId::parse("u-bob")?,
            github_login: Some("bob".into()),
            display_name: "Bob".into(),
            is_superadmin: false,
            is_instance_admin: false,
            ..app.deps.store.user(&admin).await?.ok_or("admin")?
        };
        app.deps.store.insert_user(&bob).await?;
        app.deps
            .store
            .save_github_token(
                &yulius_app::model::GitHubTokenRecord {
                    user_id: bob.id.clone(),
                    access: app.deps.crypto.seal(b"token-bob")?,
                    refresh: None,
                    expires_at: None,
                    refresh_expires_at: None,
                },
                app.now(),
            )
            .await?;
        let ctx = |user: UserRecord| AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let admin_ctx = ctx(app.deps.store.user(&admin).await?.ok_or("admin")?);
        let mut fleet = mcp_input("fleet", "http");
        fleet.scope = Some("instance".into());
        fleet.audience = Some("personal".into());
        app.create_mcp_server(&admin_ctx, sandbox.as_str(), &fleet)
            .await?;
        let id = new_task(&app, &sandbox, &admin, "Mine").await?;
        // Running, not only spawned: control waits while a launch settles its servers.
        eventually("the run", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        // The next start is refused: the sandbox repository and the worktree are not there.
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        std::fs::rename(repo.join(".git"), repo.join("git-away"))?;
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        let worktree = Path::new(&worktree);
        std::fs::rename(worktree.join(".git"), worktree.join("git-away"))?;
        app.request_task_control(&ctx(bob.clone()), id.as_str())
            .await?;
        app.accept_task_control(&admin_ctx, id.as_str()).await?;
        eventually("the stopped run", Duration::from_secs(60), || async {
            app.deps
                .store
                .live_attempt(&id)
                .await
                .ok()
                .flatten()
                .is_none()
                && !task(&app, &id)
                    .await
                    .map(|t| t.status.is_live())
                    .unwrap_or(true)
        })
        .await?;
        assert_eq!(task(&app, &id).await?.owner, bob.id);
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1, "no new run");
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A Vidoc connector whose tests always pass (only the key in use matters here).
struct AnyVidoc;

#[async_trait::async_trait]
impl yulius_app::ports::vidoc::VidocConnector for AnyVidoc {
    async fn check(
        &self,
        _key: &yulius_app::model::Secret,
    ) -> yulius_app::ports::vidoc::VidocCheck {
        yulius_app::ports::vidoc::VidocCheck::Ok
    }
}

/// SPEC-195 VDK-005: with an instance Vidoc key, an agent run of a sandbox without its own
/// `VIDOC_API_KEY` gets that variable as the placeholder (the egress proxy adds the key), never
/// the key.
#[test]
fn an_agent_run_gets_the_instance_vidoc_key_as_a_placeholder() -> TestResult {
    const KEY: &str = "vidoc-instance-key-for-runs-0123456789";
    const PLACEHOLDER: &str = "yulius-secret-VIDOC_API_KEY";
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec!["VIDOC_API_KEY".into(), "YULIUS_INTEGRATION_BRANCH".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
            Arc::new(AnyVidoc),
            Ok(Some(yulius_app::model::Secret(KEY.into()))),
        ));
        app.load_vidoc_key().await;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Scan").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV VIDOC_API_KEY=") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(
            shown.contains(&format!("ENV VIDOC_API_KEY=set len={}", PLACEHOLDER.len())),
            "{shown}"
        );
        assert!(!shown.contains(KEY), "{shown}");
        // The scan's base (review round 1): the integration branch, "dev" here.
        assert!(
            shown.contains("ENV YULIUS_INTEGRATION_BRANCH=set len=3"),
            "{shown}"
        );
        Ok(())
    })
}

/// SPEC-338 VOF-002: a run of a project whose Vidoc switch is off gets `YULIUS_VIDOC=off`, so its
/// scan says "disabled", and not the instance key as `VIDOC_API_KEY`.
#[test]
fn a_switched_off_project_runs_without_vidoc() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec![
                "YULIUS_VIDOC".into(),
                "VIDOC_API_KEY".into(),
                "YULIUS_VIDOC_SWITCH".into(),
            ],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
            Arc::new(AnyVidoc),
            Ok(Some(yulius_app::model::Secret(
                "vidoc-instance-key-for-runs-0123456789".into(),
            ))),
        ));
        app.load_vidoc_key().await;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        yulius_app::ports::store::SandboxStore::set_profile(
            app.deps.store.as_ref(),
            &sandbox,
            r#"{"vidoc":false}"#,
        )
        .await?;
        let id = new_task(&app, &sandbox, &user, "Scan").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV YULIUS_VIDOC=") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("ENV YULIUS_VIDOC=set len=3"), "{shown}");
        assert!(!shown.contains("ENV VIDOC_API_KEY=set"), "{shown}");
        // SPEC-359 VSB-006: where the switch is, to turn it on again or point to it.
        let record =
            yulius_app::ports::store::SandboxStore::sandbox(app.deps.store.as_ref(), &sandbox)
                .await?
                .ok_or("sandbox")?;
        let url = format!("{}/s/{}/ci", app.config.public_url, record.slug);
        assert!(
            shown.contains(&format!("ENV YULIUS_VIDOC_SWITCH=set len={}", url.len())),
            "{shown}"
        );
        Ok(())
    })
}

/// SPEC-356 VIO-002: with Vidoc switched off for the instance, a run of a project whose own switch
/// is on gets `YULIUS_VIDOC=off` and not the instance key as `VIDOC_API_KEY`.
#[test]
fn a_run_follows_the_instance_vidoc_switch() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec!["YULIUS_VIDOC".into(), "VIDOC_API_KEY".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
            Arc::new(AnyVidoc),
            Ok(Some(yulius_app::model::Secret(
                "vidoc-instance-key-for-runs-0123456789".into(),
            ))),
        ));
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        yulius_app::ports::store::InstanceStore::init_instance(
            app.deps.store.as_ref(),
            "https://yulius.test",
            "setup-token-hash",
        )
        .await?;
        yulius_app::ports::store_ext::UpdateStore::set_vidoc_off(
            app.deps.store.as_ref(),
            Some("superadmin"),
            yulius_domain::time::Timestamp(1_800_000_000),
        )
        .await?;
        app.load_vidoc_key().await;
        let id = new_task(&app, &sandbox, &user, "Scan").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV YULIUS_VIDOC=") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("ENV YULIUS_VIDOC=set len=3"), "{shown}");
        assert!(!shown.contains("ENV VIDOC_API_KEY=set"), "{shown}");
        Ok(())
    })
}

/// SPEC-275 VAR-004, VAR-002: a task run gets the project's variables in its environment, in
/// plain; an agent secret of the same name (created after the variable) wins with its
/// placeholder.
#[test]
fn a_task_run_gets_the_project_variables() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec![
                "OVH_ENDPOINT".into(),
                "LANGFUSE_BASE_URL".into(),
                "SHOP_TOKEN".into(),
                "PERL5LIB".into(),
                "RUST_TEST_THREADS".into(),
            ],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let var =
            |name: &str, value: &str| yulius_contracts::api::variables::ProjectVariableInput {
                name: name.into(),
                value: value.into(),
                description: None,
            };
        app.set_project_variables(
            &ctx,
            sandbox.as_str(),
            &[
                var("OVH_ENDPOINT", "ovh-ca"),
                var("LANGFUSE_BASE_URL", "https://cloud.langfuse.com"),
                var("SHOP_TOKEN", "plain"),
                // Review round 1: Yulius's own environment wins over a variable of its name.
                var("RUST_TEST_THREADS", "123456789"),
            ],
            false,
        )
        .await?;
        app.create_agent_secret(
            &ctx,
            sandbox.as_str(),
            &yulius_contracts::api::secrets::AgentSecretInput {
                name: "SHOP_TOKEN".into(),
                value: "shop-token-value-0123456789".into(),
                ..Default::default()
            },
        )
        .await?;

        // Seed a pre-ADR-0408 row. This run still receives its interpreter-named placeholder.
        let old = app
            .create_agent_secret(
                &ctx,
                sandbox.as_str(),
                &yulius_contracts::api::secrets::AgentSecretInput {
                    name: "OLD_KEY".into(),
                    value: "synthetic-old-value-123".into(),
                    ..Default::default()
                },
            )
            .await?;
        let saved = app
            .deps
            .store
            .vault_get(&old.id)
            .await?
            .ok_or("old secret")?;
        let mut legacy = saved.clone();
        legacy.name = "PERL5LIB".into();
        assert!(app.deps.store.vault_update(&legacy, &saved).await?);

        let id = new_task(&app, &sandbox, &user, "Vars").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV RUST_TEST_THREADS=") }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("ENV OVH_ENDPOINT=set len=6"), "{shown}");
        assert!(
            shown.contains("ENV LANGFUSE_BASE_URL=set len=26"),
            "{shown}"
        );
        assert!(shown.contains("ENV RUST_TEST_THREADS=set len="), "{shown}");
        assert!(
            !shown.contains("ENV RUST_TEST_THREADS=set len=9"),
            "the base environment's value wins: {shown}"
        );
        let placeholder = "yulius-secret-SHOP_TOKEN";
        assert!(
            shown.contains(&format!("ENV SHOP_TOKEN=set len={}", placeholder.len())),
            "{shown}"
        );
        assert!(
            shown.contains(&format!(
                "ENV PERL5LIB=set len={}",
                "yulius-secret-PERL5LIB".len()
            )),
            "{shown}"
        );
        Ok(())
    })
}

/// SPEC-095 PMC-001/002/005: the servers of the repository's `.mcp.json` join the sandbox's MCP
/// servers before the run, audited, and reach the run like any server configured in Yulius. One
/// already configured (here turned off) is left as it is. Agents can write the file, so a variable
/// it names, even as `${secret:…}`, reaches the server as the agent secret's placeholder, never as
/// its value: the egress proxy puts the value in on the secret's bound hosts only (ADR-20).
#[test]
fn a_repository_mcp_json_reaches_the_run_through_yulius() -> TestResult {
    const APIFY_TOKEN: &str = "synthetic-apify-token-95-0123456789abcdef";
    const PLACEHOLDER: &str = "yulius-secret-APIFY_TOKEN";
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec![
                "YULIUS_MCP_APIFY_APIFY_TOKEN".into(),
                "YULIUS_MCP_LINEAR_LINEAR_KEY".into(),
            ],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, Arc::new(ScriptedUsage::default()), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        std::fs::write(
            data.join("sandboxes")
                .join(sandbox.as_str())
                .join("repo/.mcp.json"),
            r#"{"mcpServers": {
                "apify": {"command": "npx", "args": ["-y", "@apify/actors-mcp-server"],
                          "env": {"APIFY_TOKEN": "${APIFY_TOKEN}"}},
                "pipedream-gmail": {"type": "http", "url": "https://mcp.pipedream.test/gmail",
                                    "headers": {"Authorization": "Bearer ${secret:APIFY_TOKEN}"}},
                "Linear": {"type": "stdio", "command": "linear-mcp",
                           "env": {"LINEAR_KEY": "from-the-file"}}
            }}"#,
        )?;
        let mut linear = mcp_input("linear", "stdio");
        linear.enabled = Some(false);
        app.create_mcp_server(&ctx, sandbox.as_str(), &linear)
            .await?;
        app.create_agent_secret(
            &ctx,
            sandbox.as_str(),
            &yulius_contracts::api::secrets::AgentSecretInput {
                name: "APIFY_TOKEN".into(),
                hosts: vec!["api.apify.test".into()],
                value: APIFY_TOKEN.into(),
                ..Default::default()
            },
        )
        .await?;

        let id = new_task(&app, &sandbox, &user, "Tools").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV YULIUS_MCP_LINEAR_LINEAR_KEY=") }
        })
        .await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        let (mcp, text, _) = run_mcp(&run.run_dir)?;
        let servers = mcp["mcpServers"].as_object().ok_or("mcpServers")?;
        let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["apify", "pipedream-gmail", "yulius"], "{text}");
        assert_eq!(servers["apify"]["command"], "npx");
        assert_eq!(
            servers["apify"]["env"]["APIFY_TOKEN"],
            "${YULIUS_MCP_APIFY_APIFY_TOKEN}"
        );
        assert_eq!(
            servers["pipedream-gmail"]["url"],
            "https://mcp.pipedream.test/gmail"
        );
        assert_eq!(
            servers["pipedream-gmail"]["headers"]["Authorization"],
            "Bearer ${YULIUS_MCP_PIPEDREAM_GMAIL_TOKEN}"
        );
        assert!(!text.contains(APIFY_TOKEN));
        let shown = screen(&app, &id);
        assert!(
            shown.contains(&format!(
                "ENV YULIUS_MCP_APIFY_APIFY_TOKEN=set len={}",
                PLACEHOLDER.len()
            )),
            "{shown}"
        );
        let record = app.deps.store.sandbox(&sandbox).await?.ok_or("sandbox")?;
        let attached = format!(
            "{:?}",
            app.mcp_attachments(&record, yulius_app::vault::Holder::Nobody)
                .await?
        );
        assert!(!attached.contains(APIFY_TOKEN), "{attached}");
        assert!(
            attached.contains(&format!("(\"APIFY_TOKEN\", \"{PLACEHOLDER}\")")),
            "{attached}"
        );
        assert!(
            attached.contains(&format!("bearer: Some(\"{PLACEHOLDER}\")")),
            "{attached}"
        );
        assert!(
            shown.contains("ENV YULIUS_MCP_LINEAR_LINEAR_KEY=unset"),
            "{shown}"
        );
        let stored = app.deps.store.mcp_servers(&sandbox, None).await?;
        let mut listed: Vec<(&str, bool)> = stored
            .iter()
            .map(|s| (s.name.as_str(), s.enabled))
            .collect();
        listed.sort_unstable();
        assert_eq!(
            listed,
            [
                ("apify", true),
                ("linear", false),
                ("pipedream-gmail", true)
            ]
        );
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let mut audited: Vec<(String, String)> = db
            .prepare("SELECT actor, detail FROM audit_log WHERE action = 'mcp.imported'")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        audited.sort_unstable();
        assert_eq!(
            audited,
            [
                (
                    "yulius".to_owned(),
                    "apify (sandbox, team, stdio)".to_owned()
                ),
                (
                    "yulius".to_owned(),
                    "pipedream-gmail (sandbox, team, http)".to_owned()
                ),
            ]
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10: a pause survives a Yulius restart — the new server reattaches the paused run,
/// keeps it paused, and resumes it in place once capacity returns.
#[test]
fn a_pause_survives_a_server_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let first = runtime()?;
    let (user, id, child) = first.block_on(async {
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Restart").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        probe.set("a-only", usage(99.0, HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        app.send_input(&id, b"before\r").await?;
        eventually("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let live = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        Ok::<_, Box<dyn std::error::Error>>((user, id, live.child_pid))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build(&data, &fake, probe.clone())?;
        app.reconcile().await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::WaitingCapacity,
            "still paused"
        );
        assert!(app.deps.store.open_pause(&id).await?.is_some());
        let _ = app.resume_round().await;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::WaitingCapacity,
            "no capacity yet"
        );
        probe.set("a-only", usage(1.0, 5 * HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        let _ = app.resume_round().await;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let live = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(live.child_pid, child, "the same process, never respawned");
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        app.send_input(&id, b"after\r").await?;
        eventually(
            "the echo after the restart",
            Duration::from_secs(10),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("ECHO: after") }
            },
        )
        .await?;
        assert!(
            !screen(&app, &id).contains("ECHO: Continue"),
            "nothing typed"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10, SPEC-034: a turn the CLI itself interrupted with a usage limit, with every
/// subscription exhausted, pauses at the limit; it resumes on the subscription that recovers by a
/// native resume that steps the transcript back over the limit and lets Claude Code continue the
/// turn itself: no prompt, nothing typed, no "Continue.".
#[test]
fn a_turn_interrupted_by_a_limit_resumes_without_a_typed_continue() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            report_env: vec!["CLAUDE_CODE_RESUME_INTERRUPTED_TURN".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit);
        assert_eq!(
            pause.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );

        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        eventually("the flag in the agent's environment", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV CLAUDE_CODE_RESUME_INTERRUPTED_TURN=set") }
        })
        .await?;
        assert!(!screen(&app, &id).contains("Continue."), "nothing typed");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A subscription that stops working mid-task (cancelled, so Claude Code answers "Your
/// organization has disabled Claude subscription access", 403) needs a new sign-in: the task
/// moves on to the working subscription by itself, in the same session and past the refused
/// turn, like at a usage limit, and is not finished by it. A later usage reading of the refused
/// subscription leaves it out of the pool.
#[test]
fn a_refused_subscription_moves_the_task_to_a_working_one() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            access_lost: true,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        // The task starts on a-first; b-second has room again by the time a-first is refused.
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Refused").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?;
        assert_eq!(
            first[0].account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;

        app.send_input(&id, b"hello\r").await?;
        // The run's attempt is live a moment before its task is Running (two writes): wait for both.
        eventually("the move to b-second", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2
                && attempts[1].state == yulius_domain::task::AttemptState::Live
                && has_status(&app, &id, TaskStatus::Running).await
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            has_status(&app, &id, TaskStatus::Running).await,
            "the refused turn does not end the task"
        );

        let refused = AccountId::parse("a-first")?;
        let record = app.deps.store.account(&refused).await?.ok_or("account")?;
        assert_eq!(record.status, AccountStatus::Relogin);
        assert!(
            record
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("disabled Claude subscription access")),
            "{:?}",
            record.last_error
        );
        // Its usage still answers: that does not make it usable again.
        probe.set("a-first", usage(0.0, 5 * HOUR, 0.0, 100));
        let _ = refresh(&app, &user, "a-first").await;
        let record = app.deps.store.account(&refused).await?.ok_or("account")?;
        assert_eq!(record.status, AccountStatus::Relogin);
        // Nor does it erase why (SPEC-447 MOV-005).
        assert!(
            record
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("disabled Claude subscription access")),
            "{:?}",
            record.last_error
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-034: a turn the limit cut on the only subscription resumes when that subscription
/// recovers, on the same subscription, past the limit: a native resume of the same session with
/// no prompt, and nothing typed into the paused process (it used to get "Continue.").
#[test]
fn a_limit_on_the_only_subscription_resumes_past_the_limit_when_it_recovers() -> TestResult {
    resume_live_limit(false)
}

/// A later refusal on a reattached attempt is live once its unchanged owner's replay ends.
#[test]
fn a_live_limit_after_an_unchanged_owner_reattach_resumes_in_the_capacity_round() -> TestResult {
    resume_live_limit(true)
}

fn resume_live_limit(reattach: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let build = || {
        build_with(
            &data,
            &fake,
            probe.clone(),
            FakeAgentOptions {
                transcript: true,
                limit_after: Some(if reattach { 2 } else { 1 }),
                ..FakeAgentOptions::default()
            },
        )
    };
    let first = runtime()?;
    let (app, user, id) = first.block_on(async {
        let app = build()?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        crate::support::subscription_moves::ready(&app, &id).await?;
        if reattach {
            app.send_input(&id, b"first turn\r").await?;
            crate::support::usage_credits::first_turn_transcript(&data).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>((app, user, id))
    })?;
    let (rt, app) = if reattach {
        drop(app);
        first.shutdown_timeout(Duration::from_secs(5));
        let second = runtime()?;
        let app = second.block_on(async {
            let app = build()?;
            app.reconcile().await?;
            eventually("the transcript replay", Duration::from_secs(30), || async {
                app.runtime(&id).transcript_rounds() >= 2
            })
            .await?;
            Ok::<_, Box<dyn std::error::Error>>(app)
        })?;
        (second, app)
    } else {
        (first, app)
    };
    rt.block_on(async {
        let (paused, go) = hold_pool_move(yulius_app::race::Point::LimitPauseStored, &id);
        app.send_input(&id, b"hello\r").await?;
        support::point_reached(paused).await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        assert_eq!(
            app.deps.store.open_pause(&id).await?.ok_or("pause")?.kind,
            PauseKind::Limit
        );

        let mut notices = app.runtime(&id).hub.watch();
        probe.set("a-first", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        let after_round = task(&app, &id).await?.status;
        go.send(())?;
        assert_eq!(
            after_round,
            TaskStatus::Running,
            "a live limit pause resumes in this round"
        );
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(!screen(&app, &id).contains("Continue."), "nothing typed");
        let mut said = Vec::new();
        while let Ok(frame) = notices.try_recv() {
            if let yulius_app::hub::HubFrame::Notice(text) = frame {
                said.push(text);
            }
        }
        assert!(
            said.iter().any(|n| n.contains("continues this session"))
                && !said.iter().any(|n| n.contains("another")),
            "the same subscription: {said:?}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-034 (review): a person's switch of a task paused at a usage limit resumes it past the
/// limit too, with no prompt and the transcript stepped back.
#[test]
fn a_manual_switch_at_a_limit_resumes_past_the_limit() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        )
        .await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-072 LRS-001 (#276): Claude Code continues the turn a limit cut by itself at the reset
/// ("continuing automatically") before the resume loop acts: the `limit` pause ends as
/// overtaken and the task runs; capacity returning later starts no second run, so nothing
/// terminates the turn the CLI continued.
#[test]
fn a_turn_the_cli_continues_by_itself_ends_the_limit_pause() -> TestResult {
    the_cli_continuation_ends_the_limit_pause(false)
}

/// SPEC-072 LRS-001 (review): a continuation that is only the answer ending the turn (no tool
/// call, so no agent work) ends the `limit` pause too.
#[test]
fn a_continuation_that_only_answers_ends_the_limit_pause() -> TestResult {
    the_cli_continuation_ends_the_limit_pause(true)
}

fn the_cli_continuation_ends_the_limit_pause(answer_only: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        // Long enough for the pause to exist before the continuation, also under load.
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            limit_continues_ms: Some(5_000),
            limit_continues_answer_only: answer_only,
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let since = now();
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually(
            "the pause at the limit",
            Duration::from_secs(30),
            || async {
                matches!(
                    app.deps.store.open_pause(&id).await,
                    Ok(Some(pause)) if pause.kind == PauseKind::Limit
                )
            },
        )
        .await?;
        eventually(
            "the CLI's own continuation",
            Duration::from_secs(30),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("CONTINUED") }
            },
        )
        .await?;
        eventually("the pause to end", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        let pauses = app.deps.store.pauses_since(since).await?;
        assert!(
            pauses
                .iter()
                .any(|p| p.kind == PauseKind::Limit && p.outcome.as_deref() == Some("overtaken")),
            "{pauses:?}"
        );

        probe.set("a-first", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 1, "no second run: {attempts:?}");
        assert_eq!(attempts[0].state, yulius_domain::task::AttemptState::Live);
        eventually("the continued turn's end", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("DONE continued") }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-072 LRS-002 (#277): the switch after a usage limit is refused before it ends the session
/// (the other subscription's credential cannot be opened), so the task stays at its limit with no
/// pause; a person's switch once the subscription works again still resumes past the limit: no
/// prompt, the interrupted turn continued, the transcript stepped back.
#[test]
fn a_manual_switch_after_a_refused_limit_switch_resumes_past_the_limit() -> TestResult {
    use yulius_app::ports::Crypto;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("a-first", usage(10.0, 4 * HOUR, 20.0, 100));
        refresh(&app, &user, "a-first").await?;
        probe.set("b-second", usage(40.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let b = AccountId::parse("b-second")?;
        let foreign = MasterKeyCrypto::from_key(&[9u8; 32])?.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&b, &foreign, AccountStatus::Ok, now())
            .await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        assert_eq!(
            app.deps.store.attempts(&id).await?[0]
                .account_id
                .as_ref()
                .map(AccountId::as_str),
            Some("a-first")
        );
        app.send_input(&id, b"hello\r").await?;
        eventually("the refused switch", Duration::from_secs(30), || async {
            app.deps
                .store
                .switches(&id)
                .await
                .is_ok_and(|s| s.iter().any(|s| s.outcome == "refused"))
        })
        .await?;
        assert!(app.deps.store.open_pause(&id).await?.is_none(), "no pause");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);

        let sealed = app.deps.crypto.seal(b"b-second")?;
        app.deps
            .store
            .set_account_credential(&b, &sealed, AccountStatus::Ok, now())
            .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        )
        .await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A search document of `task`'s session `session` whose index has read `bytes`: what
/// `sessions_search` leaves behind (SPEC-034: a rewritten transcript is indexed again).
async fn indexed_doc(
    app: &App,
    sandbox: &SandboxId,
    task: &TaskId,
    session: &str,
    bytes: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    use yulius_app::ports::session_docs::{SessionDocRecord, SessionKind};
    let id = format!("doc-{session}");
    app.deps
        .store
        .insert_session_doc(&SessionDocRecord {
            id: id.clone(),
            sandbox_id: sandbox.clone(),
            kind: SessionKind::Task,
            task_id: Some(task.clone()),
            harness: Harness::ClaudeCode,
            session_id: session.to_owned(),
            title: "Limit".into(),
            author_id: None,
            git_branch: None,
            cwd_relative: None,
            base: None,
            transcript_path: String::new(),
            sha256: None,
            bytes,
            redactions: 0,
            started_at: None,
            ended_at: None,
            continued_task_id: None,
            indexed_bytes: bytes,
            indexed_lines: 1,
            index_full: false,
            created_at: now(),
            updated_at: now(),
        })
        .await?;
    Ok(id)
}

/// How far the index of document `doc` has read.
async fn indexed_bytes(app: &App, doc: &str) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(app
        .deps
        .store
        .session_doc(doc)
        .await?
        .ok_or("document")?
        .indexed_bytes)
}

/// SPEC-072 LRS-003 (#278): a resume past a limit whose transcript has nothing to step back
/// (no limit record left) leaves the transcript and its search index alone.
#[test]
fn a_resume_past_a_limit_keeps_the_index_of_a_transcript_it_did_not_rewrite() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?[0].clone();
        let session = first.provider_session_id.clone().ok_or("session")?;
        let transcript = app.deps.harness.claude_transcript(
            &data.join("sandboxes").join(sandbox.as_str()).join("home"),
            Path::new(&task(&app, &id).await?.worktree_path.ok_or("worktree")?),
            &session,
        );
        // The limit record is gone already, as after an earlier step back.
        let text = std::fs::read_to_string(&transcript)?;
        let kept: String = text
            .lines()
            .filter(|l| !l.contains("usage_limit_reached"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_ne!(kept, text, "the fake agent wrote its limit record");
        std::fs::write(&transcript, &kept)?;
        let doc = indexed_doc(&app, &sandbox, &id, &session, 40).await?;

        probe.set("a-first", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 2, "{attempts:?}");
        assert!(limit_resume_problems(Path::new(&attempts[1].run_dir), &data).is_empty());
        assert_eq!(std::fs::read_to_string(&transcript)?.len(), kept.len());
        assert_eq!(indexed_bytes(&app, &doc).await?, 40, "the index is kept");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-034 LIM-003, SPEC-072 LRS-004 (#278): a person's switch of a task at its limit when no
/// subscription has capacity ends the session and finds nothing to launch on, so the task waits in
/// a `limit` pause without a process; when capacity returns, that pause resumes past the limit (a
/// continued resume without a prompt), and the transcript it rewrote is indexed again.
#[test]
fn a_limit_pause_without_a_process_resumes_past_the_limit() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: None,
                prompt: None,
            },
        )
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit, "{pause:?}");
        assert_eq!(pause.account_id, None, "no process: {pause:?}");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::WaitingCapacity);
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        let session = app.deps.store.attempts(&id).await?[0]
            .provider_session_id
            .clone()
            .ok_or("session")?;
        let doc = indexed_doc(&app, &sandbox, &id, &session, 40).await?;

        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            app.deps.store.open_pause(&id).await?.is_none(),
            "the start that was made ended the pause"
        );
        assert_eq!(indexed_bytes(&app, &doc).await?, 0, "indexed again");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A pool with one session slot, so that a second task's start waits in the queue.
fn build_one_slot(
    data: &Path,
    fake: &Path,
    usage: Arc<ScriptedUsage>,
    agent: FakeAgentOptions,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_slots(data, fake, usage, agent, u32::MAX)
}

/// [`build_one_slot`] whose queue takes at most `queue_max` tasks.
fn build_slots(
    data: &Path,
    fake: &Path,
    usage: Arc<ScriptedUsage>,
    agent: FakeAgentOptions,
    queue_max: u32,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_served_tuned(
        data,
        fake,
        usage,
        agent,
        Arc::new(FakeGitHub::default()),
        "http://127.0.0.1:9",
        |config| {
            config.max_live_sessions = 1;
            if queue_max != u32::MAX {
                config.queue_max = queue_max;
            }
        },
    )
}

/// A task that takes the only session slot and keeps it until it is stopped.
async fn slot_holder(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let holder = new_task(app, sandbox, user, "Holds the slot").await?;
    eventually("the holder's run", Duration::from_secs(30), || {
        let app = app.clone();
        let holder = holder.clone();
        async move { screen(&app, &holder).contains("FAKE-AGENT READY") }
    })
    .await?;
    Ok(holder)
}

/// SPEC-483 QST-001: a `limit` pause without a process becomes resumable while every session
/// slot is taken, so its start waits in the queue. When the slot frees, the queue starts it as
/// that start was to be: past the limit (the transcript stepped back, the interrupted turn
/// continued), not as a plain resume that sits at the limit's error. The subscription is the
/// pool's choice at that moment, not the one it chose when the start began to wait.
#[test]
fn a_resume_past_a_limit_that_waits_in_the_queue_is_still_one() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_one_slot(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second", "c-third"]).await?;
        for full in ["b-second", "c-third"] {
            probe.set(full, usage(99.0, 4 * HOUR, 30.0, 100));
            refresh(&app, &user, full).await?;
        }
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        // No subscription has capacity: the session ends and the task waits without a process.
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: None,
                prompt: None,
            },
        )
        .await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());

        // Capacity returns, and another task takes the only slot before the pool's next round.
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let holder = slot_holder(&app, &sandbox, &user).await?;
        let _ = app.resume_round().await;
        eventually("the start in the queue", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Queued)
        })
        .await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);

        // While it waits, the subscription the pool had chosen fills up and another frees.
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        probe.set("c-third", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        refresh(&app, &user, "c-third").await?;
        stop(&app, &holder).await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("c-third"),
            "the pool's choice when the slot freed"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 QST-001: a person continues a task on a subscription they chose while every session
/// slot is taken. The start waits in the queue, also across a restart of Yulius, and runs on
/// that subscription when the slot frees, not on the one the pool would choose ("Continue on…"
/// never lands elsewhere unsaid, SPEC-447 MOV-002).
#[test]
fn a_continue_on_a_chosen_subscription_keeps_it_through_the_queue() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let agent = || FakeAgentOptions {
        transcript: true,
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (id, holder) = first.block_on(async {
        let app = build_one_slot(&data, &fake, probe.clone(), agent())?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        // The pool's own choice is `a-first`: its window ends first.
        probe.set("a-first", usage(10.0, HOUR, 30.0, 100));
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Continued").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?;
        assert_eq!(
            first[0].account_id.as_ref().map(AccountId::as_str),
            Some("a-first"),
            "the pool's own choice"
        );
        stop(&app, &id).await?;

        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let holder = slot_holder(&app, &sandbox, &user).await?;
        // A refused named Continue cannot take a queue place or clear the owner's decision.
        // Seed an archived row without running disk upkeep against this fake Git worktree.
        app.deps
            .store
            .update_task(
                &id,
                &yulius_app::ports::store::TaskFieldUpdate {
                    archived_at: Some(Some(app.now())),
                    ..Default::default()
                },
                app.now(),
            )
            .await?;
        crate::support::parking::ask(&app, &id).await?;
        let selected = task(&app, &id).await?;
        probe.set("b-second", usage(100.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let refusal = app
            .continue_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask {
                    account_id: Some("b-second".into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(
            matches!(refusal, Err(yulius_app::error::AppError::Capacity(_))),
            "{refusal:?}"
        );
        let current = task(&app, &id).await?;
        assert_eq!(current.status, selected.status);
        assert_eq!(current.archived_at, selected.archived_at);
        assert!(app.deps.store.task_needs_you(&id).await?.is_some());
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        app.continue_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        )
        .await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
        Ok::<_, Box<dyn std::error::Error>>((id, holder))
    })?;
    // Yulius restarts while the task waits: the holder's session lives on and keeps the slot.
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_one_slot(&data, &fake, probe.clone(), agent())?;
        app.reconcile().await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
        assert_eq!(task(&app, &holder).await?.status, TaskStatus::Running);
        stop(&app, &holder).await?;
        eventually("the continued run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 QST-001 (review): the subscription a person chose fills up while the task waits in
/// the queue. When the slot frees, the task is not failed for it (its person is no longer
/// looking): the choice is given up, said in the task's terminal, and the pool places the task.
#[test]
fn a_chosen_subscription_that_filled_up_while_waiting_is_given_up() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_one_slot(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("a-first", usage(10.0, HOUR, 30.0, 100));
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Continued").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        stop(&app, &id).await?;
        let holder = slot_holder(&app, &sandbox, &user).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.continue_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: Some("b-second".into()),
                ..Default::default()
            },
        )
        .await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
        let mut watch = app.runtime(&id).hub.watch();

        probe.set("b-second", usage(100.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        stop(&app, &holder).await?;
        // The run's attempt is live a moment before its task is Running (two writes): wait for both.
        eventually("the continued run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2
                && attempts[1].state == yulius_domain::task::AttemptState::Live
                && has_status(&app, &id, TaskStatus::Running).await
                || task(&app, &id).await.is_ok_and(|t| t.status.is_resting())
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            (
                task(&app, &id).await?.status,
                attempts
                    .last()
                    .and_then(|run| run.account_id.as_ref())
                    .map(AccountId::as_str)
            ),
            (TaskStatus::Running, Some("a-first")),
            "the pool placed it: {} run(s)",
            attempts.len()
        );
        let mut said = Vec::new();
        while let Ok(frame) = watch.try_recv() {
            if let yulius_app::hub::HubFrame::Notice(text) = frame {
                said.push(text);
            }
        }
        assert!(
            said.iter().any(|line| line
                == "↻ The subscription chosen for this task cannot take it now, so Yulius chose one."),
            "{said:?}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 QST-002 (review, from before): a paused task becomes resumable while every slot is
/// taken and the queue is full, so its start can neither run nor wait. Its pause stays open,
/// with its kind: the pool's next round tries again. (The pause used to be ended first, and
/// nothing ever started the task again.)
#[test]
fn a_resume_that_finds_the_queue_full_keeps_its_pause() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_slots(&data, &fake, probe.clone(), agent, 1)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                account_id: None,
                prompt: None,
            },
        )
        .await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());

        // Capacity returns; one task takes the slot and another the queue's one place.
        probe.set("b-second", usage(10.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let holder = slot_holder(&app, &sandbox, &user).await?;
        let filler = new_task(&app, &sandbox, &user, "Fills the queue").await?;
        assert_eq!(task(&app, &filler).await?.status, TaskStatus::Queued);
        let _ = app.resume_round().await;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::WaitingCapacity);
        let pause = app
            .deps
            .store
            .open_pause(&id)
            .await?
            .ok_or("the pause is gone")?;
        assert_eq!(pause.kind, PauseKind::Limit, "{pause:?}");

        // The queue's place frees (its task takes the slot): the next round queues the start,
        // and it runs past the limit when its turn comes.
        stop(&app, &holder).await?;
        eventually("the queue's task running", Duration::from_secs(30), || {
            has_status(&app, &filler, TaskStatus::Running)
        })
        .await?;
        let _ = app.resume_round().await;
        eventually("the start in the queue", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Queued)
        })
        .await?;
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        stop(&app, &filler).await?;
        eventually("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let problems = limit_resume_problems(Path::new(&attempts[1].run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-072 LRS-001 (review): after a Yulius restart the reattached run's transcript is read again
/// from its start. A usage limit that Claude Code's own continuation already overtook is history:
/// it does not mark the subscription limited again, pause the task or switch it.
#[test]
fn a_restart_does_not_replay_a_limit_the_cli_continued_past() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let agent = FakeAgentOptions {
        transcript: true,
        limit_after: Some(1),
        limit_continues_ms: Some(5_000),
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (id, usage_before) = first.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        eventually("the continued turn's end", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("DONE continued") }
        })
        .await?;
        eventually("the pause to end", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Running)
        })
        .await?;
        // Working until the replay settles it: the wait after the restart proves the replay ran.
        assert_eq!(
            task(&app, &id).await?.activity,
            Some(yulius_domain::task::Activity::Working)
        );
        let account = AccountId::parse("a-first")?;
        let usage = app.deps.store.account(&account).await?.ok_or("account")?;
        Ok::<_, Box<dyn std::error::Error>>((id, usage.usage_json))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        app.reconcile().await?;
        // The replay is applied once caught up: the continued turn ended, so the task waits.
        eventually(
            "the replay's final state",
            Duration::from_secs(30),
            || async {
                task(&app, &id).await.is_ok_and(|t| {
                    t.status != TaskStatus::Running
                        || t.activity == Some(yulius_domain::task::Activity::Waiting)
                })
            },
        )
        .await?;
        let account = AccountId::parse("a-first")?;
        assert_eq!(
            app.deps
                .store
                .account(&account)
                .await?
                .ok_or("account")?
                .usage_json,
            usage_before,
            "the old limit is not recorded again"
        );
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        assert!(app.deps.store.open_pause(&id).await?.is_none(), "no pause");
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-072 LRS-001 (review round 2): Yulius is down while the task waits at its limit, and
/// Claude Code continues the turn by itself meanwhile. The reattached run's replay ends the
/// `limit` pause, so capacity returning starts no second run and nothing ends the continued turn.
#[test]
fn a_continuation_while_yulius_is_down_ends_the_limit_pause_at_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let agent = FakeAgentOptions {
        transcript: true,
        limit_after: Some(1),
        limit_continues_ms: Some(5_000),
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (user, id, transcript) = first.block_on(async {
        let app = build_with(&data, &fake, probe.clone(), agent.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        eventually("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let session = app.deps.store.attempts(&id).await?[0]
            .provider_session_id
            .clone()
            .ok_or("session")?;
        let transcript = app.deps.harness.claude_transcript(
            &data.join("sandboxes").join(sandbox.as_str()).join("home"),
            Path::new(&task(&app, &id).await?.worktree_path.ok_or("worktree")?),
            &session,
        );
        Ok::<_, Box<dyn std::error::Error>>((user, id, transcript))
    })?;
    // Yulius stops before the continuation; the detached run goes on.
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        eventually(
            "the continuation while Yulius is down",
            Duration::from_secs(30),
            || {
                let transcript = transcript.clone();
                async move {
                    std::fs::read_to_string(&transcript)
                        .is_ok_and(|t| t.contains("msg_fake_continue_end"))
                }
            },
        )
        .await?;
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        // The first server never saw the continuation: only the replay can end the pause.
        assert!(
            app.deps.store.open_pause(&id).await?.is_some(),
            "still paused"
        );
        app.reconcile().await?;
        eventually(
            "the pause to end at the replay",
            Duration::from_secs(30), // The pause closes, then the task runs: wait for both.
            || async {
                app.deps
                    .store
                    .open_pause(&id)
                    .await
                    .is_ok_and(|p| p.is_none())
                    && has_status(&app, &id, TaskStatus::Running).await
            },
        )
        .await?;
        probe.set("a-first", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        let _ = app.resume_round().await;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 1, "no second run: {attempts:?}");
        assert_eq!(attempts[0].state, yulius_domain::task::AttemptState::Live);
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
