//! End-to-end supervision with real processes: supervisor → detached `yulius ptyhost` → fake
//! agent, with real git worktrees. Covers launch, terminal I/O, secret env delivery, a server
//! restart (reconcile + reattach without respawn), crash exits and stop (SPEC-001 RUN-001..006,
//! AC-RUN-1..4).

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{FakeGitHub, TestResult};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::error::AppResult;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::harness::{
    HarnessLaunch, HarnessPlan, HarnessPort, OutputMasker, TranscriptWatch, TranslationTarget,
};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

/// Real harness planning (env, files, secrets) with the agent executable swapped for the fake.
struct FakeAgentHarness {
    inner: HarnessAdapter,
    fake: PathBuf,
}

impl HarnessPort for FakeAgentHarness {
    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan> {
        let mut plan = self.inner.plan(launch)?;
        let session = plan
            .session_id
            .clone()
            .unwrap_or_else(|| "fake-session".into());
        plan.plan.argv = vec![
            self.fake.to_string_lossy().into_owned(),
            "--mode".into(),
            "echo".into(),
            "--session-id".into(),
            session,
            "--report-env".into(),
            "YULIUS_TASK_TOKEN".into(),
        ];
        Ok(plan)
    }
    fn claude_transcript(&self, home: &Path, worktree: &Path, session: &str) -> PathBuf {
        self.inner.claude_transcript(home, worktree, session)
    }
    fn find_codex_rollout(&self, dir: &Path, session: &str) -> AppResult<Option<PathBuf>> {
        self.inner.find_codex_rollout(dir, session)
    }
    fn discover_codex_session(
        &self,
        dir: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        self.inner.discover_codex_session(dir, worktree, since)
    }
    fn newest_claude_session(
        &self,
        home: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        self.inner.newest_claude_session(home, worktree, since)
    }
    fn watch(
        &self,
        harness: Harness,
        path: &Path,
        from_start: bool,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        self.inner.watch(harness, path, from_start)
    }
    fn translate(
        &self,
        from: Harness,
        source: &Path,
        to: Harness,
        target: &TranslationTarget,
    ) -> AppResult<PathBuf> {
        self.inner.translate(from, source, to, target)
    }
    fn login_argv(&self, harness: Harness) -> Vec<String> {
        self.inner.login_argv(harness)
    }
    fn extract_claude_token(&self, output: &str) -> Option<String> {
        self.inner.extract_claude_token(output)
    }
    fn masker(&self) -> Box<dyn OutputMasker> {
        self.inner.masker()
    }
    fn install_skills(
        &self,
        skills: &[(String, PathBuf)],
        claude: &Path,
        codex: &Path,
    ) -> AppResult<()> {
        self.inner.install_skills(skills, claude, codex)
    }
}

fn build(data: &Path, fake: &Path) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let store = SqliteStore::open(&data.join("yulius.db"))?;
    let mut config = AppConfig::new(
        data.to_path_buf(),
        "https://yulius.test",
        "http://127.0.0.1:9",
        exe.clone(),
    );
    config.contained = false;
    let deps = Deps {
        store: Arc::new(store),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
        github: Arc::new(FakeGitHub::default()),
        session_host: Arc::new(PtySessionHost::new(exe.clone())),
        harness: Arc::new(FakeAgentHarness {
            inner: HarnessAdapter::new(exe, "http://127.0.0.1:9"),
            fake: fake.to_path_buf(),
        }),
        git: Arc::new(GitAdapter::new("git")),
    };
    Ok(App::new(config, deps))
}

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(())
}

/// Bare origin with one commit on `main`, cloned as the sandbox repository.
fn seed_repo(data: &Path, sandbox: &SandboxId) -> TestResult {
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    git(&seed, &["add", "README.md"])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("no parent")?)?;
    git(
        data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    Ok(())
}

async fn seed_records(app: &App) -> Result<(SandboxId, UserId), Box<dyn std::error::Error>> {
    let store = &app.deps.store;
    let user = UserRecord {
        id: UserId::parse("u-admin")?,
        github_login: Some("admin".into()),
        github_id: None,
        display_name: "Admin".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    store.insert_user(&user).await?;
    let sandbox = SandboxRecord {
        id: SandboxId::parse("s-fixture")?,
        slug: "fixture".into(),
        repo_owner: "acme".into(),
        repo_name: "fixture".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 41000,
        port_count: 10,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: user.id.clone(),
        created_at: Timestamp(1),
        removed_at: None,
    };
    store.insert_sandbox(&sandbox).await?;
    store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a-claude")?,
            provider: Provider::Anthropic,
            label: "Claude test".into(),
            credential: Some(app.deps.crypto.seal(b"sk-ant-oat01-test-credential")?),
            priority: 0,
            enabled: true,
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok((sandbox.id, user.id))
}

async fn wait_for<F, Fut>(what: &str, timeout: Duration, mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

async fn task(app: &App, id: &TaskId) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

async fn new_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::ClaudeCode,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
        })
        .await?;
    Ok(record.id)
}

async fn screen_contains(app: &App, id: &TaskId, needle: &str) -> bool {
    app.runtime(id).hub.screen_text().contains(needle)
}

fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

#[test]
fn runs_survive_a_server_restart_and_stop_cleanly() -> TestResult {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("yulius_app=info")
        .with_test_writer()
        .try_init();
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    yulius::serve::check_socket_paths(&data)?;

    // Phase 1: first server process launches the run.
    let first = runtime()?;
    let (sandbox, user, task_id, child_pid) = first.block_on(async {
        let app = build(&data, &fake)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Echo survives restart").await?;
        let banner = wait_for("the fake agent banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await;
        if banner.is_err() {
            eprintln!("task: {:?}", task(&app, &id).await?);
            for a in app.deps.store.attempts(&id).await? {
                eprintln!(
                    "attempt: {:?} {:?} {:?} {:?}",
                    a.state, a.exit_code, a.exit_reason, a.run_dir
                );
                for entry in std::fs::read_dir(&a.run_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                    eprintln!(
                        "--- {}\n{}",
                        entry.path().display(),
                        text.chars().take(2000).collect::<String>()
                    );
                }
            }
            eprintln!("screen: {}", app.runtime(&id).hub.screen_text());
        }
        banner?;
        let record = task(&app, &id).await?;
        assert_eq!(record.status, TaskStatus::Running);
        let worktree = PathBuf::from(record.worktree_path.ok_or("no worktree")?);
        assert!(worktree.join("README.md").is_file(), "worktree checked out");
        assert!(record.branch.is_some_and(|b| b.starts_with("feat/")));
        wait_for("the secret env report", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ENV YULIUS_TASK_TOKEN=set")
        })
        .await?;
        app.send_input(&id, b"hello yulius\r").await?;
        wait_for("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello yulius")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        Ok::<_, Box<dyn std::error::Error>>((sandbox, user, id, attempt.child_pid))
    })?;
    // The server "crashes": every task of the first runtime is dropped, sockets close.
    first.shutdown_timeout(Duration::from_secs(5));

    // Phase 2: a new server process reconciles and reattaches without respawning.
    let second = runtime()?;
    second.block_on(async {
        let app = build(&data, &fake)?;
        app.reconcile().await?;
        let record = task(&app, &task_id).await?;
        assert_eq!(record.status, TaskStatus::Running, "run kept after restart");
        let attempt = app
            .deps
            .store
            .live_attempt(&task_id)
            .await?
            .ok_or("attempt lost on restart")?;
        assert_eq!(attempt.child_pid, child_pid, "same process, not respawned");
        assert!(
            screen_contains(&app, &task_id, "ECHO: hello yulius").await,
            "the snapshot restores the screen"
        );
        app.send_input(&task_id, b"again\r").await?;
        wait_for("the echo after restart", Duration::from_secs(10), || {
            screen_contains(&app, &task_id, "ECHO: again")
        })
        .await?;

        // A crashing agent ends the run with its exit code.
        let crash = new_task(&app, &sandbox, &user, "Crash exits").await?;
        wait_for("the second banner", Duration::from_secs(30), || {
            screen_contains(&app, &crash, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&crash, b"/crash\r").await?;
        wait_for("the crash outcome", Duration::from_secs(15), || async {
            task(&app, &crash).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        let attempts = app.deps.store.attempts(&crash).await?;
        let last = attempts.last().ok_or("no attempt")?;
        assert_eq!(last.exit_code, Some(3));
        assert!(last.state.is_terminal());

        // Stop ends the first run and releases its process.
        app.stop_task_now(&task_id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &task_id)
                .await
                .is_ok_and(|t| matches!(t.status, TaskStatus::Stopped(_)))
        })
        .await?;
        let attempts = app.deps.store.attempts(&task_id).await?;
        assert!(attempts.iter().all(|a| a.state.is_terminal()));
        assert_eq!(attempts.len(), 1, "one attempt across the restart");
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
