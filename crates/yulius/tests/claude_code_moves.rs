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
use yulius_domain::harness::{DEFAULT_CLAUDE_EFFORT, DEFAULT_CLAUDE_MODEL};
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
        assert_eq!(before.effort.as_deref(), Some(DEFAULT_CLAUDE_EFFORT));
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
        data_dir, hooked, new_task, runtime, screen, start_with, stop, wait_for,
    };
    use crate::pool_supervision::{HOUR, ScriptedUsage, refresh, seed, usage};
    use crate::support::agents::FakeAgentOptions;

    use std::os::unix::fs::OpenOptionsExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use yulius_app::auth::AuthContext;
    use yulius_app::context::{App, AppConfig};
    use yulius_app::hub::HubFrame;
    use yulius_app::model::{LaunchKind, TaskMode};
    use yulius_app::ports::agent_cli::{AgentCli, AgentCliStatus, CliSource};
    use yulius_app::race::{self, Point};
    use yulius_domain::agent_cli::{Channel, Version};
    use yulius_domain::ids::{SandboxId, TaskId, UserId};
    use yulius_domain::task::{Activity, AttemptState, StopReason, TaskStatus};
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

    struct Server {
        app: Arc<App>,
        installed: Arc<Installed>,
        probe: Arc<ScriptedUsage>,
        sandbox: SandboxId,
        user: UserId,
        id: TaskId,
    }

    impl Server {
        async fn ctx(&self) -> Result<AuthContext, Box<dyn std::error::Error>> {
            Ok(AuthContext {
                user: self.app.deps.store.user(&self.user).await?.ok_or("user")?,
                csrf_token: None,
                session_hash: None,
                token: None,
            })
        }
    }

    async fn ready(app: &Arc<App>, id: &TaskId) -> TestResult {
        wait_for("the run", Duration::from_secs(60), || async {
            screen(app, id).contains("FAKE-AGENT READY")
        })
        .await
    }

    /// A served app whose sessions start on Claude Code 2.1.311 (the version file) while runs
    /// get `gets`, and a running task of `mode`.
    async fn server(
        data: &Path,
        version: &Path,
        options: FakeAgentOptions,
        mode: TaskMode,
        gets: &str,
    ) -> Result<Server, Box<dyn std::error::Error>> {
        server_with(data, version, options, mode, gets, |_| {}).await
    }

    /// [`server`] whose configuration `configure` changes.
    async fn server_with(
        data: &Path,
        version: &Path,
        options: FakeAgentOptions,
        mode: TaskMode,
        gets: &str,
        configure: impl FnOnce(&mut AppConfig),
    ) -> Result<Server, Box<dyn std::error::Error>> {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        std::fs::write(version, "2.1.311\n")?;
        let probe = Arc::new(ScriptedUsage::default());
        let options = FakeAgentOptions {
            cli_version_file: Some(version.to_path_buf()),
            ..options
        };
        let (app, _port) = start_with(data, &fake, options, 0, probe.clone(), configure).await?;
        let installed = Arc::new(Installed(Mutex::new(Some(v(gets)))));
        assert!(app.set_agent_cli(installed.clone() as Arc<_>, Channel::Latest));
        let (sandbox, user) = seed(&app, data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Stay current", mode).await?;
        ready(&app, &id).await?;
        Ok(Server {
            app,
            installed,
            probe,
            sandbox,
            user,
            id,
        })
    }

    /// A FIFO whose reader stays until it is let go; a test that fails lets it go too.
    struct Held(PathBuf);

    impl Held {
        fn at(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
            nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU)?;
            Ok(Self(path))
        }

        /// A writer that comes and goes: the reader reads the end and exits. `false` while
        /// nothing reads the FIFO.
        fn end_reader(&self) -> bool {
            std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
                .open(&self.0)
                .is_ok()
        }

        /// Ends the process that reads it.
        async fn let_go(&self) -> TestResult {
            wait_for("the reader", Duration::from_secs(30), || async {
                self.end_reader()
            })
            .await
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            self.end_reader();
        }
    }

    /// Waits until the session's terminal named `count` background shells.
    async fn backgrounds(s: &Server, count: usize) -> TestResult {
        wait_for("the background shell", Duration::from_secs(30), || async {
            screen(&s.app, &s.id).matches("BACKGROUND ").count() == count
        })
        .await
    }

    /// A flag set the next time the server reaches `point` for the task `id`.
    fn reached(point: Point, id: &TaskId) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let set = Arc::clone(&flag);
        race::on(point, Path::new(id.as_str()), move || {
            set.store(true, Ordering::SeqCst);
        });
        flag
    }

    /// A release is installed: runs get `version`, and a session started from now names it.
    fn release(s: &Server, file: &Path, version: &str) -> TestResult {
        std::fs::write(file, format!("{version}\n"))?;
        s.installed.release(version);
        Ok(())
    }

    async fn turn_of(app: &Arc<App>, id: &TaskId, text: &str) -> TestResult {
        let before = app.deps.store.attempts(id).await?.len();
        app.send_input(id, format!("{text}\r").as_bytes()).await?;
        wait_for("the turn", Duration::from_secs(60), || async {
            screen(app, id).contains(&format!("ECHO: {text}"))
                || app.deps.store.attempts(id).await.unwrap_or_default().len() > before
        })
        .await
    }

    async fn turn(s: &Server, text: &str) -> TestResult {
        turn_of(&s.app, &s.id, text).await
    }

    async fn state(app: &App, id: &TaskId) -> Option<(TaskStatus, Option<Activity>)> {
        let task = app.deps.store.task(id).await.ok().flatten();
        task.map(|t| (t.status, t.activity))
    }

    async fn settled(s: &Server, activity: Activity) -> TestResult {
        let waited = wait_for("the task to settle", Duration::from_secs(30), || async {
            state(&s.app, &s.id).await == Some((TaskStatus::Running, Some(activity)))
        })
        .await;
        if waited.is_err() {
            let now = state(&s.app, &s.id).await;
            return Err(format!("the task is {now:?}, not {activity:?}").into());
        }
        Ok(())
    }

    async fn runs_of(app: &App, id: &TaskId) -> usize {
        app.deps.store.attempts(id).await.unwrap_or_default().len()
    }

    async fn runs(s: &Server) -> usize {
        runs_of(&s.app, &s.id).await
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

    /// Set once the next settled turn of `id` decided whether its session moves (and started
    /// the restart if so): what a test waits for instead of a pause.
    fn decided(id: &TaskId) -> Arc<AtomicBool> {
        reached(Point::ClaudeMoveSettled, id)
    }

    async fn until_decided(flag: &AtomicBool) -> TestResult {
        wait_for(
            "the turn's end to decide",
            Duration::from_secs(60),
            || async { flag.load(Ordering::SeqCst) },
        )
        .await
    }

    /// A turn of the session ends and decides to leave it where it is.
    async fn turn_that_stays(s: &Server, text: &str) -> TestResult {
        let flag = decided(&s.id);
        turn(s, text).await?;
        until_decided(&flag).await?;
        assert_eq!(runs(s).await, 1, "the session was restarted after {text:?}");
        Ok(())
    }

    /// AC9: an idle prompt-mode session on 2.1.311 is restarted once its turn ended, when runs
    /// get 2.1.322: the same Claude session and subscription, by resume, with one line in its
    /// terminal; it is idle again, names the new version at once, is not restarted again, and
    /// the task's info shows both sessions and no subscription switch.
    #[test]
    fn a_running_task_moves_to_the_new_claude_code_when_its_turn_ends() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt, "2.1.311").await?;
            // Runs get what the session runs: its turn ends and nothing restarts.
            turn_that_stays(&s, "one").await?;
            settled(&s, Activity::Waiting).await?;
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                [Some("2.1.311".to_owned())]
            );

            // A release is installed: new runs get 2.1.322, and the task is behind.
            release(&s, &version, "2.1.322")?;
            let ctx = s.ctx().await?;
            let behind = s.app.claude_code_view(&ctx).await?.behind;
            assert_eq!(behind.len(), 1, "{behind:?}");
            assert_eq!(
                (behind[0].number, behind[0].version.as_str()),
                (1, "2.1.311")
            );
            assert_eq!(behind[0].sandbox, "pool");
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert_eq!(detail.cli_moves_to.as_deref(), Some("2.1.322"));
            assert_eq!(runs(&s).await, 1, "an idle session is left alone");

            // Its next turn ends: the session restarts on the new Claude Code.
            let mut watch = s.app.runtime(&s.id).hub.subscribe().rx;
            let restarted = decided(&s.id);
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            // The restart is over, with all it says.
            until_decided(&restarted).await?;
            let attempts = s.app.deps.store.attempts(&s.id).await?;
            assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
            assert_eq!(
                attempts[0].provider_session_id, attempts[1].provider_session_id,
                "the same Claude session"
            );
            assert_eq!(attempts[0].account_id, attempts[1].account_id);
            settled(&s, Activity::Waiting).await?;
            // One line, about Claude Code, and none about subscriptions.
            let notices = said(&mut watch);
            assert_eq!(
                notices,
                ["↻ Claude Code 2.1.322 is installed: this session restarts on it and continues…"]
            );
            // The restarted session counts as on the new version before its first prompt.
            wait_for(
                "the new session's version",
                Duration::from_secs(30),
                || async {
                    run_versions(&s.app, &s.id).await
                        == [Some("2.1.311".to_owned()), Some("2.1.322".to_owned())]
                },
            )
            .await?;
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert_eq!(detail.cli_version.as_deref(), Some("2.1.322"));
            assert_eq!(detail.cli_moves_to, None);
            assert!(s.app.claude_code_view(&ctx).await?.behind.is_empty());

            // Its next turn ends there, and it is not restarted again.
            let flag = decided(&s.id);
            turn(&s, "three").await?;
            until_decided(&flag).await?;
            settled(&s, Activity::Waiting).await?;
            assert_eq!(runs(&s).await, 2, "moved once");
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert_eq!(detail.cli_version.as_deref(), Some("2.1.322"));
            assert!(
                detail.switches.is_empty(),
                "no subscription switch: {:?}",
                detail.switches
            );
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
            let s = server(&data, &version, hooked(), TaskMode::Prompt, "2.1.311").await?;
            crate::pool_supervision::add_account(&s.app, "b-second").await?;
            turn(&s, "one").await?;
            settled(&s, Activity::Waiting).await?;
            let ctx = s.ctx().await?;
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

    /// AC9: a session a person typed into in the last minute is not restarted under them, also
    /// when they start typing while the turn's end looks at the session's background work.
    #[test]
    fn a_session_someone_types_into_is_not_restarted() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // The draft starts after the turn's end found nothing in the background.
            let (app, id, user) = (s.app.clone(), s.id.clone(), s.user.clone());
            let probed = Arc::new(AtomicBool::new(false));
            let seen = Arc::clone(&probed);
            race::on(
                Point::ClaudeMoveProbed,
                Path::new(s.id.as_str()),
                move || {
                    app.record_typing(&id, &user);
                    seen.store(true, Ordering::SeqCst);
                },
            );
            turn_that_stays(&s, "two").await?;
            assert!(
                probed.load(Ordering::SeqCst),
                "the move was due until they typed"
            );
            // A draft typed before the turn ended keeps the session too.
            turn_that_stays(&s, "three").await?;
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                [Some("2.1.311".to_owned())]
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// AC9 (review round 1, finding 1): a session with a background shell is left alone, and so
    /// is one whose only background shell runs `yulius wait`: restarted, it would never hear
    /// what it waits for.
    #[test]
    fn a_session_with_background_work_or_a_yulius_wait_is_not_restarted() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        // A stand-in for `yulius wait`: a process named `yulius` whose first argument is `wait`,
        // reading a FIFO until the test lets it go; and a build, reading another.
        let waiting = data.join("waiting");
        std::fs::create_dir_all(&waiting)?;
        std::fs::copy("/bin/cat", waiting.join("yulius"))?;
        let wait = Held::at(waiting.join("wait"))?;
        let build = Held::at(data.join("building"))?;
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;

            // The agent waits for Yulius's news in the background and ends its turn.
            let waits = format!("/bg cd {} && ./yulius wait\r", waiting.display());
            s.app.send_input(&s.id, waits.as_bytes()).await?;
            backgrounds(&s, 1).await?;
            turn_that_stays(&s, "two").await?;

            // A build runs in the background; the wait ends.
            let builds = format!("/bg cat {}\r", build.0.display());
            s.app.send_input(&s.id, builds.as_bytes()).await?;
            backgrounds(&s, 2).await?;
            wait.let_go().await?;
            turn_that_stays(&s, "three").await?;
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                [Some("2.1.311".to_owned())]
            );
            build.let_go().await?;
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
            // A turn ends when this file exists.
            let may_end = data.join("turn-may-end");
            let options = FakeAgentOptions {
                turn_hold_file: Some(may_end.clone()),
                ..hooked()
            };
            // Runs get 2.1.322 from the start; the first session started on 2.1.311.
            let s = server(&data, &version, options, TaskMode::Goal, "2.1.322").await?;
            std::fs::write(&version, "2.1.322\n")?;
            // The goal's turn ends once Yulius read that the goal is set: the session works on
            // after that turn.
            wait_for("the goal to be read", Duration::from_secs(60), || async {
                s.app.goal_is_open(&s.id)
            })
            .await?;
            assert_eq!(runs(&s).await, 1);
            std::fs::write(&may_end, "")?;
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

    /// Review round 1, finding 7: a goal task that is Done is not restarted at the end of a
    /// follow-up turn: its goal would be typed again, which is work nobody asked for.
    #[test]
    fn a_done_goal_task_keeps_its_session_after_a_follow_up() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Goal, "2.1.311").await?;
            settled(&s, Activity::Done).await?;
            release(&s, &version, "2.1.322")?;
            // The follow-up's own Stop: a move at it would hold the hook before this point, and
            // the next clean Stop would be the restarted session's.
            let stopped = Arc::new(AtomicBool::new(false));
            let set = Arc::clone(&stopped);
            race::on(Point::StopClean, Path::new(s.id.as_str()), move || {
                set.store(true, Ordering::SeqCst);
            });
            turn(&s, "one more thing").await?;
            until_decided(&stopped).await?;
            assert_eq!(runs(&s).await, 1, "the goal is not set again");
            let goals = screen(&s.app, &s.id).matches("GOAL SET").count();
            assert_eq!(goals, 1, "{}", screen(&s.app, &s.id));
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// A prompt-mode session on 2.1.311 while runs get 2.1.322, whose turns end when `may_end`
    /// exists and whose goal evaluator gives no verdict; with `code` changed in its worktree
    /// and no spec, so the gate blocks its turn's end.
    async fn session_with_own_goal(
        data: &Path,
        version: &Path,
        may_end: &Path,
        code: bool,
    ) -> Result<Server, Box<dyn std::error::Error>> {
        let options = FakeAgentOptions {
            turn_hold_file: Some(may_end.to_path_buf()),
            goal_verdict: Some("none"),
            ..hooked()
        };
        let s = server(data, version, options, TaskMode::Prompt, "2.1.322").await?;
        std::fs::write(version, "2.1.322\n")?;
        if code {
            change_code(&s).await?;
        }
        // Its person sets a goal of their own in the terminal; the goal's turn ends once
        // Yulius read that the goal is set.
        s.app
            .send_input(&s.id, b"/goal keep the header blue\r")
            .await?;
        wait_for("the goal to be read", Duration::from_secs(60), || async {
            s.app.goal_is_open(&s.id)
        })
        .await?;
        Ok(s)
    }

    /// Code changed in the task's worktree, and no spec: the gate blocks the next turn's end.
    async fn change_code(s: &Server) -> TestResult {
        let task = s.app.deps.store.task(&s.id).await?.ok_or("the task")?;
        let worktree = PathBuf::from(task.worktree_path.ok_or("a worktree")?);
        std::fs::create_dir_all(worktree.join("src"))?;
        std::fs::write(
            worktree.join("src/lib.rs"),
            "pub fn answer() -> u32 {\n    42\n}\n",
        )?;
        Ok(())
    }

    /// What became of a session that held its person's goal: its runs, how often its screen
    /// says the goal was set, and whether the goal is still open.
    async fn own_goal(s: &Server) -> (usize, usize, bool) {
        let goals = screen(&s.app, &s.id).matches("GOAL SET").count();
        (runs(s).await, goals, s.app.goal_is_open(&s.id))
    }

    /// SPEC-468 CCF-001 (review round 3, minor 1): a goal a person set in a prompt-mode session
    /// lives in its process, and a prompt-mode resume types nothing. A turn that closes with
    /// that goal open and no verdict leaves the task its person's turn: the session is not
    /// restarted idle, or the goal would be gone and nobody told.
    #[test]
    fn a_persons_own_goal_keeps_an_idle_prompt_session_where_it_is() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let may_end = data.join("turn-may-end");
        let rt = runtime()?;
        rt.block_on(async {
            let s = session_with_own_goal(&data, &version, &may_end, false).await?;
            let flag = decided(&s.id);
            std::fs::write(&may_end, "")?;
            until_decided(&flag).await?;
            assert_eq!(
                own_goal(&s).await,
                (1, 1, true),
                "{}",
                screen(&s.app, &s.id)
            );
            settled(&s, Activity::Waiting).await?;
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-468 CCF-001: the same session at a Stop the gate blocks. A restart there would give
    /// the resumed session the gate's reasons and no goal: the Stop is answered with the block
    /// instead, and the session keeps its goal.
    #[test]
    fn a_persons_own_goal_keeps_the_session_at_a_stop_the_gate_blocks() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let may_end = data.join("turn-may-end");
        let rt = runtime()?;
        rt.block_on(async {
            let s = session_with_own_goal(&data, &version, &may_end, true).await?;
            std::fs::write(&may_end, "")?;
            // The Stop was answered: with the gate's block, or by a restart of the session.
            wait_for("the Stop's answer", Duration::from_secs(60), || async {
                screen(&s.app, &s.id).contains("STOP BLOCKED") || runs(&s).await > 1
            })
            .await?;
            assert_eq!(
                own_goal(&s).await,
                (1, 1, true),
                "{}",
                screen(&s.app, &s.id)
            );
            assert!(
                screen(&s.app, &s.id).contains("STOP BLOCKED"),
                "the gate blocks as it would without a newer Claude Code"
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-468 CCF-002 (review round 3, minor 2; SPEC-461 CCM-013's "a gate's block"): a
    /// session that works on after its turn because the gate blocks moves at that Stop, in the
    /// same Claude session and subscription, and the restarted session is told the gate's
    /// reasons; it is one switch decision, for Claude Code.
    #[test]
    fn a_session_the_gate_blocks_moves_at_that_stop_with_the_gates_reasons() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server(&data, &version, hooked(), TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            change_code(&s).await?;
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            let attempts = s.app.deps.store.attempts(&s.id).await?;
            assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
            assert_eq!(
                attempts[0].provider_session_id, attempts[1].provider_session_id,
                "the same Claude session"
            );
            assert_eq!(attempts[0].account_id, attempts[1].account_id);
            // The restarted session is told why its turn could not end.
            let launch =
                std::fs::read_to_string(Path::new(&attempts[1].run_dir).join("fake-launch.txt"))?;
            assert!(
                launch.contains("prompt: Some(\"This task needs a spec"),
                "{launch}"
            );
            wait_for("the switch to end", Duration::from_secs(60), || async {
                let switches = s.app.deps.store.switches(&s.id).await;
                switches.is_ok_and(|all| all.iter().all(|one| one.outcome != "started"))
            })
            .await?;
            let switches = s.app.deps.store.switches(&s.id).await?;
            let made: Vec<(&str, &str)> = switches
                .iter()
                .map(|one| (one.reason.as_str(), one.outcome.as_str()))
                .collect();
            assert_eq!(made, [("claude_code", "completed")]);
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                [Some("2.1.311".to_owned()), Some("2.1.322".to_owned())]
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// CCM-013 (review round 1, finding 3): a session that cannot continue on a version stops
    /// its task, and no other session is sent to that version: the card says so.
    #[test]
    fn a_version_a_session_could_not_continue_on_gets_no_other_session() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let broken = data.join("launch-fails");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                fail_launch_file: Some(broken.clone()),
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // The new Claude Code does not start.
            std::fs::write(&broken, "")?;
            turn(&s, "two").await?;
            wait_for("the failed move", Duration::from_secs(90), || async {
                state(&s.app, &s.id).await.map(|(status, _)| status)
                    == Some(TaskStatus::Stopped(StopReason::SwitchFailed))
            })
            .await?;
            let stopped = moves_stopped(&s).await?;
            assert!(
                stopped.starts_with(
                    "Running tasks are no longer moved to 2.1.322: task #1 could not continue on it"
                ),
                "{stopped}"
            );
            assert!(
                stopped.ends_with(
                    "Sessions that start or continue still get it; choose Off to run the host's \
                     Claude Code instead."
                ),
                "{stopped}"
            );
            std::fs::remove_file(&broken)?;
            another_task_stays(&s, &version).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }
    /// Waits until the Instance page says why running tasks are no longer moved.
    async fn moves_stopped(s: &Server) -> Result<String, Box<dyn std::error::Error>> {
        let ctx = s.ctx().await?;
        wait_for("the card to say so", Duration::from_secs(60), || async {
            s.app
                .claude_code_view(&ctx)
                .await
                .is_ok_and(|view| view.moves_stopped.is_some())
        })
        .await?;
        let view = s.app.claude_code_view(&ctx).await?;
        Ok(view.moves_stopped.ok_or("why moves stopped")?)
    }

    /// Another task, started on the old version, keeps its session when its turn ends, and is
    /// listed as behind.
    async fn another_task_stays(s: &Server, version: &Path) -> TestResult {
        std::fs::write(version, "2.1.311\n")?;
        let other = new_task(&s.app, &s.sandbox, &s.user, "Another", TaskMode::Prompt).await?;
        ready(&s.app, &other).await?;
        let flag = decided(&other);
        turn_of(&s.app, &other, "hello").await?;
        until_decided(&flag).await?;
        assert_eq!(runs_of(&s.app, &other).await, 1, "not sent to that version");
        let behind = s.app.claude_code_view(&s.ctx().await?).await?.behind;
        assert!(
            behind
                .iter()
                .any(|task| task.number == 2 && task.version == "2.1.311"),
            "{behind:?}"
        );
        stop(&s.app, &other).await
    }

    /// CCM-013 (review round 2, major 2): a release that starts and exits at once, as one that
    /// cannot resume a session does, gets no other session: the restarted session's end by
    /// itself, before it started, stops further restarts, and the card says so.
    #[test]
    fn a_restarted_session_that_exits_at_once_stops_further_restarts() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let broken = data.join("exits-at-start");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                exit_at_start_file: Some(broken.clone()),
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // The new Claude Code starts and exits.
            std::fs::write(&broken, "")?;
            turn(&s, "two").await?;
            let stopped = moves_stopped(&s).await?;
            assert!(
                stopped.starts_with(
                    "Running tasks are no longer moved to 2.1.322: the session of task #1 ended \
                     as it restarted on it."
                ),
                "{stopped}"
            );
            wait_for("the task to stop", Duration::from_secs(60), || async {
                state(&s.app, &s.id)
                    .await
                    .is_some_and(|(status, _)| !status.is_live())
            })
            .await?;
            assert_eq!(runs(&s).await, 2, "{:?}", state(&s.app, &s.id).await);
            std::fs::remove_file(&broken)?;
            another_task_stays(&s, &version).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// CCM-013 (review round 2, major 2): a release that starts and never starts its session,
    /// as one that opens on a dialog nobody answers does, gets no other session once its time
    /// to start passed; the session itself is left for its person.
    #[test]
    fn a_restarted_session_that_never_starts_stops_further_restarts() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let stuck = data.join("stuck-at-start");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                stuck_at_start_file: Some(stuck.clone()),
                ..hooked()
            };
            let s = server_with(
                &data,
                &version,
                options,
                TaskMode::Prompt,
                "2.1.311",
                |config| config.claude_code_start_ms = 2_000,
            )
            .await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            std::fs::write(&stuck, "")?;
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            let stopped = moves_stopped(&s).await?;
            assert!(
                stopped.starts_with(
                    "Running tasks are no longer moved to 2.1.322: the session of task #1 did \
                     not start on it within 2 s."
                ),
                "{stopped}"
            );
            // The session is left as it is, for its person to look at.
            assert_eq!(
                state(&s.app, &s.id).await.map(|(status, _)| status),
                Some(TaskStatus::Running)
            );
            assert_eq!(runs(&s).await, 2);
            std::fs::remove_file(&stuck)?;
            another_task_stays(&s, &version).await?;
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// CCM-013: a restarted session that started in time shows that the version works: when its
    /// time to start passed, nothing is stopped, also after the session ended later.
    #[test]
    fn a_restarted_session_that_starts_in_time_stops_nothing() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server_with(
                &data,
                &version,
                hooked(),
                TaskMode::Prompt,
                "2.1.311",
                // Long enough for a session to start on a loaded machine.
                |config| config.claude_code_start_ms = 10_000,
            )
            .await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            let judged = reached(Point::ClaudeMoveStartJudged, &s.id);
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            wait_for(
                "its time to start to pass",
                Duration::from_secs(60),
                || async { judged.load(Ordering::SeqCst) },
            )
            .await?;
            let ctx = s.ctx().await?;
            assert_eq!(s.app.claude_code_view(&ctx).await?.moves_stopped, None);
            // Its later end, by itself, says nothing about the version either.
            s.app.send_input(&s.id, b"/crash\r").await?;
            wait_for("the session's end", Duration::from_secs(60), || async {
                state(&s.app, &s.id)
                    .await
                    .is_some_and(|(status, _)| !status.is_live())
            })
            .await?;
            assert_eq!(s.app.claude_code_view(&ctx).await?.moves_stopped, None);
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// Review round 2 (minor 5): the decision is made inside the Stop hook's request, which its
    /// client gives up on after 5 s. A look at the session that does not end in the time it has
    /// (none here) leaves the session where it is, and the turn's end goes on.
    #[test]
    fn a_decision_that_takes_too_long_leaves_the_session_where_it_is() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = server_with(
                &data,
                &version,
                hooked(),
                TaskMode::Prompt,
                "2.1.311",
                |config| config.claude_code_decision_ms = 0,
            )
            .await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // Nothing else keeps the session: only the look at its processes, which has no time.
            turn_that_stays(&s, "two").await?;
            settled(&s, Activity::Waiting).await?;
            let detail = s.app.task_detail(&s.ctx().await?, s.id.as_str()).await?;
            assert_eq!(detail.cli_moves_to.as_deref(), Some("2.1.322"));
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// Review round 2 (minor 4): a restart that cannot start now (its subscription is at its
    /// limit) says nothing, records nothing, and keeps no subscription switch waiting: it has
    /// its own wait.
    #[test]
    fn a_refused_restart_is_silent_and_keeps_no_subscription_switch_waiting() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        // A turn ends when this file exists.
        let may_end = data.join("turn-may-end");
        std::fs::write(&may_end, "")?;
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                turn_hold_file: Some(may_end.clone()),
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // While the next turn runs, its subscription reaches its limit.
            std::fs::remove_file(&may_end)?;
            let mut watch = s.app.runtime(&s.id).hub.subscribe().rx;
            s.app.send_input(&s.id, b"two\r").await?;
            wait_for("the turn to run", Duration::from_secs(30), || async {
                state(&s.app, &s.id).await == Some((TaskStatus::Running, Some(Activity::Working)))
            })
            .await?;
            s.probe.set("a-first", usage(100.0, 4 * HOUR, 30.0, 100));
            refresh(&s.app, &s.user, "a-first").await?;
            let flag = decided(&s.id);
            std::fs::write(&may_end, "")?;
            until_decided(&flag).await?;

            assert_eq!(runs(&s).await, 1, "the session stays");
            assert!(s.app.claude_move_cooling(&s.id), "the restart was tried");
            assert!(
                !s.app.switch_cooling(&s.id),
                "no subscription switch waits for it"
            );
            let notices = said(&mut watch);
            assert!(notices.is_empty(), "{notices:?}");
            assert!(
                s.app.deps.store.switches(&s.id).await?.is_empty(),
                "a refused restart is no switch decision"
            );
            let ctx = s.ctx().await?;
            assert_eq!(s.app.claude_code_view(&ctx).await?.moves_stopped, None);
            // The subscription has room again: the next turn's end does not try again while
            // that wait lasts.
            s.probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
            refresh(&s.app, &s.user, "a-first").await?;
            turn_that_stays(&s, "three").await?;
            assert!(s.app.claude_move_cooling(&s.id));
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }
}
