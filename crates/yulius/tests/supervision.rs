//! End-to-end supervision with real processes: supervisor → detached `yulius ptyhost` → fake
//! agent, with real git worktrees. Covers launch, terminal I/O, secret env delivery, a server
//! restart (reconcile + reattach without respawn), crash exits and stop (SPEC-001 RUN-001..006,
//! AC-RUN-1..4).

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::agents::{FakeAgentHarness, FakeAgentOptions, limit_resume_problems};
use support::{FakeGitHub, TestResult};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, LaunchKind, SandboxRecord, Secret, TaskOrigin, TaskRecord,
    UserRecord,
};
use yulius_app::ports::usage::UsageProbe;
use yulius_app::supervisor::RunStart;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};
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
    build_custom(
        data,
        fake,
        agent,
        tune,
        Arc::new(yulius_app::ports::usage::NoUsageProbe),
    )
}

fn build_custom(
    data: &Path,
    fake: Option<&Path>,
    agent: FakeAgentOptions,
    tune: impl FnOnce(&mut AppConfig),
    usage: Arc<dyn UsageProbe>,
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
        usage,
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
        github_id: Some(4242),
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
            mode: yulius_app::model::TaskMode::Prompt,
            supertask: false,
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
        // F 2026-10-11 (AC-GIT-3): commits are the creator's, with the address GitHub links to
        // their account; the run's `gh` shim is in place.
        let email = Command::new("git")
            .args(["config", "user.email"])
            .current_dir(&worktree)
            .output()?;
        assert_eq!(
            String::from_utf8_lossy(&email.stdout).trim(),
            "4242+admin@users.noreply.github.com"
        );
        let shim = std::fs::read_to_string(data.join("runtime/gh/gh"))?;
        assert!(shim.starts_with("#!/bin/sh\n# Yulius gh shim"), "{shim}");
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

async fn add_account(app: &App, id: &str) -> Result<AccountId, Box<dyn std::error::Error>> {
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
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(account)
}

/// AC-TSK-9: a goal-mode task gets its prompt as `/goal <prompt>` typed into the session once
/// the TUI is ready, not as a first message.
#[test]
fn goal_mode_types_the_goal_once_the_session_is_ready() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let record = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Ship the widget\nwith tests".into(),
                harness: Harness::ClaudeCode,
                title: Some("Goal".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
                mode: yulius_app::model::TaskMode::Goal,
                supertask: false,
            })
            .await?;
        let id = record.id;
        wait_for("the goal command", Duration::from_secs(60), || {
            screen_contains(&app, &id, "ECHO: /goal Ship the widget with tests")
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

/// AC-ROT-2 (reactive path): the provider reports a usage limit at the end of a turn; Yulius moves
/// the same session to the next account of the provider with a native resume. The limited account
/// then shows its window full until the reset, and new tasks start on the other account (#26,
/// #27).
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
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_account(&app, "a-claude-2").await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Rotation").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        // The switch may replace the screen before a poll sees the limit message.
        wait_for(
            "the limit or the switch",
            Duration::from_secs(30),
            || async {
                screen_contains(&app, &id, "LIMIT REACHED").await
                    || app
                        .deps
                        .store
                        .attempts(&id)
                        .await
                        .is_ok_and(|a| a.len() >= 2)
            },
        )
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
        // SPEC-034: the transcript steps back over the limit and Claude Code continues the turn
        // itself; nothing like "Continue." is sent.
        let problems = limit_resume_problems(Path::new(&second.run_dir), &data);
        assert!(problems.is_empty(), "{problems:?}");
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
        let limited = app
            .deps
            .store
            .account(first.account_id.as_ref().ok_or("first account")?)
            .await?
            .ok_or("the first account")?;
        assert_eq!(limited.status, AccountStatus::Limited);
        let usage =
            yulius_app::usage_json::from_json(limited.usage_json.as_deref().unwrap_or("{}"));
        let window = usage
            .primary
            .ok_or("the session window from the observed limit")?;
        assert_eq!(window.used_percent, Some(100.0));
        assert_eq!(window.window_minutes, Some(300));
        assert!(window.resets_at.is_some(), "the reset time of the limit");
        // Routing skips the account until its reset: a new task starts on the other one.
        let next = new_task(&app, &sandbox, &user, "After the limit").await?;
        wait_for("the next task's run", Duration::from_secs(30), || async {
            !app.deps
                .store
                .attempts(&next)
                .await
                .unwrap_or_default()
                .is_empty()
        })
        .await?;
        let started = app.deps.store.attempts(&next).await?;
        assert_eq!(
            started[0].account_id, second.account_id,
            "a new task does not start on the limited account"
        );
        for live in [&id, &next] {
            app.stop_task_now(live).await?;
            wait_for("the stop", Duration::from_secs(30), || async {
                task(&app, live).await.is_ok_and(|t| !t.status.is_live())
            })
            .await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-TSK-7: a closed task continues by resuming the same provider session. AC-ROT-5: a manual
/// switch to a chosen account resumes it there with the task's model and effort. These are the
/// runs `POST /continue` and `POST /switch` start after their access checks (see `access.rs`).
#[test]
fn a_closed_task_continues_its_session_and_a_manual_switch_keeps_model_and_effort() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: None,
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_account(&app, "a-claude-2").await?;
        seed_repo(&data, &sandbox)?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Continue me: answer with an echo".into(),
                harness: Harness::ClaudeCode,
                title: Some("Continue me".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: Some("claude-opus-4-1".into()),
                effort: Some("high".into()),
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
            })
            .await?
            .id;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the closed task", Duration::from_secs(20), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Closed)
        })
        .await?;
        let running = |count: usize| {
            let app = app.clone();
            let id = id.clone();
            move || {
                let app = app.clone();
                let id = id.clone();
                async move {
                    let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                    attempts.len() == count
                        && attempts.last().is_some_and(|a| !a.state.is_terminal())
                        && task(&app, &id)
                            .await
                            .is_ok_and(|t| t.status == TaskStatus::Running)
                }
            }
        };

        // Continue: the same provider session resumes (TSK-007).
        app.start_run(&id, RunStart::Resume { prompt: None }, None)
            .await?;
        wait_for("the continued run", Duration::from_secs(30), running(2)).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts[1].launch_kind, LaunchKind::Resume);
        assert!(attempts[0].provider_session_id.is_some());
        assert_eq!(
            attempts[0].provider_session_id, attempts[1].provider_session_id,
            "the same session"
        );

        // Manual switch to the other account: resumed there (ROT-007).
        let other = if attempts[1].account_id.as_ref().map(AccountId::as_str) == Some("a-claude") {
            "a-claude-2"
        } else {
            "a-claude"
        };
        app.switch_task_now(
            &id,
            Some(AccountId::parse(other)?),
            "manual",
            Some("Continue the task exactly where you left off.".into()),
        )
        .await?;
        wait_for("the switched run", Duration::from_secs(90), running(3)).await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[2].account_id.as_ref().map(AccountId::as_str),
            Some(other)
        );
        assert_eq!(attempts[2].launch_kind, LaunchKind::Resume);
        assert_eq!(
            attempts[2].provider_session_id,
            attempts[0].provider_session_id
        );
        // Every launch asked for the task's model and effort (AC-ROT-5).
        for attempt in &attempts {
            let launch =
                std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
            assert!(
                launch.contains("model=Some(\"claude-opus-4-1\")")
                    && launch.contains("effort=Some(\"high\")"),
                "attempt {}: {launch}",
                attempt.seq
            );
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

/// A sandbox that refuses a start (here: over its disk quota).
struct RefusingIsolation(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl yulius_app::ports::isolation::SandboxIsolation for RefusingIsolation {
    async fn prepare(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(yulius_app::error::AppError::Conflict(
                "this sandbox uses 112.2 GB of its 100.0 GB disk quota".into(),
            ));
        }
        Ok(None)
    }

    fn release(&self, _: &SandboxId) {}
}

/// ROT-014: a switch that could not start its new session leaves the running one alone and says
/// why; once the cause is gone the switch works.
#[test]
fn a_switch_that_cannot_start_leaves_the_running_session_alone() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_account(&app, "a-claude-2").await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Over quota").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let refusing = Arc::new(RefusingIsolation(std::sync::atomic::AtomicBool::new(true)));
        assert!(app.set_isolation(refusing.clone()));
        let before = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("live attempt")?;
        let mut viewer = app.runtime(&id).hub.subscribe();
        let other = AccountId::parse("a-claude-2")?;
        let refused = app
            .switch_task_now(&id, Some(other.clone()), "manual", None)
            .await;
        assert!(
            matches!(&refused, Err(e) if e.public_message().contains("112.2 GB")),
            "{refused:?}"
        );
        let after = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("still live")?;
        assert_eq!(after.id, before.id, "the session was not ended");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let mut notice = None;
        while let Ok(frame) = viewer.rx.try_recv() {
            if let yulius_app::hub::HubFrame::Notice(text) = frame {
                notice = Some(text);
            }
        }
        assert!(
            notice.is_some_and(|n| n.contains("112.2 GB")),
            "the reason reaches the terminal"
        );
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(switches.len(), 1);
        assert_eq!(switches[0].outcome, "refused");
        assert!(switches[0].inputs.contains("112.2 GB"));
        // A repeated automatic refusal inside the cooldown writes no second record or notice.
        let again = app
            .switch_task_now(&id, Some(other.clone()), "Proactive", None)
            .await;
        assert!(
            matches!(&again, Err(e) if e.public_message().contains("112.2 GB")),
            "{again:?}"
        );
        assert_eq!(app.deps.store.switches(&id).await?.len(), 1);
        // Space is back: the same switch now moves the session.
        refusing.0.store(false, std::sync::atomic::Ordering::SeqCst);
        app.switch_task_now(&id, Some(other), "manual", None)
            .await?;
        wait_for("the switched run", Duration::from_secs(90), || async {
            app.deps
                .store
                .attempts(&id)
                .await
                .is_ok_and(|a| a.len() == 2 && a.last().is_some_and(|a| !a.state.is_terminal()))
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

/// AC-DSC-5: a task worktree refuses the agent's pushes to the integration and production
/// branches (the agent's own Git runs the Yulius pre-push guard) and lets the task branch go.
/// AC-UI-3: status transitions reach the UI as task events within 2 s.
#[test]
fn task_worktrees_guard_protected_branches_and_status_changes_reach_the_ui() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build(&data, &fake)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let mut events = app.events.subscribe();
        let id = new_task(&app, &sandbox, &user, "Guarded").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let mut seen: Vec<String> = Vec::new();
        // Collects this task's statuses from the UI events; a lagging receiver skips ahead.
        let drain =
            |events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
             seen: &mut Vec<String>| {
                loop {
                    match events.try_recv() {
                        Ok(envelope) => {
                            if let yulius_contracts::events::UiEvent::TaskUpdated { task } =
                                envelope.event
                                && task.id == id.as_str()
                            {
                                seen.push(task.status);
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            };
        // The banner can come before the supervisor records the run as running: wait for it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !seen.iter().any(|s| s == "running") && Instant::now() < deadline {
            drain(&mut events, &mut seen);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            seen.iter().any(|s| s == "running"),
            "running reached the UI: {seen:?}"
        );

        // The agent's own Git, in the task worktree.
        let record = task(&app, &id).await?;
        let worktree = PathBuf::from(record.worktree_path.ok_or("worktree")?);
        let branch = record.branch.ok_or("branch")?;
        let agent_git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&worktree)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_AUTHOR_NAME", "Agent")
                .env("GIT_AUTHOR_EMAIL", "agent@example.invalid")
                .env("GIT_COMMITTER_NAME", "Agent")
                .env("GIT_COMMITTER_EMAIL", "agent@example.invalid")
                .output()
        };
        // The agent's work: without a new commit a push is "up to date" and runs no hook.
        std::fs::write(worktree.join("guarded.txt"), "agent work\n")?;
        for args in [
            &["add", "guarded.txt"][..],
            &["commit", "--quiet", "-m", "agent work"],
        ] {
            let done = agent_git(args)?;
            assert!(
                done.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&done.stderr)
            );
        }
        let push = |refspec: String| agent_git(&["push", "--quiet", "origin", &refspec]);
        for protected in ["main", "dev"] {
            let refused = push(format!("HEAD:refs/heads/{protected}"))?;
            assert!(
                !refused.status.success(),
                "push to {protected} was not refused"
            );
            let stderr = String::from_utf8_lossy(&refused.stderr);
            assert!(
                stderr.contains(&format!("protected branch {protected}")),
                "{stderr}"
            );
        }
        let allowed = push(format!("HEAD:refs/heads/{branch}"))?;
        assert!(
            allowed.status.success(),
            "the task branch pushes: {}",
            String::from_utf8_lossy(&allowed.stderr)
        );

        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        let stopped = task(&app, &id).await?.status.as_str().to_owned();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !seen.contains(&stopped) && Instant::now() < deadline {
            drain(&mut events, &mut seen);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            seen.contains(&stopped),
            "{stopped} reached the UI within 2 s: {seen:?}"
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// #31: a run's tokens, turns and active time are stored while it goes, not only when it ends,
/// so the dashboard counts work in progress.
#[test]
fn live_runs_report_metrics_before_they_end() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: None,
            ..FakeAgentOptions::default()
        };
        let app = build_full(&data, Some(&fake), agent, 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Live metrics").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("live metrics", Duration::from_secs(30), || async {
            app.deps.store.task_metrics(&id).await.is_ok_and(|m| {
                m.iter()
                    .any(|r| r.output_tokens.is_some_and(|t| t > 0) && r.turns == Some(1))
            })
        })
        .await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Running,
            "saved while the run goes on"
        );
        let attempts = app.deps.store.attempts(&id).await?;
        assert!(
            attempts.iter().all(|a| !a.state.is_terminal()),
            "the run has not ended"
        );
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        let finals = app.deps.store.task_metrics(&id).await?;
        assert_eq!(finals.len(), 1, "one row per run: live saves are upserts");
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-6: no capacity anywhere → the task waits for capacity instead of failing, and says
/// when it resumes. The fake writes Claude Code 2.1.292's usage-limit record (F 2026-10-09).
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
            ..FakeAgentOptions::default()
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
        let waiting = app.task_summary(&task(&app, &id).await?).await?;
        assert_eq!(waiting.status, "waiting_capacity");
        let resumes_at = waiting.resumes_at.ok_or("no resume time while waiting")?;
        assert!(resumes_at.ends_with('Z'), "RFC 3339 in UTC: {resumes_at}");
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

/// SES-101: a Done task keeps its idle session but no longer holds a slot, so a queued task
/// starts without the Done one being stopped (F 2026-10-12).
#[test]
fn a_done_task_frees_its_slot_for_a_queued_task() -> TestResult {
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
        // Still working: the queue waits.
        app.drain_queue().await;
        assert_eq!(task(&app, &second).await?.status, TaskStatus::Queued);
        app.deps
            .store
            .set_activity(&first, Some(Activity::Done), app.now())
            .await?;
        app.drain_queue().await;
        wait_for("the queued task to start", Duration::from_secs(60), || {
            screen_contains(&app, &second, "FAKE-AGENT READY")
        })
        .await?;
        let done = task(&app, &first).await?;
        assert_eq!(
            done.status,
            TaskStatus::Running,
            "the Done task keeps its session"
        );
        for id in [&first, &second] {
            app.stop_task_now(id).await?;
        }
        for id in [&first, &second] {
            wait_for("the stop", Duration::from_secs(30), || async {
                task(&app, id).await.is_ok_and(|t| !t.status.is_live())
            })
            .await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-057 SUB-001, SUB-003: a Done task no longer holds its subscription. A working task counts
/// on it and keeps it from being removed; once the task is Done the subscription shows no task,
/// and removing it closes the Done session (task #48).
#[test]
fn a_done_task_lets_its_subscription_go() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 1)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let id = new_task(&app, &sandbox, &user, "Finished").await?;
        wait_for("the run", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let in_use = |app: Arc<App>, ctx: AuthContext| async move {
            let accounts = app.list_accounts(&ctx).await?;
            let status = app.status(&ctx).await?;
            let capacity = app.capacity_view(&ctx).await?;
            Ok::<_, Box<dyn std::error::Error>>((
                accounts[0].in_use_by,
                status.accounts[0].in_use_by,
                capacity.accounts[0].in_use_by,
            ))
        };
        // Working: the subscription is in use and cannot be removed.
        assert_eq!(
            in_use(app.clone(), ctx.clone()).await?,
            (1, Some(1), Some(1))
        );
        assert!(matches!(
            app.delete_account(&ctx, "a-claude").await,
            Err(yulius_app::error::AppError::Conflict(_))
        ));
        // Done: the idle session stays, but no longer counts on the subscription.
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        assert_eq!(
            in_use(app.clone(), ctx.clone()).await?,
            (0, Some(0), Some(0))
        );
        // Removing the subscription closes the Done session, as archiving does.
        app.delete_account(&ctx, "a-claude").await?;
        assert!(
            app.deps
                .store
                .account(&AccountId::parse("a-claude")?)
                .await?
                .is_none()
        );
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Closed);
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-057 SUB-003: removing a subscription is refused while a task works on it, and then leaves
/// its Done neighbour alone; once every task on it is Done, removal closes all of them.
#[test]
fn removing_a_subscription_spares_working_tasks_and_closes_every_done_one() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 2)?;
        let (sandbox, user) = seed_records(&app).await?;
        seed_repo(&data, &sandbox)?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let first = new_task(&app, &sandbox, &user, "First").await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        for id in [&first, &second] {
            wait_for("a run", Duration::from_secs(30), || {
                screen_contains(&app, id, "FAKE-AGENT READY")
            })
            .await?;
        }
        let done = |id: TaskId| {
            let app = app.clone();
            async move {
                app.deps
                    .store
                    .set_activity(&id, Some(Activity::Done), app.now())
                    .await
            }
        };
        done(first.clone()).await?;
        // The second task still works: refused, and the Done one keeps its session.
        assert!(matches!(
            app.delete_account(&ctx, "a-claude").await,
            Err(yulius_app::error::AppError::Conflict(_))
        ));
        assert_eq!(task(&app, &first).await?.status, TaskStatus::Running);
        assert!(app.deps.store.live_attempt(&first).await?.is_some());
        // Both Done: removal closes both and deletes the subscription.
        done(second.clone()).await?;
        app.delete_account(&ctx, "a-claude").await?;
        assert!(
            app.deps
                .store
                .account(&AccountId::parse("a-claude")?)
                .await?
                .is_none()
        );
        for id in [&first, &second] {
            assert_eq!(task(&app, id).await?.status, TaskStatus::Closed);
            assert!(app.deps.store.live_attempt(id).await?.is_none());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-TSK-3: an agent's normal exit closes its task; another task of the sandbox keeps running.
#[test]
fn a_normal_exit_closes_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_tuned(&data, Some(&fake), FakeAgentOptions::default(), |_| {})?;
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

/// The first account (seeded by [`seed_records`]) is at 98 % of its five-hour window, the pool's
/// switch threshold (F 2026-10-10); others report no usage.
struct FirstAccountNearLimit;

#[async_trait::async_trait]
impl UsageProbe for FirstAccountNearLimit {
    async fn fetch(
        &self,
        _provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, yulius_app::ports::usage::UsageFailure> {
        if credential.expose() != "sk-ant-oat01-test-credential" {
            return Ok(AccountUsage::default());
        }
        Ok(AccountUsage {
            primary: Some(UsageWindow {
                used_percent: Some(98.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(4_000_000_000_000)),
            }),
            ..AccountUsage::default()
        })
    }
}

/// AC-ROT-2 (proactive path): refreshed usage puts the task's account at the 98 % threshold; at
/// the next turn boundary the session moves to the next Claude account with a native resume of
/// the same session.
#[test]
fn usage_near_the_limit_rotates_at_the_turn_boundary() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: None,
            ..FakeAgentOptions::default()
        };
        let app = build_custom(
            &data,
            Some(&fake),
            agent,
            |_| {},
            Arc::new(FirstAccountNearLimit),
        )?;
        let (sandbox, user) = seed_records(&app).await?;
        add_account(&app, "a-claude-2").await?;
        seed_repo(&data, &sandbox)?;
        let id = new_task(&app, &sandbox, &user, "Proactive").await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let first_account = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .and_then(|a| a.account_id)
            .ok_or("first account")?;
        assert_eq!(first_account.as_str(), "a-claude");
        // An administrator refreshes the account's usage: it is at the threshold.
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        app.refresh_account_usage(&ctx, "a-claude").await?;
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "no switch in the middle of a turn"
        );
        // The next turn ends: the session moves on.
        app.send_input(&id, b"next turn\r").await?;
        wait_for("the proactive switch", Duration::from_secs(90), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() >= 2 && attempts.last().is_some_and(|a| !a.state.is_terminal())
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let (first, second) = (&attempts[0], &attempts[1]);
        assert_eq!(
            second.account_id.as_ref().map(AccountId::as_str),
            Some("a-claude-2")
        );
        assert_eq!(second.harness, Harness::ClaudeCode);
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(first.provider_session_id, second.provider_session_id);
        assert!(!app.deps.store.switches(&id).await?.is_empty());
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
            ..FakeAgentOptions::default()
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
        // Lineage records are written by the watcher; wait until both sessions are there.
        let expected = [original.clone(), new_session.to_owned()];
        wait_for(
            "both sessions in the lineage",
            Duration::from_secs(20),
            || async {
                app.deps
                    .store
                    .provider_sessions(&id)
                    .await
                    .is_ok_and(|sessions| {
                        expected
                            .iter()
                            .all(|want| sessions.iter().any(|s| &s.session_id == want))
                    })
            },
        )
        .await?;
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
        let run_dir = PathBuf::from(
            app.deps
                .store
                .live_attempt(&id)
                .await?
                .ok_or("attempt")?
                .run_dir,
        );
        let probes = [
            (worktree.join("README.md"), "READ README.md OK"),
            // The generated Claude settings are visible; the rest of the run directory is not.
            (
                run_dir.join("claude-settings.json"),
                "READ claude-settings.json OK",
            ),
            (run_dir.join("launch.json"), "READ launch.json ERR"),
            (PathBuf::from("/etc/resolv.conf"), "READ resolv.conf OK"),
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
