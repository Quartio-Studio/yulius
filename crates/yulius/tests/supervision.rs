//! End-to-end supervision with real processes: supervisor → detached `yulius ptyhost` → fake
//! agent, with real git worktrees. Covers launch, terminal I/O, secret env delivery, a server
//! restart (reconcile + reattach without respawn), crash exits and stop (SPEC-001 RUN-001..006,
//! AC-RUN-1..4).

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{FakeGitHub, TestResult};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, LaunchKind, SandboxRecord, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

fn build(data: &Path, fake: &Path) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_with(data, Some(fake))
}

/// `fake = None` launches the real agent CLIs (live checks only).
fn build_with(data: &Path, fake: Option<&Path>) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_full(data, fake, FakeAgentOptions::default(), 8)
}

fn build_full(
    data: &Path,
    fake: Option<&Path>,
    agent: FakeAgentOptions,
    max_live_sessions: u32,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_tuned(data, fake, agent, |c| {
        c.max_live_sessions = max_live_sessions
    })
}

fn build_tuned(
    data: &Path,
    fake: Option<&Path>,
    agent: FakeAgentOptions,
    tune: impl FnOnce(&mut AppConfig),
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let store = SqliteStore::open(&data.join("yulius.db"))?;
    let mut config = AppConfig::new(
        data.to_path_buf(),
        "https://yulius.test",
        "http://127.0.0.1:9",
        exe.clone(),
    );
    config.contained = false;
    tune(&mut config);
    let deps = Deps {
        store: Arc::new(store),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
        github: Arc::new(FakeGitHub::default()),
        session_host: Arc::new(PtySessionHost::new(exe.clone())),
        harness: match fake {
            Some(fake) => Arc::new(FakeAgentHarness::new(exe, fake.to_path_buf(), agent)),
            None => Arc::new(HarnessAdapter::new(exe, "http://127.0.0.1:9")),
        },
        git: Arc::new(GitAdapter::new("git")),
        system: Arc::new(support::FixedSystem),
        browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
        editor: Arc::new(yulius::wiring::editor::UnavailableEditor::default()),
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

/// Live: the real Claude Code TUI inside the real ptyhost with the launch plan Yulius generates
/// (isolated config dir, hooks, MCP config) reaches its prompt without first-run or folder-trust
/// screens. The credential is a dummy, so no provider request is made.
#[test]
#[ignore = "launches the real Claude Code CLI; set YULIUS_LIVE_CLAUDE=1"]
fn real_claude_tui_reaches_the_prompt_inside_ptyhost() -> TestResult {
    if std::env::var("YULIUS_LIVE_CLAUDE").as_deref() != Ok("1") {
        return Err("set YULIUS_LIVE_CLAUDE=1 to run this live check".into());
    }
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_with(&data, None)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Live TUI").await?;
        let ready = wait_for(
            "the Claude Code prompt",
            Duration::from_secs(60),
            || async {
                let screen = app.runtime(&id).hub.screen_text();
                screen.contains("Claude Code") && screen.contains("\u{276f}")
            },
        )
        .await;
        let screen = app.runtime(&id).hub.screen_text();
        eprintln!("--- screen ---\n{screen}\n--------------");
        ready?;
        assert!(
            !screen.contains("trust this folder"),
            "folder-trust prompt shown"
        );
        assert!(
            !screen.contains("Choose the text style"),
            "onboarding shown"
        );
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn add_account(
    app: &App,
    id: &str,
    priority: i32,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Anthropic,
            label: format!("Claude {id}"),
            credential: Some(
                app.deps
                    .crypto
                    .seal(b"sk-ant-oat01-second-test-credential")?,
            ),
            priority,
            enabled: true,
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(account)
}

/// AC-ROT-2 (reactive path): the provider reports a usage limit at the end of a turn; Yulius moves
/// the same session to the next account of the provider with a native resume.
#[test]
fn a_usage_limit_moves_the_session_to_the_next_account() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_account(&app, "a-claude-2", 1).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Rotation").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the limit", Duration::from_secs(15), || {
            screen_contains(&app, &id, "LIMIT REACHED")
        })
        .await?;
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
        assert_ne!(first.account_id, second.account_id, "another account");
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same session"
        );
        assert!(
            !app.deps.store.switches(&id).await?.is_empty(),
            "decision recorded"
        );
        wait_for("the resumed run", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-6: no capacity anywhere → the task waits for capacity instead of failing.
#[test]
fn without_capacity_the_task_waits() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "No capacity").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("waiting for capacity", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::WaitingCapacity)
        })
        .await?;
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-NFR-3: beyond the live-session limit tasks queue and start when capacity frees up.
#[test]
fn tasks_queue_at_the_session_limit_and_start_when_a_slot_frees() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 1)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let first = new_task(&app, &sandbox, &user, "First").await?;
        wait_for("the first run", Duration::from_secs(30), || {
            screen_contains(&app, &first, "FAKE-AGENT READY")
        })
        .await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        assert_eq!(task(&app, &second).await?.status, TaskStatus::Queued);
        app.stop_task_now(&first).await?;
        wait_for("the queued task to start", Duration::from_secs(60), || {
            screen_contains(&app, &second, "FAKE-AGENT READY")
        })
        .await?;
        assert_eq!(task(&app, &second).await?.status, TaskStatus::Running);
        app.stop_task_now(&second).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &second).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-TSK-3 and AC-COM-2/COM-005: an agent's normal exit closes its task; a message to a task
/// that has been waiting idle wakes it by typing a notice into its terminal.
#[test]
fn messages_wake_idle_tasks_and_a_normal_exit_closes_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, Some(&fake), FakeAgentOptions::default(), |c| {
            c.wake_idle_ms = 1_000;
        })?;
        app.start_background();
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let sender = new_task(&app, &sandbox, &user, "Sender").await?;
        let recipient = new_task(&app, &sandbox, &user, "Recipient").await?;
        for id in [&sender, &recipient] {
            wait_for("a banner", Duration::from_secs(30), || {
                screen_contains(&app, id, "FAKE-AGENT READY")
            })
            .await?;
        }
        // The recipient finished its turn (what its Stop hook reports) and nobody types.
        app.deps
            .store
            .set_activity(
                &recipient,
                Some(yulius_domain::task::Activity::Waiting),
                app.now(),
            )
            .await?;
        let ctx = yulius_app::coordination::AgentContext {
            attempt: app
                .deps
                .store
                .live_attempt(&sender)
                .await?
                .ok_or("sender attempt")?,
            task: task(&app, &sender).await?,
        };
        let number = task(&app, &recipient).await?.number;
        app.tool_message_send(
            &ctx,
            &serde_json::json!({"number": number, "kind": "question", "subject": "Schema",
                "body": "Does the invoices table keep the currency column?"}),
        )
        .await?;
        wait_for("the wake notice", Duration::from_secs(20), || {
            screen_contains(&app, &recipient, "ECHO: Yulius: you have new messages")
        })
        .await?;
        assert!(
            !screen_contains(&app, &sender, "you have new messages").await,
            "only the recipient is woken"
        );

        // A normal exit of the agent closes the task; no shell is spawned.
        app.send_input(&sender, b"/exit\r").await?;
        wait_for("the closed sender", Duration::from_secs(20), || async {
            task(&app, &sender)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Closed)
        })
        .await?;
        let attempts = app.deps.store.attempts(&sender).await?;
        assert_eq!(attempts.last().and_then(|a| a.exit_code), Some(0));
        app.stop_task_now(&recipient).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &recipient)
                .await
                .is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-2 (proactive path) on Codex: rate limits observed in the rollout reach the 99 %
/// threshold; at the next turn boundary the session moves to the next OpenAI account with a
/// native resume of the same Codex session.
#[test]
fn codex_usage_near_the_limit_rotates_at_the_turn_boundary() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: None,
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        for (id, priority) in [("o-1", 0), ("o-2", 1)] {
            app.deps
                .store
                .insert_account(&AccountRecord {
                    id: AccountId::parse(id)?,
                    provider: Provider::OpenAi,
                    label: format!("Codex {id}"),
                    credential: Some(
                        app.deps
                            .crypto
                            .seal(br#"{"auth_mode":"chatgpt","tokens":{"access_token":"fake"}}"#)?,
                    ),
                    priority,
                    enabled: true,
                    status: AccountStatus::Ok,
                    usage_json: None,
                    last_error: None,
                    created_at: Timestamp(1),
                    updated_at: Timestamp(1),
                })
                .await?;
        }
        seed_repo(&data, &sandbox)?;
        let record = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Codex rotation".into(),
                harness: Harness::Codex,
                title: Some("Codex rotation".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
            })
            .await?;
        let id = record.id;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        // Usage grows 10 % per turn and reaches 99 % on the tenth.
        for n in 1..=10 {
            app.send_input(&id, format!("turn {n}\r").as_bytes())
                .await?;
            let echo = format!("ECHO: turn {n}");
            wait_for("the echo", Duration::from_secs(15), || {
                screen_contains(&app, &id, &echo)
            })
            .await?;
        }
        wait_for("the proactive switch", Duration::from_secs(90), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() >= 2 && attempts.last().is_some_and(|a| !a.state.is_terminal())
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (first, second) = (&attempts[0], &attempts[1]);
        assert_eq!(first.harness, Harness::Codex);
        assert_eq!(second.harness, Harness::Codex, "same provider first");
        assert_ne!(first.account_id, second.account_id);
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert!(
            first.provider_session_id.is_some(),
            "Codex session discovered from the rollout"
        );
        assert_eq!(first.provider_session_id, second.provider_session_id);
        assert!(!app.deps.store.switches(&id).await?.is_empty());
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-4 (synthetic): with every Claude account exhausted and a Codex account available, the
/// Claude transcript is translated into a native Codex session that the next attempt resumes.
/// Live acceptance by the real Codex CLI is not covered here.
#[test]
fn provider_exhaustion_translates_the_session_to_codex() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        app.deps
            .store
            .insert_account(&AccountRecord {
                id: AccountId::parse("o-1")?,
                provider: Provider::OpenAi,
                label: "Codex".into(),
                credential: Some(
                    app.deps
                        .crypto
                        .seal(br#"{"auth_mode":"chatgpt","tokens":{"access_token":"fake"}}"#)?,
                ),
                priority: 0,
                enabled: true,
                status: AccountStatus::Ok,
                usage_json: None,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
            })
            .await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Translate").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"remember the blue widget\r").await?;
        wait_for("the limit", Duration::from_secs(15), || {
            screen_contains(&app, &id, "LIMIT REACHED")
        })
        .await?;
        wait_for("the Codex attempt", Duration::from_secs(90), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() >= 2 && attempts.last().is_some_and(|a| !a.state.is_terminal())
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let second = &attempts[1];
        assert_eq!(attempts[0].harness, Harness::ClaudeCode);
        assert_eq!(second.harness, Harness::Codex, "switched provider");
        assert_eq!(second.launch_kind, LaunchKind::Translated);
        // The synthesized rollout carries the Claude conversation.
        let session = second
            .provider_session_id
            .clone()
            .ok_or("codex session id")?;
        let sessions = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("codex/sessions");
        let rollout = yulius_harness::paths::find_codex_rollout(&sessions, &session)?
            .ok_or("synthesized rollout")?;
        let text = std::fs::read_to_string(rollout)?;
        assert!(
            text.contains("remember the blue widget"),
            "user turn translated"
        );
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-TSK-4: when Claude starts another session inside the same TUI (`/clear`, `/resume`), the
/// new session id is adopted into the task's lineage and becomes the attempt's session.
#[test]
fn a_session_change_inside_claude_is_adopted_into_the_task_lineage() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: None,
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Drift").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"first turn\r").await?;
        wait_for("the echo", Duration::from_secs(15), || {
            screen_contains(&app, &id, "ECHO: first turn")
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("attempt")?;
        let original = attempt.provider_session_id.clone().ok_or("session id")?;
        let worktree = PathBuf::from(task(&app, &id).await?.worktree_path.ok_or("worktree")?);
        let home = data.join("sandboxes").join(sandbox.as_str()).join("home");
        let transcript = app
            .deps
            .harness
            .claude_transcript(&home, &worktree, &original);
        let project = transcript.parent().ok_or("project dir")?;
        assert!(
            transcript.is_file(),
            "the fake agent writes the transcript Yulius watches"
        );

        // What `/clear` does: Claude continues in a new session file of the same project.
        let new_session = "5f0c2d4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f";
        let record = serde_json::json!({
            "type": "user", "sessionId": new_session, "cwd": worktree.to_string_lossy(),
            "uuid": "drift-1", "parentUuid": null, "timestamp": "2026-10-07T12:00:00.000Z",
            "message": {"role": "user", "content": "after clear"}, "version": "2.1.292",
        });
        std::fs::write(
            project.join(format!("{new_session}.jsonl")),
            format!("{record}\n"),
        )?;

        wait_for("the adopted session", Duration::from_secs(20), || async {
            app.deps
                .store
                .live_attempt(&id)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.provider_session_id)
                .as_deref()
                == Some(new_session)
        })
        .await?;
        let lineage: Vec<String> = app
            .deps
            .store
            .provider_sessions(&id)
            .await?
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        assert!(
            lineage.contains(&original) && lineage.contains(&new_session.to_owned()),
            "{lineage:?}"
        );
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "same attempt, no respawn"
        );
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-NFR-6 / PRJ-007 (Linux): a contained task sees its own sandbox but neither another
/// sandbox's files nor the Yulius data directory. Skipped with a message where bubblewrap cannot
/// create namespaces.
#[cfg(target_os = "linux")]
#[test]
fn contained_tasks_cannot_read_other_sandboxes_or_the_data_directory() -> TestResult {
    let probe = Command::new("bwrap")
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
    match probe {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            eprintln!(
                "SKIPPED contained_tasks_cannot_read_other_sandboxes: bwrap cannot create namespaces: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return Ok(());
        }
        Err(err) => {
            eprintln!(
                "SKIPPED contained_tasks_cannot_read_other_sandboxes: bwrap not runnable: {err}"
            );
            return Ok(());
        }
    }
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let fake_dir = fake.parent().ok_or("fake-agent dir")?.to_path_buf();
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, Some(&fake), FakeAgentOptions::default(), move |c| {
            c.contained = true;
            c.contained_readonly = vec![fake_dir];
        })?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let other = data.join("sandboxes").join("s-other").join("repo");
        std::fs::create_dir_all(&other)?;
        std::fs::write(other.join("secret.txt"), "another sandbox's secret\n")?;
        let id = new_task(&app, &sandbox, &user, "Contained").await?;
        wait_for("the contained banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let worktree = PathBuf::from(task(&app, &id).await?.worktree_path.ok_or("worktree")?);
        let probes = [
            (worktree.join("README.md"), "READ README.md OK"),
            (other.join("secret.txt"), "READ secret.txt ERR"),
            (data.join("yulius.db"), "READ yulius.db ERR"),
        ];
        for (path, expected) in probes {
            app.send_input(&id, format!("/read {}\r", path.display()).as_bytes())
                .await?;
            let screen = app.runtime(&id).hub.screen_text();
            let expected = expected.to_owned();
            let ok = wait_for(&expected, Duration::from_secs(10), || {
                screen_contains(&app, &id, &expected)
            })
            .await;
            if ok.is_err() {
                eprintln!(
                    "screen before: {screen}\nscreen now: {}",
                    app.runtime(&id).hub.screen_text()
                );
            }
            ok?;
        }
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
