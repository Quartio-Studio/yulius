//! Sessions record the Claude Code that runs them, and a running task moves to a newer one at
//! its turn boundary in the same session (SPEC-461 CCM-011..CCM-013), with real processes as in
//! `pool_supervision.rs`: the fake agent's transcript names the version a file holds when it
//! starts, so rewriting the file stands for a newer Claude Code at the session's next start.

use crate::pool_supervision::{
    ScriptedUsage, build_with, new_task, runtime, screen, seed, stop, temp, wait_for,
};
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_domain::harness::DEFAULT_CLAUDE_MODEL;
use yulius_domain::ids::{TaskId, UserId};
use yulius_ptyhost::test_support::locate_fake_agent;

async fn ctx(app: &App, user: &UserId) -> Result<AuthContext, Box<dyn std::error::Error>> {
    Ok(AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

async fn ready(app: &Arc<App>, id: &TaskId) -> TestResult {
    wait_for("the run", Duration::from_secs(30), || {
        let app = app.clone();
        let id = id.clone();
        async move { screen(&app, &id).contains("FAKE-AGENT READY") }
    })
    .await
}

/// A fake agent whose transcript names the Claude Code version in `file`.
fn agent(file: &Path) -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        cli_version_file: Some(file.to_path_buf()),
        ..FakeAgentOptions::default()
    }
}

/// The Claude Code version each run of `id` recorded, oldest first.
async fn run_versions(app: &App, id: &TaskId) -> Vec<Option<String>> {
    let facts = app.deps.store.task_run_facts(id).await.unwrap_or_default();
    app.deps
        .store
        .attempts(id)
        .await
        .unwrap_or_default()
        .iter()
        .map(|a| {
            facts
                .iter()
                .find(|f| f.attempt == a.id)
                .and_then(|f| f.cli_version.clone())
        })
        .collect()
}

/// AC8 (CCM-011, CCM-012): a session records the Claude Code version its transcript names, and
/// the task's detail shows it with the model the task asks for and the model that answered.
#[test]
fn a_session_records_its_claude_code_and_the_task_shows_it() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let version = data.join("claude-version");
    std::fs::write(&version, "2.1.311\n")?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build_with(&data, &fake, probe, agent(&version))?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let ctx = ctx(&app, &user).await?;
        let id = new_task(&app, &sandbox, &user, "Versions").await?;
        ready(&app, &id).await?;
        // Before any line names it, nothing is claimed.
        let before = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(before.cli_version, None);
        assert_eq!(before.model, DEFAULT_CLAUDE_MODEL);
        assert!(before.model_default, "the task names no model");
        assert_eq!(before.effort.as_deref(), Some("max"));
        assert_eq!(before.cli_moves_to, None);

        app.send_input(&id, b"first turn\r").await?;
        wait_for("the version", Duration::from_secs(30), || async {
            run_versions(&app, &id).await == [Some("2.1.311".to_owned())]
        })
        .await?;
        wait_for("the answering model", Duration::from_secs(60), || async {
            app.task_detail(&ctx, id.as_str())
                .await
                .is_ok_and(|d| d.attempts[0].model.is_some())
        })
        .await?;
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(detail.cli_version.as_deref(), Some("2.1.311"));
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(detail.attempts[0].cli_version.as_deref(), Some("2.1.311"));
        // What answered is the transcript's word, not what was asked for.
        assert_eq!(detail.attempts[0].model.as_deref(), Some("fake-model"));
        assert_eq!(detail.metrics.models, ["fake-model"]);
        assert_eq!(detail.model, DEFAULT_CLAUDE_MODEL);
        // No adapter says what new runs get: nothing is promised.
        assert_eq!(detail.cli_moves_to, None);
        // It is among the live runs that name a version (the Instance page's list reads these).
        let live = app.deps.store.live_cli_versions().await?;
        assert_eq!(live, [(id.clone(), "2.1.311".to_owned())]);

        // The stopped run keeps its version; nothing live is left.
        stop(&app, &id).await?;
        assert_eq!(run_versions(&app, &id).await, [Some("2.1.311".to_owned())]);
        assert!(app.deps.store.live_cli_versions().await?.is_empty());
        let rested = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(rested.cli_version.as_deref(), Some("2.1.311"));
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

// --- CCM-013: the move, with the fake agent running Claude Code's hooks against a served app.

mod moves {
    use super::{TestResult, run_versions};
    use crate::first_message::{
        data_dir, hooked, new_task, runtime, screen, start, stop, wait_for,
    };
    use crate::pool_supervision::{ScriptedUsage, seed};
    use crate::support::agents::FakeAgentOptions;

    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use yulius_app::auth::AuthContext;
    use yulius_app::context::App;
    use yulius_app::hub::HubFrame;
    use yulius_app::model::{LaunchKind, TaskMode};
    use yulius_app::ports::agent_cli::{AgentCli, AgentCliStatus, CliSource};
    use yulius_domain::agent_cli::{Channel, Version};
    use yulius_domain::ids::{TaskId, UserId};
    use yulius_domain::task::{Activity, AttemptState, TaskStatus};
    use yulius_ptyhost::test_support::locate_fake_agent;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} is a version"))
    }

    /// An adapter that only says which Claude Code a run starting now gets.
    struct Installed(Mutex<Option<Version>>);

    impl Installed {
        fn release(&self, version: &str) {
            if let Ok(mut now) = self.0.lock() {
                *now = Some(v(version));
            }
        }
    }

    #[async_trait]
    impl AgentCli for Installed {
        async fn status(&self) -> AgentCliStatus {
            AgentCliStatus {
                unmanaged: None,
                channel: Channel::Latest,
                version: self.version_for_new_runs(),
                source: Some(CliSource::Yulius),
                host_version: None,
                missing: Some(Vec::new()),
                latest: None,
                stable: None,
                checked_at: None,
                checking: false,
                result: None,
            }
        }

        fn check_now(&self) {}

        fn set_channel(&self, _channel: Channel, _chosen: bool) {}

        fn version_for_new_runs(&self) -> Option<Version> {
            self.0.lock().ok().and_then(|version| *version)
        }
    }

    fn agent(file: &Path, more: FakeAgentOptions) -> FakeAgentOptions {
        FakeAgentOptions {
            cli_version_file: Some(file.to_path_buf()),
            ..more
        }
    }

    struct Server {
        app: Arc<App>,
        installed: Arc<Installed>,
        user: UserId,
        id: TaskId,
    }

    /// A served app whose runs get Claude Code 2.1.311, and a running task of `mode`.
    async fn server(
        data: &Path,
        version: &Path,
        options: FakeAgentOptions,
        mode: TaskMode,
    ) -> Result<Server, Box<dyn std::error::Error>> {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        std::fs::write(version, "2.1.311\n")?;
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _port) = start(data, &fake, agent(version, options), 0, probe).await?;
        let installed = Arc::new(Installed(Mutex::new(Some(v("2.1.311")))));
        assert!(app.set_agent_cli(installed.clone() as Arc<_>, Channel::Latest));
        let (sandbox, user) = seed(&app, data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Stay current", mode).await?;
        wait_for("the run", Duration::from_secs(60), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        Ok(Server {
            app,
            installed,
            user,
            id,
        })
    }

    async fn turn(s: &Server, text: &str) -> TestResult {
        let before = s.app.deps.store.attempts(&s.id).await?.len();
        s.app
            .send_input(&s.id, format!("{text}\r").as_bytes())
            .await?;
        wait_for("the turn", Duration::from_secs(60), || async {
            screen(&s.app, &s.id).contains(&format!("ECHO: {text}"))
                || s.app
                    .deps
                    .store
                    .attempts(&s.id)
                    .await
                    .unwrap_or_default()
                    .len()
                    > before
        })
        .await
    }

    async fn settled(s: &Server, activity: Activity) -> TestResult {
        let state = || async {
            let task = s.app.deps.store.task(&s.id).await.ok().flatten();
            task.map(|t| (t.status, t.activity))
        };
        let waited = wait_for("the task to settle", Duration::from_secs(30), || async {
            state().await == Some((TaskStatus::Running, Some(activity)))
        })
        .await;
        if waited.is_err() {
            return Err(format!("the task is {:?}, not {activity:?}", state().await).into());
        }
        Ok(())
    }

    async fn runs(s: &Server) -> usize {
        s.app
            .deps
            .store
            .attempts(&s.id)
            .await
            .unwrap_or_default()
            .len()
    }

    async fn second_run_live(s: &Server) -> TestResult {
        wait_for("the restarted session", Duration::from_secs(90), || async {
            let attempts = s.app.deps.store.attempts(&s.id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == AttemptState::Live
        })
        .await
    }

    fn said(watch: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(frame) = watch.try_recv() {
            if let HubFrame::Notice(text) = frame {
                out.push(text);
            }
        }
        out
    }

    /// AC9: an idle prompt-mode session on 2.1.311 is restarted once its turn ended, when runs
    /// get 2.1.322: the same Claude session and subscription, by resume, with one line in its
    /// terminal; it is not restarted again, and the task's info shows both sessions.
    #[test]
    fn a_running_task_moves_to_the_new_claude_code_when_its_turn_ends() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt).await?;
            // Runs get what the session runs: its turns end and nothing restarts.
            turn(&s, "one").await?;
            settled(&s, Activity::Waiting).await?;
            wait_for("the version", Duration::from_secs(30), || async {
                run_versions(&s.app, &s.id).await == [Some("2.1.311".to_owned())]
            })
            .await?;
            assert_eq!(runs(&s).await, 1, "nothing to move to");

            // A release is installed: new runs get 2.1.322. Nothing happens mid-idle.
            std::fs::write(&version, "2.1.322\n")?;
            s.installed.release("2.1.322");
            let ctx = AuthContext {
                user: s.app.deps.store.user(&s.user).await?.ok_or("user")?,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            let behind = s.app.claude_code_view(&ctx).await?.behind;
            assert_eq!(behind.len(), 1, "{behind:?}");
            assert_eq!((behind[0].number, behind[0].version.as_str()), (1, "2.1.311"));
            assert_eq!(behind[0].sandbox, "pool");
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert_eq!(detail.cli_moves_to.as_deref(), Some("2.1.322"));
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert_eq!(runs(&s).await, 1, "an idle session is left alone");

            // Its next turn ends: the session restarts on the new Claude Code.
            let mut watch = s.app.runtime(&s.id).hub.subscribe().rx;
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            let attempts = s.app.deps.store.attempts(&s.id).await?;
            assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
            assert_eq!(
                attempts[0].provider_session_id, attempts[1].provider_session_id,
                "the same Claude session"
            );
            assert_eq!(attempts[0].account_id, attempts[1].account_id);
            settled(&s, Activity::Waiting).await?;
            let notices = said(&mut watch);
            assert!(
                notices.iter().any(|n| n
                    == "↻ Claude Code 2.1.322 is installed: this session restarts on it and continues…"),
                "{notices:?}"
            );
            assert!(
                !notices.iter().any(|n| n.contains("subscription")),
                "{notices:?}"
            );

            // The restarted session names the new version and is not restarted again.
            turn(&s, "three").await?;
            wait_for("the new version", Duration::from_secs(30), || async {
                run_versions(&s.app, &s.id).await
                    == [Some("2.1.311".to_owned()), Some("2.1.322".to_owned())]
            })
            .await?;
            settled(&s, Activity::Waiting).await?;
            tokio::time::sleep(Duration::from_millis(800)).await;
            assert_eq!(runs(&s).await, 2, "moved once");
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert_eq!(detail.cli_version.as_deref(), Some("2.1.322"));
            assert_eq!(detail.cli_moves_to, None);
            assert!(detail.switches.is_empty(), "no subscription switch: {:?}", detail.switches);
            assert!(s.app.claude_code_view(&ctx).await?.behind.is_empty());
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// Met on the way (SPEC-447 MOV-002, "a moved idle task gets nothing typed"): a prompt-mode
    /// session that was idle is idle again after a move that types nothing, also a person's move
    /// to another subscription. The launch used to leave it "working" until its next turn ended.
    #[test]
    fn an_idle_session_is_idle_again_after_a_person_moves_it() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt).await?;
            crate::pool_supervision::add_account(&s.app, "b-second").await?;
            turn(&s, "one").await?;
            settled(&s, Activity::Waiting).await?;
            let ctx = AuthContext {
                user: s.app.deps.store.user(&s.user).await?.ok_or("user")?,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            s.app
                .switch_task(
                    &ctx,
                    s.id.as_str(),
                    &yulius_contracts::api::tasks::ContinueTask {
                        account_id: Some("b-second".into()),
                    },
                )
                .await?;
            second_run_live(&s).await?;
            settled(&s, Activity::Waiting).await?;
            // The next turn works and rests as before.
            turn(&s, "two").await?;
            settled(&s, Activity::Waiting).await?;
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// AC9: a session a person typed into in the last minute is not restarted under them, and
    /// one that runs the version runs get is left alone at every turn's end.
    #[test]
    fn a_session_someone_types_into_is_not_restarted() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt).await?;
            turn(&s, "one").await?;
            settled(&s, Activity::Waiting).await?;
            std::fs::write(&version, "2.1.322\n")?;
            s.installed.release("2.1.322");
            // A person is typing a draft while the agent answers.
            s.app.record_typing(&s.id, &s.user);
            turn(&s, "two").await?;
            settled(&s, Activity::Waiting).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert_eq!(runs(&s).await, 1, "their draft is not lost");
            let facts = run_versions(&s.app, &s.id).await;
            assert_eq!(facts, [Some("2.1.311".to_owned())]);
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// AC9 (goal mode): a goal session moves at the Stop of a turn it works on after, and its
    /// goal is set again in the restarted session, which then ends the task as achieved.
    #[test]
    fn a_goal_session_moves_at_its_stop_and_gets_its_goal_again() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                // Time to install the release before the goal turn ends.
                goal_delay_ms: Some(3_000),
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Goal).await?;
            std::fs::write(&version, "2.1.322\n")?;
            s.installed.release("2.1.322");
            second_run_live(&s).await?;
            let attempts = s.app.deps.store.attempts(&s.id).await?;
            assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
            assert_eq!(
                attempts[0].provider_session_id,
                attempts[1].provider_session_id
            );
            // The goal is set again in the restarted session, and achieved there.
            wait_for("the goal again", Duration::from_secs(90), || async {
                screen(&s.app, &s.id).contains("GOAL SET")
            })
            .await?;
            settled(&s, Activity::Done).await?;
            assert_eq!(runs(&s).await, 2, "moved once");
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                [Some("2.1.311".to_owned()), Some("2.1.322".to_owned())]
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }
}
