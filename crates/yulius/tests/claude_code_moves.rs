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
        // A run taken to be on a version (a session restarted on it that wrote nothing yet)
        // shows that one until its transcript names its own.
        let run = app.deps.store.live_attempt(&id).await?.ok_or("a run")?.id;
        let store = &app.deps.store;
        store
            .assume_run_cli_version(&run, &id, "2.1.300", app.now())
            .await?;
        assert_eq!(run_versions(&app, &id).await, [Some("2.1.300".to_owned())]);

        app.send_input(&id, b"first turn\r").await?;
        wait_for("the version", Duration::from_secs(30), || async {
            run_versions(&app, &id).await == [Some("2.1.311".to_owned())]
        })
        .await?;
        // What the transcript named is never replaced by an assumption.
        store
            .assume_run_cli_version(&run, &id, "2.1.999", app.now())
            .await?;
        assert_eq!(run_versions(&app, &id).await, [Some("2.1.311".to_owned())]);
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

/// SPEC-461 CCM-013 (review of the first-record fix): the version a run is taken to run fills
/// only what its transcript left open: a run without a row, a run whose figures were saved
/// first, never a version the transcript named; and a version named later replaces it.
#[tokio::test]
async fn an_assumed_version_fills_only_what_the_transcript_left_open() -> TestResult {
    use crate::support::{harness, insert_sandbox};
    use yulius_app::model::{
        AttemptRecord, LaunchKind, MetricsRecord, Sealed, TaskMode, TaskOrigin, TaskRecord,
    };
    use yulius_app::ports::store::TaskStore as _;
    use yulius_domain::harness::Harness;
    use yulius_domain::ids::{AttemptId, SandboxId};
    use yulius_domain::task::{AttemptState, TaskStatus};
    use yulius_domain::time::Timestamp;

    let h = harness()?;
    insert_sandbox(&h, "s1", "studio", 20000).await?;
    let task = TaskId::parse("t1")?;
    let at = Timestamp(1_794_000_000_000);
    h.store
        .insert_task(&TaskRecord {
            id: task.clone(),
            sandbox_id: SandboxId::parse("s1")?,
            number: 0,
            title: "Versions".into(),
            prompt: String::new(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: UserId::parse("u")?,
            owner: UserId::parse("u")?,
            created_at: at,
            updated_at: at,
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    let mut runs = Vec::new();
    for seq in 1..=3_u32 {
        let id = AttemptId::parse(&format!("t1-a{seq}"))?;
        h.store
            .insert_attempt(&AttemptRecord {
                id: id.clone(),
                task_id: task.clone(),
                seq,
                harness: Harness::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::Resume,
                state: AttemptState::Exited,
                run_dir: "/nonexistent".into(),
                token_hash: format!("hash-{seq}"),
                host_token: Sealed(vec![0]),
                ptyhost_pid: None,
                child_pid: None,
                started_at: at,
                ended_at: Some(at),
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        runs.push(id);
    }
    let facts = |of: usize| {
        let (store, task, run) = (&h.store, task.clone(), runs[of].clone());
        async move {
            let all = store.task_run_facts(&task).await.unwrap_or_default();
            let mine = all.into_iter().find(|facts| facts.attempt == run);
            mine.map(|facts| (facts.cli_version, facts.model))
        }
    };
    let named = |version: &str| Some((Some(version.to_owned()), None));

    // The watcher saved the second run's figures, without a version; the third run's transcript
    // named one.
    let figures = MetricsRecord {
        model: Some("fake-model".into()),
        ..MetricsRecord::default()
    };
    h.store.put_metrics(&runs[1], &task, &figures, at).await?;
    h.store
        .set_run_cli_version(&runs[2], &task, "2.1.330", at)
        .await?;
    for run in &runs {
        h.store
            .assume_run_cli_version(run, &task, "2.1.322", at)
            .await?;
    }
    assert_eq!(facts(0).await, named("2.1.322"), "a run without a row");
    assert_eq!(
        facts(1).await,
        Some((Some("2.1.322".to_owned()), Some("fake-model".to_owned()))),
        "a run whose figures were saved first keeps them"
    );
    assert_eq!(facts(2).await, named("2.1.330"), "the transcript's word");

    // The transcript names a version later: it replaces the assumption, for good.
    h.store
        .set_run_cli_version(&runs[0], &task, "2.1.330", at)
        .await?;
    h.store
        .assume_run_cli_version(&runs[0], &task, "2.1.322", at)
        .await?;
    assert_eq!(facts(0).await, named("2.1.330"));
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
    use yulius_app::events::Envelope;
    use yulius_app::hub::HubFrame;
    use yulius_app::model::{LaunchKind, TaskMode};
    use yulius_app::ports::agent_cli::{AgentCli, AgentCliStatus, CliSource};
    use yulius_app::race::{self, Point};
    use yulius_contracts::events::UiEvent;
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
        run_live(s, 2).await
    }

    /// Waits until the task has `runs` runs and the last one is live.
    async fn run_live(s: &Server, runs: usize) -> TestResult {
        wait_for("the restarted session", Duration::from_secs(90), || async {
            let attempts = s.app.deps.store.attempts(&s.id).await.unwrap_or_default();
            attempts.len() == runs && attempts[runs - 1].state == AttemptState::Live
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

    /// What a task read in one update published to the UI: its status and its activity.
    type Read = (String, Option<String>);

    /// What the task `id` read in each update published to the UI on `events` so far, oldest
    /// first.
    fn published(
        events: &mut tokio::sync::broadcast::Receiver<Envelope>,
        id: &TaskId,
    ) -> Result<Vec<Read>, Box<dyn std::error::Error>> {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut read = Vec::new();
        loop {
            match events.try_recv() {
                Ok(envelope) => {
                    if let UiEvent::TaskUpdated { task } = envelope.event
                        && task.id == id.as_str()
                    {
                        read.push((task.status.clone(), task.activity.clone()));
                    }
                }
                Err(TryRecvError::Lagged(lost)) => {
                    return Err(format!("{lost} updates were lost").into());
                }
                Err(_) => return Ok(read),
            }
        }
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
            assert!(!detail.cli_moves_stopped, "it moves when its turn ends");
            assert_eq!(runs(&s).await, 1, "an idle session is left alone");

            // Its next turn ends: the session restarts on the new Claude Code.
            let mut watch = s.app.runtime(&s.id).hub.subscribe().rx;
            let mut events = s.app.events.subscribe();
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
            // The restart is no news (SPEC-468 CCF-008): the task was its person's turn, read
            // "continuing" while it restarted, and is their turn again. It never read Working
            // in between, which viewers took for work and a notification for a new "Your turn".
            let read = published(&mut events, &s.id)?;
            let restart = read.iter().position(|(status, _)| status == "switching");
            let from_restart: Vec<(&str, Option<&str>)> = read
                [restart.ok_or("it read switching")?..]
                .iter()
                .map(|(status, activity)| (status.as_str(), activity.as_deref()))
                .collect();
            assert!(
                from_restart
                    .iter()
                    .all(|(_, activity)| *activity != Some("working")),
                "{from_restart:?}"
            );
            assert_eq!(
                from_restart.last(),
                Some(&("running", Some("waiting"))),
                "{from_restart:?}"
            );
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

            // A later release moves it again at its next turn's end: the first restart gave
            // the task's one restart at a time back when it was over.
            release(&s, &version, "2.1.330")?;
            let again = decided(&s.id);
            turn(&s, "four").await?;
            run_live(&s, 3).await?;
            until_decided(&again).await?;
            settled(&s, Activity::Waiting).await?;
            wait_for(
                "the third session's version",
                Duration::from_secs(30),
                || async {
                    run_versions(&s.app, &s.id).await
                        == [
                            Some("2.1.311".to_owned()),
                            Some("2.1.322".to_owned()),
                            Some("2.1.330".to_owned()),
                        ]
                },
            )
            .await?;
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
            // What a person typed into the old session is no draft in the new one, whose
            // prompt box starts empty (SPEC-468 CCF-010).
            s.app.record_typing(&s.id, &s.user);
            assert!(s.app.has_draft(&s.id));
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
            assert!(!s.app.has_draft(&s.id), "a new run's prompt box is empty");
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

    /// The task token of the run [`on_the_test_clock`] seeds.
    const CLOCK_TOKEN: &str = "task-token-for-claude-move-tests-0123456789";
    const MINUTE: i64 = 60_000;

    /// A prompt-mode task on the test's clock, with no process: its live run names Claude Code
    /// 2.1.311, runs get 2.1.322, and its hooks come over the internal listener ([`hook`]). Its
    /// subscription has room, or with `limited` is at a usage limit that resets in an hour.
    async fn on_the_test_clock(
        limited: bool,
    ) -> Result<(crate::support::Harness, TaskId), Box<dyn std::error::Error>> {
        on_the_test_clock_limited_for(limited.then_some(60 * MINUTE)).await
    }

    /// [`on_the_test_clock`] whose subscription is at a usage limit that resets in `limited_ms`.
    async fn on_the_test_clock_limited_for(
        limited_ms: Option<i64>,
    ) -> Result<(crate::support::Harness, TaskId), Box<dyn std::error::Error>> {
        use crate::support::{harness, seed_github_user, seed_sandbox, seed_task};
        use yulius_app::model::{AccountRecord, AccountStatus, AttemptRecord};
        use yulius_domain::harness::{Harness, Provider};
        use yulius_domain::ids::{AccountId, AttemptId};
        use yulius_domain::time::Timestamp;
        let h = harness()?;
        seed_github_user(&h, "u-alice", "alice").await?;
        seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
        let now = h.app.now();
        let usage = limited_ms.map(|resets_in| {
            serde_json::json!({"primary": null, "secondary": null, "limit_reached": true,
                "limit_resets_at": now.0 + resets_in, "observed_at": now.0})
            .to_string()
        });
        let account = AccountId::parse("acc-1")?;
        let store = &h.app.deps.store;
        store
            .insert_account(&AccountRecord {
                id: account.clone(),
                provider: Provider::Anthropic,
                label: "Claude acc-1".into(),
                credential: Some(h.app.deps.crypto.seal(b"acc-1")?),
                status: AccountStatus::Ok,
                usage_json: usage,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                email: None,
                email_checked_at: None,
                email_typed: false,
            })
            .await?;
        let task = seed_task(&h, "t-draft", "s1", "u-alice", TaskStatus::Running, None).await?;
        let run = AttemptId::parse("a-t-draft")?;
        store
            .insert_attempt(&AttemptRecord {
                id: run.clone(),
                task_id: task.id.clone(),
                seq: 1,
                harness: Harness::ClaudeCode,
                account_id: Some(account),
                provider_session_id: None,
                launch_kind: LaunchKind::New,
                state: AttemptState::Live,
                run_dir: "/nonexistent/run".into(),
                token_hash: h.app.deps.crypto.sha256_hex(CLOCK_TOKEN.as_bytes()),
                host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
                ptyhost_pid: None,
                child_pid: None,
                started_at: now,
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        store
            .set_run_cli_version(&run, &task.id, "2.1.311", now)
            .await?;
        let installed = Arc::new(Installed(Mutex::new(Some(v("2.1.322")))));
        assert!(h.app.set_agent_cli(installed as Arc<_>, Channel::Latest));
        Ok((h, task.id))
    }

    /// A hook of that run, sent now: its turn's Stop, or with `prompt` the submit of that
    /// prompt. The status it is answered with (200, or 202 while it is held) and what it said.
    async fn hook_answered(
        h: &crate::support::Harness,
        prompt: Option<&str>,
    ) -> Result<(axum::http::StatusCode, serde_json::Value), Box<dyn std::error::Error>> {
        use tower::ServiceExt as _;
        let (event, body) = match prompt {
            Some(text) => (
                "UserPromptSubmit",
                serde_json::json!({"hook_event_name": "UserPromptSubmit", "session_id": "s",
                    "prompt": text}),
            ),
            None => (
                "Stop",
                serde_json::json!({"hook_event_name": "Stop", "session_id": "s",
                    "stop_hook_active": false}),
            ),
        };
        let request = axum::http::Request::post(format!("/hooks/claude/{event}"))
            .header("authorization", format!("Bearer {CLOCK_TOKEN}"))
            .header("content-type", "application/json")
            .header("x-yulius-hook-sent-ms", h.app.now().0.to_string())
            .body(axum::body::Body::from(body.to_string()))?;
        let answer = h.internal.clone().oneshot(request).await?;
        let status = answer.status();
        let said = axum::body::to_bytes(answer.into_body(), 1 << 20).await?;
        Ok((status, serde_json::from_slice(&said).unwrap_or_default()))
    }

    /// [`hook_answered`], for its status alone: 200, or 202 while it is held.
    async fn hook(
        h: &crate::support::Harness,
        prompt: Option<&str>,
    ) -> Result<axum::http::StatusCode, Box<dyn std::error::Error>> {
        Ok(hook_answered(h, prompt).await?.0)
    }

    /// A Jev that reads a typed prompt as new scope, so Yulius holds it (TSK-013), once it
    /// read `passes` prompts as context.
    struct HoldsPrompts {
        passes: std::sync::atomic::AtomicUsize,
    }

    /// Installs [`HoldsPrompts`] as the scope check of `h`.
    fn scope_check_holds_after(h: &crate::support::Harness, passes: usize) {
        let jev = Arc::new(HoldsPrompts {
            passes: std::sync::atomic::AtomicUsize::new(passes),
        }) as Arc<dyn yulius_app::ports::intake::ChoiceEvaluator>;
        assert!(
            h.app
                .set_intake(yulius_app::intake::IntakeServices::new(Some(jev), None))
        );
    }

    #[async_trait]
    impl yulius_app::ports::intake::ChoiceEvaluator for HoldsPrompts {
        async fn evaluate(
            &self,
            _state: &str,
            question: &yulius_domain::intake::ChoiceQuestion<'_>,
            _deadline: Duration,
        ) -> Result<yulius_domain::intake::ChoiceAnswer, yulius_app::ports::intake::JevFailure>
        {
            let passed = self
                .passes
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
            let choice = if passed { "context" } else { "new_scope" };
            let odds = |key: &str| if key == choice { 0.97 } else { 0.01 };
            Ok(yulius_domain::intake::ChoiceAnswer {
                choice: choice.to_owned(),
                confidence: 0.97,
                probabilities: question
                    .options
                    .iter()
                    .map(|option| (option.key.to_owned(), odds(option.key)))
                    .collect(),
            })
        }
    }

    /// SPEC-468 CCF-010: a prompt Yulius's scope check holds (TSK-013) never reaches the agent,
    /// and Claude Code puts its text back into the prompt box (the intake module's own check
    /// with 2.1.295; 2.1.322 lands a line a hook dropped in the composer). That submit emptied
    /// nothing: the turn that ran meanwhile ends and the session stays. The hook used to count
    /// as a submit, so the held text went with the old process while the task's card still
    /// offered "Send anyway".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_prompt_yulius_holds_is_still_in_the_prompt_box() -> TestResult {
        use axum::http::StatusCode;
        let (h, task) = on_the_test_clock(false).await?;
        let alice = UserId::parse("u-alice")?;
        scope_check_holds_after(&h, 0);

        // A turn runs (a prompt of three words or fewer is not checked).
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::OK);
        // Its person types the next prompt and enters it; Yulius holds it.
        let next = "Also add a dark mode to the whole settings page";
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        h.clock.0.fetch_add(100, Ordering::SeqCst);
        let (status, answer) = hook_answered(&h, Some(next)).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["decision"], "block", "{answer}");
        assert!(
            h.app.has_draft(&task),
            "the held prompt is back in the prompt box"
        );
        // The turn that ran meanwhile ends five minutes later: the session stays.
        h.clock.0.fetch_add(5 * MINUTE, Ordering::SeqCst);
        let probed = reached(Point::ClaudeMoveProbed, &task);
        let flag = decided(&task);
        assert_eq!(hook(&h, None).await?, StatusCode::OK);
        until_decided(&flag).await?;
        assert!(
            !probed.load(Ordering::SeqCst) && !h.app.claude_move_cooling(&task),
            "a session with a held prompt in its box is not looked at, and no restart is tried"
        );
        Ok(())
    }

    /// SPEC-468 CCF-010: a prompt that waits for a subscription with room and is then held by
    /// the scope check (which let it through before the wait and reads it again after) was
    /// never taken by the agent: Claude Code puts it back into the prompt box. Its first
    /// request used to empty the box at once, so the text was forgotten.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_prompt_held_for_room_and_then_by_the_scope_check_is_still_in_the_box() -> TestResult
    {
        use axum::http::StatusCode;
        let (h, task) = on_the_test_clock_limited_for(Some(30_000)).await?;
        let alice = UserId::parse("u-alice")?;
        scope_check_holds_after(&h, 1);

        // Its person types a prompt and enters it: no subscription has room, so it waits.
        let prompt = "Also add a dark mode to the whole settings page";
        h.app.record_typing(&task, &alice);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        h.clock.0.fetch_add(100, Ordering::SeqCst);
        assert_eq!(hook(&h, Some(prompt)).await?, StatusCode::ACCEPTED);
        h.clock.0.fetch_add(10_000, Ordering::SeqCst);
        assert_eq!(hook(&h, Some(prompt)).await?, StatusCode::ACCEPTED);
        // The limit resets half a minute later. The prompt goes on to the scope check, which
        // holds it for its person's choice.
        h.clock.0.fetch_add(25_000, Ordering::SeqCst);
        let _ = h.app.resume_round().await;
        let (status, answer) = hook_answered(&h, Some(prompt)).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["decision"], "block", "{answer}");
        assert!(
            h.app.has_draft(&task),
            "the prompt is back in the prompt box"
        );
        Ok(())
    }

    /// SPEC-468 CCF-010 (review round 3): a prompt whose hook Yulius answers with an error
    /// goes on all the same, since the hook client fails open: the box is empty. Here the task
    /// is deleted while its prompt is answered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_prompt_whose_hook_fails_was_submitted_all_the_same() -> TestResult {
        use axum::http::StatusCode;
        use yulius_app::ports::store::TaskStore as _;
        let (h, task) = on_the_test_clock(false).await?;
        let alice = UserId::parse("u-alice")?;
        h.app.record_typing(&task, &alice);
        assert!(h.app.has_draft(&task));
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let (store, gone) = (h.store.clone(), task.clone());
        race::on(
            Point::PromptTurnChecked,
            Path::new(task.as_str()),
            move || {
                tokio::task::block_in_place(|| {
                    let deleted =
                        tokio::runtime::Handle::current().block_on(store.delete_task(&gone));
                    assert!(matches!(deleted, Ok(true)), "{deleted:?}");
                });
            },
        );
        let status = hook(&h, Some("first")).await?;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "the hook was answered with an error"
        );
        assert!(
            !h.app.has_draft(&task),
            "the prompt went on: what was typed before it is no draft"
        );
        Ok(())
    }

    /// SPEC-468 CCF-010 (the review's question of direction): a draft that sits in a session's
    /// prompt box keeps the session however long ago it was typed. Yulius cannot read the box,
    /// but it sees every keystroke and every submitted prompt: a session typed into since its
    /// last prompt is left alone, and the next submitted prompt lifts that. The clock is the
    /// test's, the hooks come over the internal listener, and no process runs: a restart that
    /// is tried is refused (the sandbox has no repository), which is what shows the try.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_draft_typed_since_the_last_prompt_keeps_the_session() -> TestResult {
        use axum::http::StatusCode;
        let (h, task) = on_the_test_clock(false).await?;
        let alice = UserId::parse("u-alice")?;

        // A prompt is submitted and its turn runs. Meanwhile its person types the next message
        // and does not send it.
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::OK);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        assert!(h.app.has_draft(&task));
        // Claude Code wakes the session by itself (a background shell ended): that prompt did
        // not come through the prompt box, and empties nothing.
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let wake = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n\
                    </task-notification>";
        assert_eq!(hook(&h, Some(wake)).await?, StatusCode::OK);
        assert!(h.app.has_draft(&task), "a wake-up is no submit");
        // The turn ends five minutes later: the minute after a keystroke is long over.
        h.clock.0.fetch_add(5 * MINUTE, Ordering::SeqCst);
        let probed = reached(Point::ClaudeMoveProbed, &task);
        let flag = decided(&task);
        assert_eq!(hook(&h, None).await?, StatusCode::OK);
        until_decided(&flag).await?;
        assert!(
            !probed.load(Ordering::SeqCst) && !h.app.claude_move_cooling(&task),
            "a session with a draft is not looked at, and no restart is tried"
        );

        // They send it: the box is empty again, and that turn's end restarts the session.
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        assert_eq!(hook(&h, Some("second")).await?, StatusCode::OK);
        assert!(!h.app.has_draft(&task), "typed, then submitted");
        h.clock.0.fetch_add(5 * MINUTE, Ordering::SeqCst);
        let flag = decided(&task);
        assert_eq!(hook(&h, None).await?, StatusCode::OK);
        until_decided(&flag).await?;
        assert!(probed.load(Ordering::SeqCst), "the session was looked at");
        assert!(h.app.claude_move_cooling(&task), "the restart was tried");
        Ok(())
    }

    /// SPEC-468 CCF-010: a prompt that is held at its submit (here: no subscription has room)
    /// is sent again by the hook client every ten seconds, stamped anew each time, until it
    /// goes on. That is one submit: what a person types while it is held is a draft, and it
    /// is one still when the prompt goes on. Each request used to count as a submit, so the
    /// draft was forgotten ten seconds after it was typed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_draft_typed_while_a_prompt_is_held_stays_a_draft() -> TestResult {
        use axum::http::StatusCode;
        let (h, task) = on_the_test_clock(true).await?;
        let alice = UserId::parse("u-alice")?;

        // The prompt waits for a subscription with room, and its person types on meanwhile.
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::ACCEPTED);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        assert!(h.app.has_draft(&task));
        // The hook client asks again: the same submit, still held.
        h.clock.0.fetch_add(10_000, Ordering::SeqCst);
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::ACCEPTED);
        assert!(
            h.app.has_draft(&task),
            "a prompt that is held is one submit"
        );
        // The limit resets and the prompt goes on: what was typed meanwhile is still there.
        h.clock.0.fetch_add(61 * MINUTE, Ordering::SeqCst);
        let _ = h.app.resume_round().await;
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::OK);
        assert!(h.app.has_draft(&task), "it went on, and the draft stays");
        // The next prompt they send empties the box.
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        h.app.record_typing(&task, &alice);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        assert_eq!(hook(&h, Some("second")).await?, StatusCode::OK);
        assert!(!h.app.has_draft(&task), "typed, then submitted");
        Ok(())
    }

    /// What the session's terminal printed since `watch` was last read.
    fn printed(watch: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> String {
        let mut out = String::new();
        while let Ok(frame) = watch.try_recv() {
            if let HubFrame::Output { bytes, .. } = frame {
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        out
    }

    /// The task's person tells it "Keep the header blue" through their Superagent
    /// (`task_message`): what the call answered.
    async fn told(
        app: &Arc<App>,
        sandbox: &SandboxId,
        user: &UserId,
        id: &TaskId,
    ) -> Result<serde_json::Value, yulius_app::error::AppError> {
        use yulius_app::coordination::{Channel as Through, ProjectCaller};
        let caller = ProjectCaller {
            sandbox: sandbox.clone(),
            user: user.clone(),
            task: None,
            channel: Through::Superagent,
            actor: "cockpit:admin".into(),
            detail: serde_json::json!({"via": "cockpit", "shell": "sh-test"}),
        };
        app.tool_task_message(
            &caller,
            &serde_json::json!({"task_id": id.as_str(), "text": "Keep the header blue"}),
        )
        .await
    }

    /// SPEC-468 CCF-011: a message Yulius typed for a person (here: through their Superagent)
    /// is a draft until it is submitted: a turn's end before that, which the gate blocks here,
    /// leaves the session where it is, the message gets its turn, and that turn's end
    /// restarts the session.
    ///
    /// This runs in the fake agent's order: it submits a line (the prompt hook) when it reads
    /// it, after its turn. Claude Code submits at the Enter, also while a turn runs (seen with
    /// 2.1.322), and then keeps the message in its queue until the turn ends. There this rule
    /// covers the message from its paste to that submit only, and a restart at the turn's end
    /// still loses it from the queue: issue #1204.
    #[test]
    fn a_message_yulius_typed_keeps_the_session_until_its_submit() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let may_end = data.join("turn-may-end");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                turn_hold_file: Some(may_end.clone()),
                ..hooked()
            };
            // Runs get 2.1.322 from the start; the first session started on 2.1.311.
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.322").await?;
            std::fs::write(&version, "2.1.322\n")?;
            change_code(&s).await?;
            let mut watch = s.app.runtime(&s.id).hub.subscribe().rx;
            // A turn runs, and the gate will block its end.
            let submitted = reached(Point::PromptTurnChecked, &s.id);
            s.app.send_input(&s.id, b"one\r").await?;
            wait_for("the prompt's submit", Duration::from_secs(60), || async {
                submitted.load(Ordering::SeqCst)
            })
            .await?;
            // Meanwhile the task's person tells it something through their Superagent: typed
            // into the session, where the fake agent reads it once its turn ended.
            let answer = told(&s.app, &s.sandbox, &s.user, &s.id).await?;
            assert_eq!(answer["delivered"], "sent", "{answer}");
            assert!(
                s.app.has_draft(&s.id),
                "typed into the session and not submitted yet"
            );
            // The turn ends, the message gets its turn, and the session moves at that turn's end.
            std::fs::write(&may_end, "")?;
            let mut shown = String::new();
            wait_for("the restarted session", Duration::from_secs(90), || {
                shown.push_str(&printed(&mut watch));
                async { runs(&s).await == 2 }
            })
            .await?;
            second_run_live(&s).await?;
            shown.push_str(&printed(&mut watch));
            assert!(
                shown.contains("ECHO: Message from admin (through their Superagent):"),
                "the message was lost with the old process: {shown}"
            );
            // That the session moved at all says the message's submit emptied the box: a
            // session with a draft is not restarted.
            assert_eq!(
                run_versions(&s.app, &s.id).await[0],
                Some("2.1.311".to_owned())
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-468 CCF-011 (review round 2): a message that could not be typed (here: no process
    /// runs, so the session has no terminal to type into) is in no prompt box and no queue:
    /// it leaves no draft behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_that_could_not_be_typed_is_no_draft() -> TestResult {
        use axum::http::StatusCode;
        let (h, task) = on_the_test_clock(false).await?;
        h.github
            .set_perm("alice", "acme/fixture", crate::support::perms("write"));
        assert_eq!(hook(&h, Some("first")).await?, StatusCode::OK);
        h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let refused = told(
            &h.app,
            &SandboxId::parse("s1")?,
            &UserId::parse("u-alice")?,
            &task,
        )
        .await;
        assert!(refused.is_err(), "no process runs: {refused:?}");
        assert!(!h.app.has_draft(&task), "nothing was typed: {refused:?}");
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

    /// A prompt-mode session on 2.1.311 while runs get 2.1.322, in which a person sets a goal
    /// of their own and whose goal evaluator gives no verdict; with `code` changed in its
    /// worktree and no spec, so the gate blocks its turn's end. With `may_end` the goal's turn
    /// ends when that file exists, and Yulius has read that the goal is set when this returns;
    /// without it the turn ends by itself, milliseconds after the goal was set.
    async fn session_with_own_goal(
        data: &Path,
        version: &Path,
        may_end: Option<&Path>,
        code: bool,
    ) -> Result<Server, Box<dyn std::error::Error>> {
        let options = FakeAgentOptions {
            turn_hold_file: may_end.map(Path::to_path_buf),
            goal_verdict: Some("none"),
            ..hooked()
        };
        let s = server(data, version, options, TaskMode::Prompt, "2.1.322").await?;
        std::fs::write(version, "2.1.322\n")?;
        if code {
            change_code(&s).await?;
        }
        s.app
            .send_input(&s.id, b"/goal keep the header blue\r")
            .await?;
        if may_end.is_some() {
            goal_read(&s).await?;
        }
        Ok(s)
    }

    /// Waits until Yulius read from the transcript that the session's goal is set.
    async fn goal_read(s: &Server) -> TestResult {
        wait_for("the goal to be read", Duration::from_secs(60), || async {
            s.app.goal_is_open(&s.id)
        })
        .await
    }

    /// Waits until the goal turn's Stop was answered: with the gate's block, or by a restart
    /// of the session.
    async fn stop_answered(s: &Server) -> TestResult {
        wait_for("the Stop's answer", Duration::from_secs(60), || async {
            screen(&s.app, &s.id).contains("STOP BLOCKED") || runs(s).await > 1
        })
        .await
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

    /// SPEC-468 CCF-001 (review round 3, minor 1): Yulius types nothing into a prompt-mode
    /// session it restarts and reads a goal only from the new run's lines, so it could not
    /// follow a goal its person set there across a restart; and the fake agent, unlike the
    /// Claude Code binaries read (2.1.292, 2.1.311, 2.1.322), forgets a goal when it is
    /// resumed. A turn that closes with that goal open and no verdict leaves the task its
    /// person's turn: the session is not restarted idle, so the goal stays in force and known.
    #[test]
    fn a_persons_own_goal_keeps_an_idle_prompt_session_where_it_is() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let may_end = data.join("turn-may-end");
        let rt = runtime()?;
        rt.block_on(async {
            let s = session_with_own_goal(&data, &version, Some(&may_end), false).await?;
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
    /// instead, and the session keeps its goal. Its goal is known when the Stop comes, so the
    /// session is not even looked at.
    #[test]
    fn a_persons_own_goal_keeps_the_session_at_a_stop_the_gate_blocks() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let may_end = data.join("turn-may-end");
        let rt = runtime()?;
        rt.block_on(async {
            let s = session_with_own_goal(&data, &version, Some(&may_end), true).await?;
            let looked_at = reached(Point::ClaudeMoveProbed, &s.id);
            std::fs::write(&may_end, "")?;
            stop_answered(&s).await?;
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
            assert!(
                !looked_at.load(Ordering::SeqCst),
                "a session known to hold its person's goal is not looked at"
            );
            stop(&s.app, &s.id).await
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-468 CCF-001 (review round 1 of this change, minor 1): the goal is set in the very
    /// turn whose Stop the gate blocks, and that Stop comes before Yulius read the goal's
    /// record: the transcript is read once a second, and this turn takes milliseconds. The
    /// Stop asks again once the turn's lines were read, and the session stays. It asked only
    /// before, so the session was restarted with the gate's reasons and Yulius no longer knew
    /// the goal (which the fake agent had forgotten as well).
    /// That the record was unread when the Stop came is not asserted: in the rare run in which
    /// a read falls between the two, the first question keeps the session, and it must stay
    /// either way.
    #[test]
    fn a_goal_set_in_the_turn_the_gate_blocks_keeps_the_session() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let s = session_with_own_goal(&data, &version, None, true).await?;
            stop_answered(&s).await?;
            assert_eq!(runs(&s).await, 1, "{}", screen(&s.app, &s.id));
            goal_read(&s).await?;
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

    /// SPEC-468 CCF-008: a prompt-mode session whose person's goal failed is their turn, with
    /// work left (SPEC-429), and reads so after its idle restart too: what Claude Code last
    /// recorded of the goal is what the agent left, in the same session with nothing typed. It
    /// used to be forgotten with the old run, so the task read "Answered" from then on, which
    /// its person was notified of.
    #[test]
    fn a_goal_that_failed_is_still_unfinished_after_an_idle_restart() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                goal_verdict: Some("failed"),
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.322").await?;
            std::fs::write(&version, "2.1.322\n")?;
            let ctx = s.ctx().await?;
            // Its person sets a goal of their own; the evaluator says it cannot be met, which
            // ends the goal, and the settled turn restarts the idle session.
            let restarted = decided(&s.id);
            s.app.send_input(&s.id, b"/goal make it rain\r").await?;
            until_decided(&restarted).await?;
            second_run_live(&s).await?;
            settled(&s, Activity::Waiting).await?;
            assert!(!s.app.goal_is_open(&s.id), "the goal ended");
            let detail = s.app.task_detail(&ctx, s.id.as_str()).await?;
            assert!(
                detail.summary.goal_unfinished,
                "the work the goal left is still unfinished"
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
                switches.is_ok_and(|all| {
                    !all.is_empty() && all.iter().all(|one| one.outcome != "started")
                })
            })
            .await?;
            let switches = s.app.deps.store.switches(&s.id).await?;
            let made: Vec<(&str, &str)> = switches
                .iter()
                .map(|one| (one.reason.as_str(), one.outcome.as_str()))
                .collect();
            assert_eq!(made, [("claude_code", "completed")]);
            // The restarted session counts as on the new version once the move recorded it,
            // a moment after its switch.
            wait_for(
                "the new session's version",
                Duration::from_secs(30),
                || async {
                    run_versions(&s.app, &s.id).await
                        == [Some("2.1.311".to_owned()), Some("2.1.322".to_owned())]
                },
            )
            .await?;
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
        // Its info promises that version at its next start only (SPEC-468 CCF-003).
        let detail = s.app.task_detail(&s.ctx().await?, other.as_str()).await?;
        assert_eq!(
            (detail.cli_moves_to.as_deref(), detail.cli_moves_stopped),
            (Some("2.1.322"), true)
        );
        let behind = s.app.claude_code_view(&s.ctx().await?).await?.behind;
        assert!(
            behind
                .iter()
                .any(|task| task.number == 2 && task.version == "2.1.311"),
            "{behind:?}"
        );
        // A newer release lifts it: moves were stopped for 2.1.322 only, so the task is
        // promised the newer one when its turn ends again, and the card says nothing.
        s.installed.release("2.1.330");
        let ctx = s.ctx().await?;
        let detail = s.app.task_detail(&ctx, other.as_str()).await?;
        assert_eq!(
            (detail.cli_moves_to.as_deref(), detail.cli_moves_stopped),
            (Some("2.1.330"), false)
        );
        assert_eq!(s.app.claude_code_view(&ctx).await?.moves_stopped, None);
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

    /// CCM-013: a restarted session that started shows that the version works: its start takes
    /// the restart off the breaker's watch, so nothing is stopped whenever its time to start
    /// passes, also after the session ended later. The start is seen at its race point and the
    /// time to start is an hour, so the test does not depend on how fast a session starts
    /// (SPEC-468 CCF-004).
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
                // Never reached here: what counts is the session's start, not the clock.
                |config| config.claude_code_start_ms = 3_600_000,
            )
            .await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            let started = reached(Point::ClaudeMoveStarted, &s.id);
            let restarted = decided(&s.id);
            turn(&s, "two").await?;
            second_run_live(&s).await?;
            wait_for(
                "the restarted session's start",
                Duration::from_secs(60),
                || async { started.load(Ordering::SeqCst) },
            )
            .await?;
            assert!(
                !s.app.claude_code_start_watched(&s.id),
                "a session that started is off the watch"
            );
            let ctx = s.ctx().await?;
            assert_eq!(s.app.claude_code_view(&ctx).await?.moves_stopped, None);
            // Its later end, by itself, says nothing about the version either. The restart is
            // over first: a session's start can be noticed before its run takes input.
            until_decided(&restarted).await?;
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

    /// Two turn ends of one task that both decided to restart its session on 2.1.322 before
    /// either began: the first waits for the second to decide, and the second goes on once the
    /// first's restart reached `second_goes_on_at`. The restarted session never starts and has
    /// 2 s for it. One restart must come of it, and it must be on the breaker's watch.
    async fn two_turn_ends_decide(data: &Path, second_goes_on_at: Point) -> TestResult {
        const HELD: Duration = Duration::from_secs(60);
        let version = data.join("claude-version");
        let stuck = data.join("stuck-at-start");
        let options = FakeAgentOptions {
            stuck_at_start_file: Some(stuck.clone()),
            ..hooked()
        };
        let s = server_with(
            data,
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
        let first_decided = Arc::new(AtomicBool::new(false));
        let in_order = Arc::new(AtomicBool::new(false));
        let (second_tx, second_rx) = std::sync::mpsc::channel::<()>();
        let (go_on_tx, go_on_rx) = std::sync::mpsc::channel::<()>();
        let task = Path::new(s.id.as_str());
        let there = Arc::clone(&first_decided);
        race::on(Point::ClaudeMoveDecided, task, move || {
            there.store(true, Ordering::SeqCst);
            let _ = tokio::task::block_in_place(|| second_rx.recv_timeout(HELD));
        });
        let order = Arc::clone(&in_order);
        race::on(Point::ClaudeMoveDecided, task, move || {
            let _ = second_tx.send(());
            let first = tokio::task::block_in_place(|| go_on_rx.recv_timeout(HELD));
            order.store(first.is_ok(), Ordering::SeqCst);
        });
        // Hooks of one point run in the order they were set: the first's restart gets this
        // one, and each turn's end that is over one of the flags after it.
        race::on(second_goes_on_at, task, move || {
            let _ = go_on_tx.send(());
        });
        let mut over = vec![decided(&s.id)];
        if second_goes_on_at != Point::ClaudeMoveSettled {
            over.push(decided(&s.id));
        }
        let judged = reached(Point::ClaudeMoveStartJudged, &s.id);
        turn(&s, "two").await?;
        wait_for(
            "the first turn's end to decide",
            Duration::from_secs(60),
            || async { first_decided.load(Ordering::SeqCst) },
        )
        .await?;
        turn(&s, "three").await?;
        for flag in &over {
            until_decided(flag).await?;
        }
        assert!(
            in_order.load(Ordering::SeqCst),
            "the second turn's end decided before the first began, and went on after it"
        );
        assert_eq!(runs(&s).await, 2, "one restart");
        // It is watched: the session that never starts is noticed.
        wait_for(
            "its time to start to pass",
            Duration::from_secs(60),
            || async { judged.load(Ordering::SeqCst) },
        )
        .await?;
        let view = s.app.claude_code_view(&s.ctx().await?).await?;
        let stopped = view.moves_stopped.unwrap_or_default();
        assert!(
            stopped.starts_with(
                "Running tasks are no longer moved to 2.1.322: the session of task #1 did not \
                 start on it within 2 s."
            ),
            "the restart was not watched: {stopped:?}"
        );
        assert_eq!(runs(&s).await, 2, "one restart");
        std::fs::remove_file(&stuck)?;
        stop(&s.app, &s.id).await
    }

    /// SPEC-468 CCF-006 (review round 3, nit 6): two turn ends of one task that both decide to
    /// restart its session. The second, which comes while the first's switch is under way (the
    /// task is switching), leaves the session to it. It used to register an equal start, be
    /// refused its switch, and take the first's start off the watch with its own, so a session
    /// that never started stopped nothing.
    #[test]
    fn two_turn_ends_that_decide_at_once_make_one_watched_restart() -> TestResult {
        let (_dir, data) = data_dir()?;
        let rt = runtime()?;
        rt.block_on(two_turn_ends_decide(&data, Point::SwitchDecided))?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-468 CCF-006: the second turn's end comes once the first's restart is over. It
    /// finds the session sent to that version and restarts nothing: the restarted session
    /// would be ended for a third run, and the first restart's start replaced on the watch.
    #[test]
    fn a_turn_end_that_decided_before_a_restart_and_comes_after_it_restarts_nothing() -> TestResult
    {
        let (_dir, data) = data_dir()?;
        let rt = runtime()?;
        rt.block_on(two_turn_ends_decide(&data, Point::ClaudeMoveSettled))?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// Review of the first-record fix: a release lands while a session restarts. The restarted
    /// process names its own, newer version with its first record; the version it was sent to,
    /// written after that, does not replace it.
    #[test]
    fn a_restarted_session_keeps_the_version_its_transcript_named_first() -> TestResult {
        let (_dir, data) = data_dir()?;
        let version = data.join("claude-version");
        let rt = runtime()?;
        rt.block_on(async {
            let options = FakeAgentOptions {
                resume_record: true,
                ..hooked()
            };
            let s = server(&data, &version, options, TaskMode::Prompt, "2.1.311").await?;
            turn_that_stays(&s, "one").await?;
            release(&s, &version, "2.1.322")?;
            // A newer release lands as the session restarts: its process runs 2.1.330.
            std::fs::write(&version, "2.1.330\n")?;
            // The restart's own word waits here until the transcript's is recorded.
            let (named, wait) = std::sync::mpsc::channel::<()>();
            race::on(
                Point::ClaudeMoveRestarted,
                Path::new(s.id.as_str()),
                move || {
                    let _ = wait.recv_timeout(Duration::from_secs(60));
                },
            );
            let restarted = decided(&s.id);
            turn(&s, "two").await?;
            let both = [Some("2.1.311".to_owned()), Some("2.1.330".to_owned())];
            wait_for(
                "the transcript's version",
                Duration::from_secs(60),
                || async { run_versions(&s.app, &s.id).await == both },
            )
            .await?;
            named.send(())?;
            until_decided(&restarted).await?;
            assert_eq!(
                run_versions(&s.app, &s.id).await,
                both,
                "what the transcript named stays"
            );
            // It runs a newer one than runs got when it was sent: nothing restarts it again.
            let flag = decided(&s.id);
            turn(&s, "three").await?;
            until_decided(&flag).await?;
            settled(&s, Activity::Waiting).await?;
            assert_eq!(runs(&s).await, 2);
            stop(&s.app, &s.id).await
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
