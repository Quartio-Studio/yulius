//! The task follows its Claude Code session, however the session ends or stops (F 2026-10-10:
//! "tasks are not synchronised with the session — when I stop Claude, task status should
//! change"; SPEC-001 HRN-002, TSK-014, SPEC-002 §6.1), and a prompt a person types into the
//! session passes the scope check (TSK-013, HRN-006).
//!
//! End to end: supervisor → detached `yulius ptyhost` → the fake agent shaped like Claude Code
//! 2.1.292 (Ctrl+C handling, `SessionEnd`, the interrupt line in its transcript) running Claude
//! Code's hooks through the real `yulius hook` client against the internal listener; keys typed
//! through the terminal API a viewer uses; real worktrees.

use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use support::TestResult;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::intake::IntakeServices;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, TaskMode, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::intake::{ChoiceEvaluator, JevFailure};
use yulius_app::terminal::TerminalAttach;
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion};
use yulius_domain::task::{Activity, StopReason, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

type BoxError = Box<dyn std::error::Error>;

/// A server: the app and its internal listener, as `yulius serve` starts them. The agents'
/// hooks reach it there.
async fn start(data: &Path, fake: &Path, agent: FakeAgentOptions) -> Result<Arc<App>, BoxError> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let listener = socket.listen(64)?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let mut config = AppConfig::new(data.to_path_buf(), "https://yulius.test", &url, exe.clone());
    config.contained = false;
    let deps = Deps {
        store: Arc::new(SqliteStore::open(&data.join("yulius.db"))?),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
        github: Arc::new(support::FakeGitHub::default()),
        session_host: Arc::new(PtySessionHost::new(exe.clone())),
        harness: Arc::new(FakeAgentHarness {
            inner: HarnessAdapter::new(exe.clone(), url),
            fake: fake.to_path_buf(),
            options: agent,
            yulius_exe: exe,
        }),
        git: Arc::new(GitAdapter::new("git")),
        system: Arc::new(support::FixedSystem),
        browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
        usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
    };
    let app = App::new(config, deps);
    let internal = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
    Ok(app)
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
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

/// A superadmin, a sandbox whose repository is a clone of a bare origin, and a Claude account.
async fn seed(app: &App, data: &Path) -> Result<(SandboxId, UserRecord), BoxError> {
    let store = &app.deps.store;
    let user = UserRecord {
        id: UserId::parse("u-admin")?,
        github_login: None,
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
        id: SandboxId::parse("s-sync")?,
        slug: "sync".into(),
        repo_owner: "acme".into(),
        repo_name: "sync".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 44000,
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
            email: None,
            email_checked_at: None,
        })
        .await?;
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
    let repo = data
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("repo");
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
    Ok((sandbox.id, user))
}

async fn new_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, BoxError> {
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
            mode: TaskMode::Prompt,
            supertask: false,
        })
        .await?;
    Ok(record.id)
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

async fn task(app: &App, id: &TaskId) -> Result<TaskRecord, BoxError> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

async fn screen_contains(app: &App, id: &TaskId, needle: &str) -> bool {
    app.runtime(id).hub.screen_text().contains(needle)
}

async fn has_status(app: &App, id: &TaskId, status: TaskStatus) -> bool {
    task(app, id).await.is_ok_and(|t| t.status == status)
}

/// A viewer's terminal on the task, as the web terminal opens it.
async fn viewer(app: &App, ctx: &AuthContext, id: &TaskId) -> Result<TerminalAttach, BoxError> {
    Ok(app.terminal_open(ctx, id.as_str(), Some((120, 40))).await?)
}

/// Keys typed by the viewer (the terminal WebSocket's input frames).
async fn keys(
    app: &Arc<App>,
    ctx: &AuthContext,
    attach: &TerminalAttach,
    bytes: &[u8],
) -> TestResult {
    app.terminal_input(ctx, attach, bytes, false).await?;
    Ok(())
}

/// A started task with its fake agent ready, and its viewer.
async fn ready(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserRecord,
    title: &str,
) -> Result<(TaskId, AuthContext, TerminalAttach), BoxError> {
    let id = new_task(app, sandbox, &user.id, title).await?;
    wait_for("the fake agent", Duration::from_secs(30), || {
        screen_contains(app, &id, "FAKE-AGENT READY")
    })
    .await?;
    let ctx = AuthContext {
        user: user.clone(),
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let attach = viewer(app, &ctx, &id).await?;
    Ok((id, ctx, attach))
}

fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

fn data_dir() -> Result<(tempfile::TempDir, PathBuf), BoxError> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

fn agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        hooks: true,
        interrupts: true,
        ..FakeAgentOptions::default()
    }
}

/// Every way a Claude Code session stops moves its task at once, the same way each time:
/// Ctrl+C during a turn → the turn ends (`waiting`, it was stuck at `working`); Ctrl+C twice and
/// `/exit` → `closed`; a crash → `stopped_crashed`; a `kill` from outside → `stopped_killed`;
/// Stop in the UI → `stopping` at once, then `stopped_cancelled`. Viewers get each change.
#[test]
fn the_task_follows_its_session_however_it_stops() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = start(&data, &fake, agent()).await?;
        let (sandbox, user) = seed(&app, &data).await?;

        // Ctrl+C while the agent works: Claude Code runs no Stop hook; its interrupt line ends
        // the turn and the task waits for its person.
        let (id, ctx, attach) = ready(&app, &sandbox, &user, "Interrupt and leave").await?;
        keys(&app, &ctx, &attach, b"/work 30000 refactor the parser\r").await?;
        wait_for("the turn", Duration::from_secs(20), || async {
            screen_contains(&app, &id, "WORKING").await
                && task(&app, &id)
                    .await
                    .is_ok_and(|t| t.activity == Some(Activity::Working))
        })
        .await?;
        keys(&app, &ctx, &attach, b"\x03").await?;
        wait_for("the interrupt", Duration::from_secs(10), || {
            screen_contains(&app, &id, "INTERRUPTED")
        })
        .await?;
        wait_for(
            "Your turn after the interrupt",
            Duration::from_secs(5),
            || async {
                task(&app, &id).await.is_ok_and(|t| {
                    t.status == TaskStatus::Running && t.activity == Some(Activity::Waiting)
                })
            },
        )
        .await?;
        // Ctrl+C twice at the prompt leaves the session: closed, like /exit.
        let mut events = app.events.subscribe();
        keys(&app, &ctx, &attach, b"\x03").await?;
        wait_for("the first Ctrl+C", Duration::from_secs(10), || {
            screen_contains(&app, &id, "Press Ctrl-C again to exit")
        })
        .await?;
        keys(&app, &ctx, &attach, b"\x03").await?;
        wait_for("closed after Ctrl+C twice", Duration::from_secs(10), || {
            has_status(&app, &id, TaskStatus::Closed)
        })
        .await?;
        let last = app.deps.store.attempts(&id).await?.pop().ok_or("attempt")?;
        assert_eq!(last.exit_code, Some(0));
        // The store changes first and the event follows it, so wait for the event.
        let mut seen = Vec::new();
        wait_for("the closed event", Duration::from_secs(10), || {
            while let Ok(envelope) = events.try_recv() {
                if let UiEvent::TaskUpdated { task } = envelope.event
                    && task.id == id.as_str()
                {
                    seen.push(task.status);
                }
            }
            let closed = seen.last().map(String::as_str) == Some("closed");
            async move { closed }
        })
        .await?;
        app.terminal_leave(&attach).await;

        // /exit: SessionEnd, then a clean exit.
        let (id, ctx, attach) = ready(&app, &sandbox, &user, "Leave with exit").await?;
        keys(&app, &ctx, &attach, b"/exit\r").await?;
        wait_for("closed after /exit", Duration::from_secs(10), || {
            has_status(&app, &id, TaskStatus::Closed)
        })
        .await?;
        app.terminal_leave(&attach).await;

        // A crash.
        let (id, ctx, attach) = ready(&app, &sandbox, &user, "Crash").await?;
        keys(&app, &ctx, &attach, b"/crash\r").await?;
        wait_for("stopped after a crash", Duration::from_secs(10), || {
            has_status(&app, &id, TaskStatus::Stopped(StopReason::Crashed))
        })
        .await?;
        app.terminal_leave(&attach).await;

        // Killed from outside Yulius: SIGTERM (Claude Code exits 143 after SessionEnd) and SIGKILL.
        for (title, signal) in [
            ("Terminated", nix::sys::signal::Signal::SIGTERM),
            ("Killed", nix::sys::signal::Signal::SIGKILL),
        ] {
            let id = new_task(&app, &sandbox, &user.id, title).await?;
            wait_for("the fake agent", Duration::from_secs(30), || {
                screen_contains(&app, &id, "FAKE-AGENT READY")
            })
            .await?;
            let pid = app
                .deps
                .store
                .live_attempt(&id)
                .await?
                .and_then(|a| a.child_pid)
                .ok_or("child pid")?;
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(i32::try_from(pid)?), signal)?;
            wait_for("stopped after a kill", Duration::from_secs(10), || {
                has_status(&app, &id, TaskStatus::Stopped(StopReason::Killed))
            })
            .await?;
        }

        // Stop in the UI: `stopping` as soon as it is asked for, then stopped (Claude Code does
        // not exit on the first Ctrl+C; the escalation's SIGTERM ends it).
        let id = new_task(&app, &sandbox, &user.id, "Stopped by a person").await?;
        wait_for("the fake agent", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.stop_task_now(&id).await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Stopping);
        wait_for("stopped by the person", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::Stopped(StopReason::Cancelled))
        })
        .await?;
        assert!(
            app.deps.store.live_attempts().await?.is_empty(),
            "nothing left running"
        );
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A fake Jev: a typed message about dark mode is new scope; anything else is context.
struct FakeJev;

#[async_trait]
impl ChoiceEvaluator for FakeJev {
    async fn evaluate(
        &self,
        state: &str,
        question: &ChoiceQuestion<'_>,
        _deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure> {
        let message = state.split("NEW MESSAGE").nth(1).unwrap_or_default();
        let winner = if message.contains("dark mode") {
            "new_scope"
        } else {
            "context"
        };
        let p = 0.94;
        let rest = (1.0 - p) / (question.options.len() - 1) as f64;
        Ok(ChoiceAnswer {
            choice: winner.to_owned(),
            confidence: p,
            probabilities: question
                .options
                .iter()
                .map(|o| (o.key.to_owned(), if o.key == winner { p } else { rest }))
                .collect(),
        })
    }
}

/// TSK-013 / HRN-006 end to end ("they should catch messages sent to Claude as well"): a prompt
/// a person types into the task's Claude Code terminal goes through Claude Code's
/// `UserPromptSubmit` hook — the real `yulius hook` client — to the scope check; new scope is
/// held: the agent never runs it, and the task shows the held prompt's card. A prompt within the
/// task's scope passes.
#[test]
fn a_prompt_typed_into_the_terminal_passes_the_scope_check() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = start(&data, &fake, agent()).await?;
        assert!(app.set_intake(IntakeServices::new(
            Some(Arc::new(FakeJev) as Arc<dyn ChoiceEvaluator>),
            None,
        )));
        let (sandbox, user) = seed(&app, &data).await?;
        let (id, ctx, attach) = ready(&app, &sandbox, &user, "Fix the invoice totals").await?;

        keys(
            &app,
            &ctx,
            &attach,
            b"round the totals to cents in the summary\r",
        )
        .await?;
        wait_for("the in-scope prompt", Duration::from_secs(20), || {
            screen_contains(&app, &id, "ECHO: round the totals to cents in the summary")
        })
        .await?;

        keys(
            &app,
            &ctx,
            &attach,
            b"also add a dark mode to the settings page\r",
        )
        .await?;
        wait_for("the held prompt", Duration::from_secs(20), || {
            screen_contains(&app, &id, "PROMPT BLOCKED")
        })
        .await?;
        assert!(
            !screen_contains(&app, &id, "ECHO: also add a dark mode").await,
            "a held prompt never reaches the agent"
        );
        let summary = app.task_summary(&task(&app, &id).await?).await?;
        let held = summary
            .held_prompt
            .ok_or("the task shows the held prompt")?;
        assert_eq!(held.class, "new_scope");
        assert!(held.excerpt.contains("dark mode"));
        assert_eq!(held.percent, 94);

        app.terminal_leave(&attach).await;
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// F 2026-10-10 item 1(a) end to end: the status line command Yulius configures runs the real
/// `yulius hook` client; the session's plan usage (Claude Code reads it from its own API
/// responses — the usage endpoint refuses a `setup-token` sign-in) becomes its subscription's
/// windows, and the line shows the repository and branch (HRN-005).
#[test]
fn the_session_reports_its_usage_through_the_status_line() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let limits = data.join("rate-limits.json");
    let in_two_hours = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        + 7_200;
    std::fs::write(
        &limits,
        serde_json::json!({
            "five_hour": {"used_percentage": 37.5, "resets_at": in_two_hours},
            "seven_day": {"used_percentage": 12.0, "resets_at": in_two_hours + 86_400}
        })
        .to_string(),
    )?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = start(
            &data,
            &fake,
            FakeAgentOptions {
                rate_limits_file: Some(limits.clone()),
                ..agent()
            },
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = new_task(&app, &sandbox, &user.id, "Usage from the session").await?;
        wait_for("the status line", Duration::from_secs(30), || {
            screen_contains(&app, &id, "STATUS → sync git:(feat/")
        })
        .await?;
        let account = AccountId::parse("a-claude")?;
        wait_for("the usage reading", Duration::from_secs(10), || async {
            app.deps
                .store
                .account(&account)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.usage_json)
                .is_some_and(|u| u.contains("37.5"))
        })
        .await?;
        let view = yulius_app::accounts::account_usage(
            &app.deps.store.account(&account).await?.ok_or("account")?,
        );
        assert_eq!(view.primary.and_then(|w| w.used_percent), Some(37.5));
        assert_eq!(view.secondary.and_then(|w| w.used_percent), Some(12.0));
        assert!(view.observed_at.is_some());
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
