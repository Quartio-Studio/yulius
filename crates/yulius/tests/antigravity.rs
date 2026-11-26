//! Antigravity as a third harness with real processes: supervisor → detached
//! `yulius ptyhost` → fake agent writing Antigravity CLI 1.5.10 shaped transcripts and running its
//! hooks through `yulius hook antigravity`. An Antigravity task runs on a Google account in its own
//! home, its conversation is learnt from its SessionStart and watched, it moves to another Google
//! account at a limit by resuming the conversation, a goal task gets its request as its first
//! message, and a refreshed sign-in is kept. No test calls Google.

use crate::support::antigravity::{API_KEY, add_google_account, api_key};
use crate::support::live::{Live, UNSERVED};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::support;
use crate::support::agents::FakeAgentOptions;
use crate::support::live::supervision::{
    build_full, runtime, screen_contains, seed_records, seed_repo, task,
};
use crate::support::{TestResult, eventually};
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountStatus, LaunchKind, TaskMode, TaskOrigin};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_harness::login::antigravity_sign_in_credential;
use yulius_ptyhost::test_support::locate_fake_agent;

async fn new_agy_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    mode: TaskMode,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::Antigravity,
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

/// An app whose agents' hooks reach it: its internal listener serves them, as `yulius serve`'s
/// does, so the fake agent's hooks go through `yulius hook antigravity` to it.
async fn build_hooked(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    Ok(Live::new(Some(fake), agent).start(data, 0, |_| {}).await?.0)
}

fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// An Antigravity
/// task runs on a Google account (never the Claude one) in `<sandbox>/antigravity/<account>`,
/// whose settings, hooks and MCP configuration are private and whose conversations are the
/// sandbox's; its conversation is learnt from its SessionStart (sent through
/// `yulius hook antigravity`); its turns and tokens are read from the transcript; it runs Gemini 4
/// Neon by default.
#[test]
fn an_antigravity_task_runs_in_its_home() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            report_env: vec!["HOME".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        let account = add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity echo", TaskMode::Prompt).await?;
        eventually("the fake agent banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        assert_eq!(attempt.harness, Harness::Antigravity);
        assert_eq!(
            attempt.account_id.as_ref(),
            Some(&account),
            "a Google account"
        );
        assert_eq!(attempt.model.as_deref(), Some("gemini-4-neon"));
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("model=None"),
            "the task names none: {launch}"
        );

        let sandbox_dir = data.join("sandboxes").join(sandbox.as_str());
        // The run's own home, in its run directory: no agent of the sandbox could have made it
        // a link before a contained run binds its `.gemini`.
        let agy_home = Path::new(&attempt.run_dir).join("antigravity-home");
        let gemini = agy_home.join(".gemini");
        assert!(!gemini.starts_with(&sandbox_dir), "{gemini:?}");
        // This run is uncontained: its HOME is that home.
        let home_len = agy_home.as_os_str().len();
        assert!(
            screen_contains(&app, &id, &format!("ENV HOME=set len={home_len}")).await,
            "{}",
            app.runtime(&id).hub.screen_text()
        );
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        for file in [
            "antigravity-cli/settings.json",
            "antigravity-cli/cache/onboarding.json",
            "config/hooks.json",
            "config/mcp_config.json",
        ] {
            let mode = std::fs::metadata(gemini.join(file))?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file}");
        }
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            gemini.join("antigravity-cli/settings.json"),
        )?)?;
        assert_eq!(settings["modelProvider"], "gemini");
        assert_eq!(settings["trustedWorkspaces"][0], worktree.as_str());
        let hooks = std::fs::read_to_string(gemini.join("config/hooks.json"))?;
        assert!(hooks.contains("hook antigravity Stop"), "{hooks}");
        assert!(!hooks.contains(API_KEY) && !settings.to_string().contains(API_KEY));
        for shared in ["brain", "conversations"] {
            assert_eq!(
                std::fs::read_link(gemini.join("antigravity-cli").join(shared))?,
                sandbox_dir.join("antigravity/shared").join(shared)
            );
        }

        // The conversation is the one its SessionStart named.
        let session = attempt.id.as_str().to_owned();
        eventually(
            "the conversation learnt from SessionStart",
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
        app.send_input(&id, b"hello antigravity\r").await?;
        eventually("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello antigravity")
        })
        .await?;
        let transcript = sandbox_dir
            .join("antigravity/shared/brain")
            .join(&session)
            .join(".system_generated/logs/transcript.jsonl");
        eventually("the transcript's turn", Duration::from_secs(10), || async {
            std::fs::read_to_string(&transcript).is_ok_and(|t| t.contains("PLANNER_RESPONSE"))
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-604. A conversation opened with no prompt emits no new Stop hook.
#[test]
fn a_bare_resume_under_the_neon_banner_can_close() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let fixture = include_str!("fixtures/antigravity/gemini-neon-idle.txt");
    std::fs::write(data.join("screen.txt"), fixture)?;
    std::os::unix::fs::symlink(fake, data.join("fake-agent"))?;
    let cli = data.join("agy");
    std::fs::write(
        &cli,
        r#"#!/bin/sh
cat "${0%/*}/screen.txt"
exec "${0%/*}/fake-agent" "$@"
"#,
    )?;
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700))?;
    runtime()?.block_on(async {
        use yulius_domain::task::Activity;

        let release = data.join("release-turn");
        std::fs::write(&release, b"")?;
        let options = FakeAgentOptions {
            answers_first_message: true,
            resume_prompts: true,
            turn_hold_file: Some(release.clone()),
            ..crate::support::live::first_message::hooked()
        };
        let (app, _) = crate::support::live::first_message::start_with(
            &data,
            &cli,
            options,
            0,
            Arc::default(),
            |config| config.park_after_ms = 1,
        )
        .await?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Neon", TaskMode::Prompt).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        eventually("first turn settled", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "ECHO: Neon: answer with an echo").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(Activity::Waiting))
        })
        .await?;
        app.close_task_and_wait(&id).await?;
        app.continue_task(&ctx, id.as_str(), &Default::default())
            .await?;
        eventually(
            "resumed conversation screen",
            Duration::from_secs(30),
            || screen_contains(&app, &id, fixture.trim_end()),
        )
        .await?;
        assert_eq!(task(&app, &id).await?.activity, Some(Activity::Waiting));
        eventually(
            "idle conversation parked",
            Duration::from_secs(30),
            || async {
                app.park_idle_sessions_now().await;
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.status == TaskStatus::Parked)
                    && matches!(app.deps.store.live_attempt(&id).await, Ok(None))
            },
        )
        .await?;
        std::fs::remove_file(&release)?;
        app.continue_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask {
                prompt: Some("Next turn".into()),
                ..Default::default()
            },
        )
        .await?;
        assert_eq!(task(&app, &id).await?.activity, Some(Activity::Working));
        assert_eq!(app.park_idle_sessions_now().await, 0);
        std::fs::write(&release, b"")?;
        eventually(
            "new turn answered and settled",
            Duration::from_secs(30),
            || async {
                screen_contains(&app, &id, "ECHO: Next turn").await
                    && task(&app, &id)
                        .await
                        .is_ok_and(|t| t.activity == Some(Activity::Waiting))
            },
        )
        .await?;
        app.close_task_for_person(&ctx, id.as_str()).await?;
        eventually(
            "closed with its session released",
            Duration::from_secs(30),
            || async {
                task(&app, &id)
                    .await
                    .is_ok_and(|t| t.status == TaskStatus::Closed)
                    && matches!(app.deps.store.live_attempt(&id).await, Ok(None))
            },
        )
        .await?;
        Ok(())
    })
}

/// A plan's quota in the transcript is a limit of the
/// subscription: the task resumes the same conversation on another Google account (asking agy
/// to go on), the first account is limited, and the Claude account is untouched.
#[test]
fn an_antigravity_task_at_its_limit_moves_to_another_google_account() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        let one = add_google_account(&app, "a-google-1", &api_key()?).await?;
        let two = add_google_account(&app, "a-google-2", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity rotation",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        app.send_input(&id, b"again\r").await?;
        eventually(
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
        assert_ne!(
            first.account_id, second.account_id,
            "another Google account"
        );
        assert_eq!(
            second.harness,
            Harness::Antigravity,
            "the task keeps its harness"
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert!(first.provider_session_id.is_some());
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same conversation"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("Resume {") && launch.contains("continue_interrupted: true"),
            "{launch}"
        );
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

/// SPEC-505 AGY-009, AC-2, with SPEC-447: an Antigravity task moves
/// only between Antigravity subscriptions. A person's move to the Claude subscription is refused
/// naming why; removing its subscription moves it to the other Google one in the same
/// conversation, never to the Claude one that has room; removing that one too leaves it waiting,
/// asking for an Antigravity subscription. Its notices never speak of Claude.
#[test]
fn an_antigravity_task_moves_only_between_antigravity_subscriptions() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        add_google_account(&app, "a-google-1", &api_key()?).await?;
        add_google_account(&app, "a-google-2", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity moves", TaskMode::Prompt).await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        eventually("the conversation", Duration::from_secs(30), || async {
            app.deps
                .store
                .attempts(&id)
                .await
                .is_ok_and(|a| a.first().is_some_and(|a| a.provider_session_id.is_some()))
        })
        .await?;
        let first = app.deps.store.attempts(&id).await?;
        let on = first[0].account_id.clone().ok_or("the first account")?;
        let other = if on.as_str() == "a-google-1" {
            "a-google-2"
        } else {
            "a-google-1"
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
                assert!(
                    text.contains("is not an Antigravity subscription"),
                    "{text}"
                );
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
        eventually(
            "the move to the other Antigravity subscription",
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
            Harness::Antigravity,
            "the task keeps its harness"
        );
        assert_eq!(
            attempts[1].provider_session_id,
            attempts[0].provider_session_id
        );
        eventually("the removal's notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("was removed: this session continues on Antigravity"));
            async move { done }
        })
        .await?;

        app.delete_account(&ctx, other).await?;
        eventually(
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
            request.action.contains("Antigravity subscription"),
            "{}",
            request.action
        );
        eventually("the stranded notice", Duration::from_secs(10), || {
            said.extend(notices(&mut watch));
            let done = said
                .iter()
                .any(|n| n.contains("No Antigravity subscription is left"));
            async move { done }
        })
        .await?;
        assert!(
            !said.iter().any(|n| n.contains("Claude")),
            "an Antigravity task's notices never speak of Claude: {said:?}"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// An Antigravity goal task gets its whole request as its first message, never a
/// `/goal` Yulius types (the CLI's own `/goal` is its own).
#[test]
fn an_antigravity_goal_task_gets_its_request_as_its_first_message() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let account = add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity goal", TaskMode::Goal).await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let run_dir = Path::new(&attempt.run_dir);
        let launch = std::fs::read_to_string(run_dir.join("fake-launch.txt"))?;
        assert!(
            launch.contains("Antigravity goal: answer with an echo"),
            "{launch}"
        );
        assert!(!launch.contains("/goal"), "{launch}");
        // No `/goal` is typed later either, and its Stop is not held as a goal's.
        for marker in ["launch-goal-pending", "typed-goal-pending"] {
            assert!(!run_dir.join(marker).exists(), "{marker}");
        }
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(
            detail.model, "gemini-4-neon",
            "Gemini 4 Neon, not the Claude default"
        );
        assert_eq!(detail.effort.as_deref(), Some("high"));
        assert!(detail.model_default);
        // The pool weighs it by the model its run was launched with, so a subscription it is at
        // the limit of is left out: one family for both.
        let record = app.deps.store.task(&id).await?.ok_or("task")?;
        let input = app.rotation_input(&record, Some(account)).await?;
        assert_eq!(input.running, Some(input.family));
        assert_eq!(input.family, yulius_domain::usage::ModelFamily::Any);
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A Google sign-in's token file is written into its home for the run;
/// when the run ends, a token file the CLI refreshed replaces the stored sign-in, and the file
/// leaves the sandbox once no run of the account is left.
#[test]
fn a_refreshed_antigravity_sign_in_is_kept_and_leaves_the_sandbox() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let stored = antigravity_sign_in_credential(b"SYNTHETIC-token-old").ok_or("envelope")?;
        let account = add_google_account(&app, "a-google", &stored).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity sign-in",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let token = Path::new(&attempt.run_dir)
            .join("antigravity-home/.gemini/antigravity-cli/antigravity-oauth-token");
        assert_eq!(std::fs::read(&token)?, b"SYNTHETIC-token-old");
        assert_eq!(
            std::fs::metadata(&token)?.permissions().mode() & 0o777,
            0o600
        );
        // The CLI refreshes its sign-in during the run.
        std::fs::write(&token, b"SYNTHETIC-token-new")?;
        app.send_input(&id, b"/exit\r").await?;
        eventually("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        let expected = antigravity_sign_in_credential(b"SYNTHETIC-token-new").ok_or("envelope")?;
        eventually("the refresh kept", Duration::from_secs(10), || async {
            let Ok(Some(record)) = app.deps.store.account(&account).await else {
                return false;
            };
            record
                .credential
                .and_then(|sealed| app.deps.crypto.open(&sealed).ok())
                .is_some_and(|plain| plain == expected)
        })
        .await?;
        eventually("the file gone", Duration::from_secs(10), || async {
            !token.exists()
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A turn whose model API fails for good writes an error step per
/// attempt (no end of the turn: agy goes on) and then its Stop with the error, which Yulius settles
/// without a gate: the task leaves `working` and waits for its person.
#[test]
fn a_failed_antigravity_turn_leaves_its_task_waiting() -> TestResult {
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity failure",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        // A code change the gate would block a clean Stop for: a failed turn's is never held.
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        std::fs::write(Path::new(&worktree).join("api.rs"), "fn main() {}\n")?;
        app.send_input(&id, b"/apierror\r").await?;
        eventually("the failed turn", Duration::from_secs(20), || {
            screen_contains(&app, &id, "API ERROR")
        })
        .await?;
        eventually("the task waiting", Duration::from_secs(20), || async {
            task(&app, &id).await.is_ok_and(|t| {
                t.status == TaskStatus::Running && t.activity == Some(Activity::Waiting)
            })
        })
        .await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 0, "no gate block");
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A run's token file replaces only the sign-in its launch gave it. A sign-in a
/// person made during the run, or another run's newer refresh, stays: the run's refreshed file
/// is dropped, and an unchanged one changes nothing.
#[test]
fn a_run_never_replaces_a_newer_antigravity_sign_in() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let given = antigravity_sign_in_credential(b"SYNTHETIC-token-given").ok_or("envelope")?;
        let account = add_google_account(&app, "a-google", &given).await?;
        seed_repo(&data, &sandbox)?;
        let newer = antigravity_sign_in_credential(b"SYNTHETIC-token-newer").ok_or("envelope")?;
        for (title, run_refreshes) in [("refreshed", true), ("unchanged", false)] {
            // The vault holds what the launch gives.
            let previous = app
                .deps
                .store
                .account(&account)
                .await?
                .ok_or("account")?
                .credential
                .ok_or("credential")?;
            app.deps
                .store
                .refresh_account_credential(
                    &account,
                    &previous,
                    &app.deps.crypto.seal(&given)?,
                    app.now(),
                )
                .await?;
            let id = new_agy_task(&app, &sandbox, &user, title, TaskMode::Prompt).await?;
            eventually("the banner", Duration::from_secs(30), || {
                screen_contains(&app, &id, "FAKE-AGENT READY")
            })
            .await?;
            let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
            let token = Path::new(&attempt.run_dir)
                .join("antigravity-home/.gemini/antigravity-cli/antigravity-oauth-token");
            // A person signs the subscription in again (or another run stores its refresh).
            let launched = app
                .deps
                .store
                .account(&account)
                .await?
                .ok_or("account")?
                .credential
                .ok_or("credential")?;
            app.deps
                .store
                .refresh_account_credential(
                    &account,
                    &launched,
                    &app.deps.crypto.seal(&newer)?,
                    app.now(),
                )
                .await?;
            if run_refreshes {
                std::fs::write(&token, b"SYNTHETIC-token-run")?;
            }
            app.send_input(&id, b"/exit\r").await?;
            eventually("the run's end", Duration::from_secs(30), || async {
                task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
            })
            .await?;
            eventually("the run's file gone", Duration::from_secs(10), || async {
                !token.exists()
            })
            .await?;
            let held = app
                .deps
                .store
                .account(&account)
                .await?
                .and_then(|a| a.credential)
                .and_then(|sealed| app.deps.crypto.open(&sealed).ok())
                .ok_or("held")?;
            assert_eq!(held, newer, "{title}: the newer sign-in stays");
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A run whose SessionStart named nothing (no hooks here) takes the CLI's last
/// conversation of its worktree from its home's `last_conversations.json`, once its transcript
/// was written since the run began.
#[test]
fn an_antigravity_conversation_is_found_without_its_session_start() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity found", TaskMode::Prompt).await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        app.send_input(&id, b"hello\r").await?;
        eventually("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello")
        })
        .await?;
        assert_eq!(attempt.provider_session_id, None, "no hook named it");
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        let cache =
            Path::new(&attempt.run_dir).join("antigravity-home/.gemini/antigravity-cli/cache");
        std::fs::write(
            cache.join("last_conversations.json"),
            serde_json::json!({ worktree: attempt.id.as_str() }).to_string(),
        )?;
        eventually(
            "the conversation found",
            Duration::from_secs(60),
            || async {
                app.deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|a| a.provider_session_id)
                    .as_deref()
                    == Some(attempt.id.as_str())
            },
        )
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The sandbox's shared conversations folder is opened without following a
/// link: one an agent put there fails the start, and nothing is spawned.
#[test]
fn a_linked_antigravity_folder_fails_the_start() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let agy = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("antigravity");
        std::fs::create_dir_all(&agy)?;
        let elsewhere = data.join("elsewhere");
        std::fs::create_dir_all(&elsewhere)?;
        std::os::unix::fs::symlink(&elsewhere, agy.join("shared"))?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity linked",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the failed start", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| matches!(t.status, TaskStatus::Failed(_)))
        })
        .await?;
        assert!(
            app.deps.store.attempts(&id).await?.is_empty(),
            "nothing spawned"
        );
        assert!(
            std::fs::read_dir(&elsewhere)?.next().is_none(),
            "nothing made through the link"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A contained run sees its run's `.gemini` (the hooks Yulius wrote) at the
/// sandbox home's, and the data directory not at all. Skipped where bwrap cannot create
/// namespaces.
#[test]
fn a_contained_antigravity_run_sees_its_home_at_the_sandbox_homes() -> TestResult {
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
            "SKIPPED a_contained_antigravity_run_sees_its_home_at_the_sandbox_homes: bwrap cannot create namespaces"
        );
        return Ok(());
    }
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app =
            Live::new(Some(&fake), FakeAgentOptions::default()).app(&data, UNSERVED, move |c| {
                c.contained = true;
                c.contained_readonly = vec![fake_dir];
            })?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity contained",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let home = data.join("sandboxes").join(sandbox.as_str()).join("home");
        for (path, expected) in [
            (home.join(".gemini/config/hooks.json"), "READ hooks.json OK"),
            (data.join("yulius.db"), "READ yulius.db ERR"),
        ] {
            app.send_input(&id, format!("/read {}\r", path.display()).as_bytes())
                .await?;
            eventually(expected, Duration::from_secs(10), || {
                screen_contains(&app, &id, expected)
            })
            .await?;
        }
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The plan of a contained Antigravity run binds its run's own `.gemini`, in the
/// server's run directory, over the sandbox home's, writable, and sets no `HOME` of its own (the
/// agent's shell keeps the sandbox's); its provided CLI's folder is bound read-only. Runs
/// everywhere: the session host records the plan and starts nothing.
#[test]
fn a_contained_antigravity_plan_binds_the_runs_home() -> TestResult {
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(crate::support::ScriptedHost::default());
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let cli = data.join("antigravity-cli/1.5.12-linux-x64");

        let app = Live {
            host: Some(host.clone()),
            ..Live::new(
                Some(&exe),
                FakeAgentOptions {
                    provided_antigravity: Some(yulius_app::ports::harness::ProvidedCli {
                        program: cli.join("antigravity"),
                        dir: cli.clone(),
                    }),
                    ..FakeAgentOptions::default()
                },
            )
        }
        .app(&data, UNSERVED, |config| config.contained = true)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity planned",
            TaskMode::Prompt,
        )
        .await?;
        eventually("the recorded plan", Duration::from_secs(30), || async {
            !host.plans().is_empty()
        })
        .await?;
        let plan = host.plans().into_iter().next().ok_or("no plan")?;
        let yulius_app::ports::session::Containment::Bubblewrap {
            sandbox_root,
            home,
            readonly,
            binds,
            ..
        } = &plan.containment
        else {
            return Err("not contained".into());
        };
        let gemini: Vec<_> = binds
            .iter()
            .filter(|bind| bind.dest == home.join(".gemini"))
            .collect();
        match gemini.as_slice() {
            [bind] => {
                assert!(bind.writable);
                assert!(bind.source.starts_with(data.join("runs")), "{bind:?}");
                assert!(
                    bind.source.ends_with("antigravity-home/.gemini"),
                    "{bind:?}"
                );
                assert!(!bind.source.starts_with(sandbox_root), "{bind:?}");
                assert!(bind.source.join("config/hooks.json").is_file());
            }
            other => return Err(format!("one bind of .gemini: {other:?}").into()),
        }
        assert!(readonly.contains(&cli), "{readonly:?}");
        assert!(
            !plan
                .env
                .iter()
                .any(|(k, v)| k == "HOME" && v.contains("antigravity-home")),
            "{:?}",
            plan.env
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
