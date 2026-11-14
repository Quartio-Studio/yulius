//! A task whose goal Claude Code records as achieved is Done (SPEC-001 TSK-014, AC-TSK-13).
//!
//! End to end: supervisor → detached `yulius ptyhost` → fake agent shaped like Claude Code
//! 2.1.292 (goal records in its transcript, Bash-tool background shells) running Claude Code's
//! hooks through the real `yulius hook` client against the internal listener; real worktrees; a
//! server restart that replays the transcripts.

use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::TestResult;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::error::AppResult;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, TaskMode, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::harness::HarnessPort;
use yulius_app::ports::session::{
    AttachedRun, BackgroundProbe, LaunchPlan, RunInspection, SessionHost, SpawnedRun,
};
use yulius_app::ports::store::TaskStore;
use yulius_app::supervisor::RunStart;
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, StopReason, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

type BoxError = Box<dyn std::error::Error>;

/// A server: the app and its internal listener on `port` (0 = any free port), as `yulius serve`
/// would start it. The agents' hooks reach it at that port.
async fn start(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
) -> Result<(Arc<App>, u16), BoxError> {
    start_on(data, fake, agent, port, None, None, false, None).await
}

/// A session host that connects to a run only once the agent's SessionStart hook was answered, so
/// the hook lands before the supervisor watches the run, as on a busy host (#243). Bounded: without
/// a hook the attach goes ahead after 30 s.
struct AttachAfterSessionStart {
    inner: PtySessionHost,
    session_started: tokio::sync::watch::Receiver<bool>,
}

#[async_trait::async_trait]
impl SessionHost for AttachAfterSessionStart {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        self.inner
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        let mut started = self.session_started.clone();
        let _ = tokio::time::timeout(Duration::from_secs(30), started.wait_for(|s| *s)).await;
        self.inner.attach(run_dir, token).await
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        self.inner.background_shells(child_pid)
    }
}

/// [`start`], and with `attach_late` the run is attached only after its SessionStart hook.
async fn start_with(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    attach_late: bool,
) -> Result<(Arc<App>, u16), BoxError> {
    start_on(data, fake, agent, port, None, None, attach_late, None).await
}

/// [`start`] on `host` (default: the real one, attached late with `attach_late`), with the stop
/// and close escalation shortened to `escalation_ms` and at most `sessions` live sessions.
#[allow(clippy::too_many_arguments)]
async fn start_on(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    host: Option<Arc<dyn SessionHost>>,
    escalation_ms: Option<u64>,
    attach_late: bool,
    sessions: Option<u32>,
) -> Result<(Arc<App>, u16), BoxError> {
    start_holding(
        data,
        fake,
        agent,
        port,
        host,
        (escalation_ms, None),
        attach_late,
        sessions,
        None,
        None,
    )
    .await
}

/// A `/goal` prompt hook the test server handles only after `hold` and, within 30 s, once a Stop
/// that arrived meanwhile was answered (not held): the first one once `armed`. `stop_handled`
/// says that Stop was answered first (the late path), `handled` when the prompt was handled.
#[derive(Clone)]
struct LateHook {
    hold: Duration,
    armed: Arc<std::sync::atomic::AtomicBool>,
    holding: Arc<std::sync::atomic::AtomicBool>,
    stop_handled: tokio::sync::watch::Sender<bool>,
    handled: tokio::sync::watch::Sender<bool>,
}

/// What a test sees and decides of the turn hooks at the internal listener (SPEC-453): each Stop
/// Yulius answered (not held) is reported on `stop_answered`, and while `prompts_lost` is set a
/// `UserPromptSubmit` never reaches Yulius: its client fails open, as on an unreachable server.
#[derive(Clone)]
struct TurnHooks {
    stop_answered: std::sync::mpsc::Sender<()>,
    prompts_lost: Arc<std::sync::atomic::AtomicBool>,
}

/// [`start_on`], and with `late_goal` the server sees a `/goal` prompt hook (a resumed session's
/// typed goal) only after its hold, then handles it to its end, as a loaded host does once the
/// hook client gave up waiting (SPEC-346); with `turn_hooks` the test follows the turn hooks.
/// The pair shortens the stop and close escalation, and a switch's wait for its old session.
#[allow(clippy::too_many_arguments)]
async fn start_holding(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    host: Option<Arc<dyn SessionHost>>,
    (escalation_ms, switch_end_ms): (Option<u64>, Option<u64>),
    attach_late: bool,
    sessions: Option<u32>,
    late_goal: Option<LateHook>,
    turn_hooks: Option<TurnHooks>,
) -> Result<(Arc<App>, u16), BoxError> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    let listener = socket.listen(64)?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let mut config = AppConfig::new(data.to_path_buf(), "https://yulius.test", &url, exe.clone());
    let (session_started, session_started_rx) = tokio::sync::watch::channel(false);
    config.contained = false;
    if let Some(ms) = escalation_ms {
        config.stop_escalation_ms = ms;
    }
    if let Some(ms) = switch_end_ms {
        config.switch_end_ms = ms;
    }
    if let Some(sessions) = sessions {
        config.max_live_sessions = sessions;
    }
    let deps = Deps {
        store: Arc::new(SqliteStore::open(&data.join("yulius.db"))?),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
        github: Arc::new({
            let github = support::FakeGitHub::default();
            // Bob writes the fixture's repository (a hand-over during a launch, SPEC-248).
            github.set_perm("bob", "acme/done", support::perms("write"));
            github
        }),
        session_host: if let Some(host) = host {
            host
        } else if attach_late {
            Arc::new(AttachAfterSessionStart {
                inner: PtySessionHost::new(exe.clone()),
                session_started: session_started_rx,
            })
        } else {
            Arc::new(PtySessionHost::new(exe.clone()))
        },
        harness: Arc::new(FakeAgentHarness {
            inner: HarnessAdapter::new(exe.clone(), url),
            fake: fake.to_path_buf(),
            options: agent,
            yulius_exe: exe,
        }),
        git: Arc::new(crate::support::confined_git()),
        system: Arc::new(support::FixedSystem),
        browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
        usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
    };
    let app = App::new(config, deps);
    // Says when a SessionStart hook was answered (the hook client waits for that answer).
    let session_start_answered = axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let session_started = session_started.clone();
            async move {
                let start = request.uri().path().ends_with("/SessionStart");
                let response = next.run(request).await;
                if start {
                    session_started.send_replace(true);
                }
                response
            }
        },
    );
    let goal_held = axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let late_goal = late_goal.clone();
            async move {
                use axum::response::IntoResponse as _;
                use std::sync::atomic::Ordering::SeqCst;
                let Some(late) = late_goal else {
                    return next.run(request).await;
                };
                let path = request.uri().path();
                if path.ends_with("/Stop") && late.holding.load(SeqCst) {
                    // To its end even if its client hangs up, then the held prompt goes on.
                    let stop = tokio::spawn(async move {
                        let response = next.run(request).await;
                        // A held Stop (202) decided nothing about the turn.
                        if response.status() == axum::http::StatusCode::OK {
                            late.stop_handled.send_replace(true);
                        }
                        response
                    });
                    return stop.await.unwrap_or_else(|_| {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    });
                }
                if !path.ends_with("/UserPromptSubmit") {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1 << 20)
                    .await
                    .unwrap_or_default();
                let goal = String::from_utf8_lossy(&bytes).contains("\"prompt\":\"/goal ");
                let request =
                    axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
                if !goal || !late.armed.swap(false, SeqCst) {
                    return next.run(request).await;
                }
                late.holding.store(true, SeqCst);
                // Its own task: the client hanging up does not cancel it (ADR-0115).
                let handling = tokio::spawn(async move {
                    tokio::time::sleep(late.hold).await;
                    let mut stopped = late.stop_handled.subscribe();
                    let _ = tokio::time::timeout(Duration::from_secs(30), stopped.wait_for(|s| *s))
                        .await;
                    late.holding.store(false, SeqCst);
                    let response = next.run(request).await;
                    late.handled.send_replace(true);
                    response
                });
                handling.await.unwrap_or_else(|_| {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                })
            }
        },
    );
    let turn_followed = axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let turn_hooks = turn_hooks.clone();
            async move {
                use axum::response::IntoResponse as _;
                let Some(turn) = turn_hooks else {
                    return next.run(request).await;
                };
                let path = request.uri().path();
                if path.ends_with("/UserPromptSubmit")
                    && turn.prompts_lost.load(std::sync::atomic::Ordering::SeqCst)
                {
                    return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                if !path.ends_with("/Stop") {
                    return next.run(request).await;
                }
                // To its end even if its client hangs up.
                let stop = tokio::spawn(async move {
                    let response = next.run(request).await;
                    // A held Stop (202) decided nothing about the turn.
                    if response.status() == axum::http::StatusCode::OK {
                        let _ = turn.stop_answered.send(());
                    }
                    response
                });
                stop.await.unwrap_or_else(|_| {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                })
            }
        },
    );
    let internal = yulius_server::internal_router(app.clone())
        .layer(goal_held)
        .layer(turn_followed)
        .layer(session_start_answered)
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
    Ok((app, port))
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

/// A user, a sandbox whose repository is a clone of a bare origin, and a Claude account.
async fn seed(app: &App, data: &Path) -> Result<(SandboxId, UserId), BoxError> {
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
        id: SandboxId::parse("s-done")?,
        slug: "done".into(),
        repo_owner: "acme".into(),
        repo_name: "done".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 43000,
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
            credential: Some(
                app.deps
                    .crypto
                    .seal(concat!("sk-ant-oat01-", "test-credential").as_bytes())?,
            ),
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
    Ok((sandbox.id, user.id))
}

/// A goal-mode task (TSK-010): Yulius types `/goal <prompt>` once the session is ready.
async fn goal_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, BoxError> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title} and verify it"),
            harness: Harness::ClaudeCode,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: TaskMode::Goal,
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

async fn task(app: &App, id: &TaskId) -> Result<TaskRecord, BoxError> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

async fn activity(app: &App, id: &TaskId) -> Option<Activity> {
    app.deps
        .store
        .task(id)
        .await
        .ok()
        .flatten()
        .and_then(|t| t.activity)
}

/// What the UI receives for the task: its activity and background shells.
async fn shown(app: &App, id: &TaskId) -> Result<(Option<String>, Option<u32>), BoxError> {
    let summary = app.task_summary(&task(app, id).await?).await?;
    Ok((summary.activity, summary.background_shells))
}

/// What the task's first launch asked the agent for (the fake harness records it).
async fn first_launch(app: &App, id: &TaskId) -> Result<String, BoxError> {
    let attempts = app.deps.store.attempts(id).await?;
    let first = attempts.first().ok_or("the task has no attempt")?;
    Ok(std::fs::read_to_string(
        Path::new(&first.run_dir).join("fake-launch.txt"),
    )?)
}

async fn screen_contains(app: &App, id: &TaskId, needle: &str) -> bool {
    app.runtime(id).hub.screen_text().contains(needle)
}

async fn stop(app: &Arc<App>, id: &TaskId) -> TestResult {
    support::stop_tasks(app, &[id]).await
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

/// TSK-014: the goal turn ends cleanly (Stop hook), Claude Code records the goal as achieved and
/// no shell is left: the task is Done. Typing again makes it Working; after that turn it waits
/// for the user (the achieved goal no longer counts), but it answered with nothing open: nobody
/// has to act, and it is not counted in Needs you (SPEC-429; task #17 of another project read
/// Your turn after "you don't need anything from me?" was answered).
#[test]
fn an_achieved_goal_with_no_shell_left_makes_the_task_done() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        wait_for("the achieved goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "Goal achieved")
        })
        .await?;
        // SPEC-117 ONE-001: the session's only first message is its goal, the launch prompt.
        let launch = first_launch(&app, &id).await?;
        assert!(
            launch.contains("prompt: Some(\"/goal Tidy the parser and verify it\\n\\nDone = "),
            "{launch}"
        );
        wait_for("Done", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        wait_for("the shell probe", Duration::from_secs(20), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        let record = task(&app, &id).await?;
        assert_eq!(
            record.status,
            TaskStatus::Running,
            "the session stays attached"
        );

        // The user types again: Working, then (the turn ends) their turn, not Done.
        let mut events = app.events.subscribe();
        app.send_input(&id, b"also rename the module\r").await?;
        wait_for("the second turn", Duration::from_secs(20), || {
            screen_contains(&app, &id, "ECHO: also rename the module")
        })
        .await?;
        wait_for("Your turn", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        let mut seen = Vec::new();
        while let Ok(envelope) = events.try_recv() {
            if let UiEvent::TaskUpdated { task } = envelope.event
                && task.id == id.as_str()
            {
                seen.push(task.activity.unwrap_or_default());
            }
        }
        assert!(
            seen.iter().any(|a| a == "working"),
            "typing made it Working: {seen:?}"
        );
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(
            activity(&app, &id).await,
            Some(Activity::Waiting),
            "a late read of the prompt does not reopen the finished turn"
        );
        let summary = app.task_summary(&task(&app, &id).await?).await?;
        assert!(!summary.goal_unfinished, "its goal was met");
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let counts = app
            .list_sandboxes(&ctx)
            .await?
            .into_iter()
            .find(|s| s.id == sandbox.as_str())
            .ok_or("sandbox")?
            .counts;
        assert_eq!(counts.needs_attention, 0, "answered: nobody has to act");
        // It reads Answered under Done, where Archive is offered: archiving closes its session
        // as for a Done task, not "stop the task first" (SPEC-429 ANS-007).
        let archived = app.archive_task(&ctx, id.as_str(), true).await?;
        assert!(archived.archived, "archived");
        // Closed, not stopped; it stays Running until its session is gone (`close_task_now`).
        wait_for("the close", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Closed)
        })
        .await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-085 STT-001 (task #60 on 2026-10-13: Done, then Working for 19 minutes until it was
/// stopped): Claude Code compacts an idle session before its prompt cache expires. The records
/// it writes (the summary the session continues from, the files it reads again, its SessionStart
/// hook and notice) are no prompt: the Done task stays Done and never reads Working.
#[test]
fn an_idle_compaction_leaves_a_done_task_done() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            idle_compact_ms: Some(6_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Fix the status").await?;
        wait_for("Done", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        if screen_contains(&app, &id, "COMPACTED WHILE IDLE").await {
            return Err("the compaction came before Done: the test proves nothing".into());
        }
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut events = app.events.subscribe();
        let collector = tokio::spawn({
            let (seen, id) = (Arc::clone(&seen), id.as_str().to_owned());
            async move {
                loop {
                    match events.recv().await {
                        Ok(envelope) => {
                            if let UiEvent::TaskUpdated { task } = envelope.event
                                && task.id == id
                            {
                                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                                seen.push(task.activity.unwrap_or_default());
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        wait_for("the idle compaction", Duration::from_secs(30), || {
            screen_contains(&app, &id, "COMPACTED WHILE IDLE")
        })
        .await?;
        // Its records were written before it said so: two more watcher rounds read them.
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for("the compaction read", Duration::from_secs(20), || async {
            runtime.transcript_rounds() >= rounds + 2
        })
        .await?;
        assert_eq!(activity(&app, &id).await, Some(Activity::Done));
        collector.abort();
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(
            seen.iter().all(|a| a == "done"),
            "the compaction never made the task Working: {seen:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-085 STT-005: a person types `/compact` into the idle session of a Done task. Claude Code
/// writes the command as typed, compacts and writes its records (no turn, no Stop hook): the task
/// stays Done throughout and after. A prompt typed right after is work as always.
#[test]
fn a_typed_compaction_leaves_a_done_task_done() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Fix the status").await?;
        wait_for("Done", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut events = app.events.subscribe();
        let collector = tokio::spawn({
            let (seen, id) = (Arc::clone(&seen), id.as_str().to_owned());
            async move {
                loop {
                    match events.recv().await {
                        Ok(envelope) => {
                            if let UiEvent::TaskUpdated { task } = envelope.event
                                && task.id == id
                            {
                                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                                seen.push(task.activity.unwrap_or_default());
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        let seen_now = || seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        app.send_input(&id, b"/compact keep the plan\r").await?;
        wait_for("the compaction", Duration::from_secs(20), || {
            screen_contains(&app, &id, "COMPACTED: keep the plan")
        })
        .await?;
        // Its records were written before it said so: two more watcher rounds read them.
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for("the compaction read", Duration::from_secs(20), || async {
            runtime.transcript_rounds() >= rounds + 2
        })
        .await?;
        assert_eq!(activity(&app, &id).await, Some(Activity::Done));
        assert!(
            seen_now().iter().all(|a| a == "done"),
            "the compaction never made the task Working: {:?}",
            seen_now()
        );

        // A prompt typed right after is work: Working, then (the goal no longer counts) Your turn.
        app.send_input(&id, b"also rename the module\r").await?;
        wait_for("the prompt's turn", Duration::from_secs(20), || {
            screen_contains(&app, &id, "ECHO: also rename the module")
        })
        .await?;
        wait_for("Your turn", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        wait_for("the Working update", Duration::from_secs(10), || async {
            seen_now().iter().any(|a| a == "working")
        })
        .await?;
        collector.abort();
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The real session host whose probe also finds `waits` shells that only run `yulius wait`
/// (what `yulius_ptyhost::shells::count_background` tells apart on a real process tree).
struct WaitingHost {
    inner: PtySessionHost,
    waits: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait::async_trait]
impl SessionHost for WaitingHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        self.inner
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        self.inner.attach(run_dir, token).await
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        self.inner.background_shells(child_pid)
    }

    fn background_probe(&self, child_pid: u32) -> Option<BackgroundProbe> {
        self.inner
            .background_probe(child_pid)
            .map(|found| BackgroundProbe {
                waits: self.waits.load(std::sync::atomic::Ordering::SeqCst),
                ..found
            })
    }
}

/// SPEC-085 STT-002 ("needs you was too often": 173 idle periods of 37 tasks read Your turn while
/// their agent waited in `yulius wait` for its checks or merge): the probe's waiting shells reach
/// the task's view and the sandbox's counts. The waiting task is not counted as needing a person
/// and is still no background work; once the wait ended, Your turn needs the person again.
#[test]
fn an_agent_waiting_for_yulius_does_not_need_a_person() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_verdict: Some("failed"),
            ..FakeAgentOptions::default()
        };
        let waits = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let host = Arc::new(WaitingHost {
            inner: PtySessionHost::new(exe),
            waits: Arc::clone(&waits),
        });
        let (app, _) = start_on(&data, &fake, agent, 0, Some(host), None, false, None).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let needs_you = || async {
            app.list_sandboxes(&ctx)
                .await
                .ok()
                .and_then(|sandboxes| sandboxes.into_iter().find(|s| s.id == sandbox.as_str()))
                .map(|s| s.counts.needs_attention)
        };
        let mut events = app.events.subscribe();
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        let view = || async {
            let summary = app.task_summary(&task(&app, &id).await?).await?;
            Ok::<_, BoxError>((
                summary.activity,
                summary.background_shells,
                summary.yulius_wait,
            ))
        };
        wait_for("Your turn", Duration::from_secs(90), || async {
            view().await.ok() == Some((Some("waiting".into()), Some(0), false))
        })
        .await?;
        // Its goal failed: the work is unfinished, so it is the person's turn (SPEC-429 ANS-002).
        // The fake's turn is shorter than a transcript read, so the record may come after it: the
        // UI is told then, not left reading Answered (ANS-008).
        let published = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match events.recv().await {
                    Ok(envelope) => {
                        if let UiEvent::TaskUpdated { task } = envelope.event
                            && task.id == id.as_str()
                            && task.goal_unfinished
                        {
                            return true;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return false,
                }
            }
        })
        .await;
        assert_eq!(published, Ok(true), "an update says the goal is unfinished");
        let summary = app.task_summary(&task(&app, &id).await?).await?;
        assert!(summary.goal_unfinished, "a failed goal is unfinished");
        assert_eq!(needs_you().await, Some(1), "Your turn needs the person");

        // The agent waits for its checks: the next probe finds it.
        waits.store(1, std::sync::atomic::Ordering::SeqCst);
        wait_for("the wait in the view", Duration::from_secs(20), || async {
            view().await.ok() == Some((Some("waiting".into()), Some(0), true))
        })
        .await?;
        assert_eq!(needs_you().await, Some(0), "nobody has to act");

        // The wait returned and the agent stopped: Your turn again.
        waits.store(0, std::sync::atomic::Ordering::SeqCst);
        wait_for("the wait over", Duration::from_secs(20), || async {
            view().await.ok() == Some((Some("waiting".into()), Some(0), false))
        })
        .await?;
        assert_eq!(needs_you().await, Some(1));
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A goal task that got its goal from its launch prompt, then stopped and continued: the resumed
/// session (no prompt) gets its `/goal` typed once it reads input (SPEC-117 ONE-007). Returns
/// once the resumed run's screen replaced the first one's, before its goal.
async fn continued_goal_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
) -> Result<TaskId, BoxError> {
    continued_goal_task_with(app, sandbox, user, |_| {}).await
}

/// [`continued_goal_task`]; `before` runs with the task's id before the task is continued.
async fn continued_goal_task_with(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    before: impl FnOnce(&TaskId),
) -> Result<TaskId, BoxError> {
    let id = goal_task(app, sandbox, user, "Tidy the parser").await?;
    wait_for("the launch goal", Duration::from_secs(60), || {
        screen_contains(app, &id, "GOAL SET")
    })
    .await?;
    stop(app, &id).await?;
    before(&id);
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    app.continue_task(
        &ctx,
        id.as_str(),
        &yulius_contracts::api::tasks::ContinueTask { account_id: None },
    )
    .await?;
    wait_for("the resumed session", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
        attempts.len() == 2
            && attempts[1].state == AttemptState::Live
            && screen_contains(app, &id, "FAKE-AGENT READY").await
            && !screen_contains(app, &id, "GOAL SET").await
    })
    .await?;
    Ok(id)
}

/// The resumed session's goal was typed: its launch had no prompt, and the goal turn ran.
async fn goal_typed_into_the_resumed_session(app: &App, id: &TaskId) -> TestResult {
    wait_for("the typed goal", Duration::from_secs(60), || {
        screen_contains(app, id, "ECHO: /goal Tidy the parser")
    })
    .await?;
    let attempts = app.deps.store.attempts(id).await?;
    let second = attempts.get(1).ok_or("no resumed run")?;
    let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
    assert!(
        launch.contains("Resume {") && launch.contains("prompt: None"),
        "{launch}"
    );
    Ok(())
}

/// TSK-010, SPEC-117 ONE-007: a goal typed into a resumed session whose TUI drops the first
/// submit is typed again until Claude Code takes it, so the task goes on instead of showing the
/// goal unsubmitted.
#[test]
fn a_goal_the_tui_dropped_is_submitted_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            // The resumed session drops the typed goal's first submit, which costs the first two
            // submit waits (13 s). The launch goal is no typed line.
            drop_lines: Some(1),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = continued_goal_task(&app, &sandbox, &user).await?;
        goal_typed_into_the_resumed_session(&app, &id).await?;
        // About a second after the goal turn, also at a load average of 60 (SPEC-067).
        wait_for("Done", Duration::from_secs(45), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 TYP-002: the TUI dropped the typed goal's submit, so the goal may still sit in the
/// prompt box until a later try submits it. A message for the task that comes meanwhile is
/// refused, as while a message is being typed: typed now, it would be sent with the goal by the
/// next Enter, or erased when the goal is typed again.
#[test]
fn nothing_is_typed_while_a_typed_goal_is_not_taken() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            drop_lines: Some(1),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let refused = Arc::new(std::sync::Mutex::new(None));
        let id = continued_goal_task_with(&app, &sandbox, &user, |id| {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let refused = refused.clone();
            let caller = yulius_app::coordination::ProjectCaller {
                sandbox: sandbox.clone(),
                user: user.clone(),
                task: None,
                channel: yulius_app::coordination::Channel::Superagent,
                actor: "cockpit:admin".into(),
                detail: serde_json::json!({"via": "cockpit", "shell": "sh-test"}),
            };
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::GoalEntered,
                Path::new(key.as_str()),
                move || {
                    let answer = wait_in_hook(
                        &handle,
                        app.tool_task_message(
                            &caller,
                            &serde_json::json!({"task_id": id.as_str(), "text": "one more thing"}),
                        ),
                    );
                    if let Ok(mut slot) = refused.lock() {
                        *slot = Some(answer.map(|_| ()).map_err(|err| err.to_string()));
                    }
                },
            );
        })
        .await?;
        goal_typed_into_the_resumed_session(&app, &id).await?;
        let answer = refused.lock().map_err(|_| "poisoned")?.clone();
        let answer = answer.ok_or("no message was tried after the goal's Enter")?;
        assert!(
            matches!(&answer, Err(why) if why.contains("is being typed")),
            "{answer:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-346 LPH-002, LPH-003 (#921): the goal typed into a resumed session (after a dropped
/// submit, as above) whose `UserPromptSubmit` the server handles only after the hook client gave
/// up (5 s): the agent ran the goal turn meanwhile, its Stop waited for the verdict and Claude
/// Code recorded the goal as met. The late prompt changes nothing, and the task is Done. Before,
/// the late prompt set Working and reset the goal, or the met goal left it reading Your turn.
#[test]
fn a_goal_whose_prompt_hook_is_answered_late_is_done() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            drop_lines: Some(1),
            ..FakeAgentOptions::default()
        };
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let (handled_tx, mut handled) = tokio::sync::watch::channel(false);
        // Past the client's 5 s, and then until the turn's Stop was handled.
        let late = LateHook {
            hold: Duration::from_millis(5_500),
            armed: Arc::new(AtomicBool::new(false)),
            holding: Arc::new(AtomicBool::new(false)),
            stop_handled: tokio::sync::watch::channel(false).0,
            handled: handled_tx,
        };
        let (app, _) = start_holding(
            &data,
            &fake,
            agent,
            0,
            None,
            (None, None),
            false,
            None,
            Some(late.clone()),
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = continued_goal_task(&app, &sandbox, &user).await?;
        // The resumed session's goal is typed from now on (its first submit is dropped).
        late.armed.store(true, SeqCst);
        goal_typed_into_the_resumed_session(&app, &id).await?;
        tokio::time::timeout(Duration::from_secs(60), handled.wait_for(|h| *h)).await??;
        assert!(
            *late.stop_handled.borrow(),
            "the goal turn's Stop was handled before its prompt (the late path)"
        );
        wait_for(
            "Done after the late prompt",
            Duration::from_secs(45),
            || async { activity(&app, &id).await == Some(Activity::Done) },
        )
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// [`start`] with the turn hooks followed: the receiver gets each Stop Yulius answered.
async fn start_following(
    data: &Path,
    fake: &Path,
) -> Result<(Arc<App>, TurnHooks, std::sync::mpsc::Receiver<()>), BoxError> {
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };
    let (stop_answered, answered) = std::sync::mpsc::channel();
    let turn = TurnHooks {
        stop_answered,
        prompts_lost: Arc::default(),
    };
    let hooks = Some(turn.clone());
    let (app, _) = start_holding(
        data,
        fake,
        agent,
        0,
        None,
        (None, None),
        false,
        None,
        None,
        hooks,
    )
    .await?;
    Ok((app, turn, answered))
}

/// SPEC-453 TPO-001 (#1066): a goal turn's Stop arrives while the transcript watcher is between
/// its last-Stop check of that turn's prompt and the turn it starts for it: the Stop waits for
/// the prompt, opens its verdict wait after it, and Claude Code's "met" makes the task Done.
/// Before, the watcher cleared the verdict wait the Stop had just opened: "met" found no waited
/// turn and the task stayed Working with the agent idle, as on a loaded host.
#[test]
fn a_goal_whose_stop_races_its_transcript_prompt_is_done() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        use yulius_app::race::{Point, on};
        let (app, _, answered) = start_following(&data, &fake).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        // The launch goal is met: the watcher read its whole turn.
        wait_for("the launch goal Done", Duration::from_secs(90), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        let key = PathBuf::from(id.as_str());
        let (checked_tx, checked) = std::sync::mpsc::channel();
        // The next turn's Stop, about to settle, waits until the watcher's prompt passed its check.
        let raced = Arc::new(AtomicBool::new(false));
        let stop_raced = raced.clone();
        on(Point::StopClean, &key, move || {
            let checked =
                tokio::task::block_in_place(|| checked.recv_timeout(Duration::from_secs(30)));
            stop_raced.store(checked.is_ok(), SeqCst);
        });
        on(Point::UserTurnChecked, &key, move || {
            // Stops of the turns before are not this turn's: its own cannot end before this.
            while answered.try_recv().is_ok() {}
            let _ = checked_tx.send(());
            // The Stop goes on now; the watcher gives it time to end before the prompt does.
            let _ = tokio::task::block_in_place(|| answered.recv_timeout(Duration::from_secs(2)));
        });
        app.send_input(&id, b"/goal Tidy the lexer too\r").await?;
        // Its prompt hook was answered (Working) before the agent echoes it.
        wait_for("the typed goal's turn", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: /goal Tidy the lexer too")
        })
        .await?;
        wait_for("Done", Duration::from_secs(45), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        assert!(raced.load(SeqCst), "both points were reached");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-453 TPO-002 (#1066): a turn Yulius knows only from the transcript (its prompt hook was
/// lost) whose Stop read the task before the watcher's prompt set Working: the Stop settles the
/// task as it is by then, Your turn. It used to settle from its copy, which already read Your
/// turn, so it wrote nothing and the task stayed Working with the agent idle.
#[test]
fn a_stop_that_raced_its_transcript_prompt_settles_the_turn() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        use yulius_app::race::{Point, on};
        let (app, turn, answered) = start_following(&data, &fake).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Make the header blue".into(),
                harness: Harness::ClaudeCode,
                title: Some("Blue header".into()),
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
            .await?
            .id;
        wait_for("the run", Duration::from_secs(90), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        // An ordinary turn first: its hooks arrive and its Stop leaves Your turn.
        app.send_input(&id, b"hello\r").await?;
        wait_for("the first turn's Stop", Duration::from_secs(30), || async {
            answered.try_recv().is_ok()
        })
        .await?;
        wait_for("Your turn", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        // The next prompt hook is lost: only the transcript says a turn began.
        turn.prompts_lost
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let key = PathBuf::from(id.as_str());
        let (parked_tx, parked) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        // The turn's Stop read the task and is about to settle it: it waits for the test.
        on(Point::StopClean, &key, move || {
            let _ = parked_tx.send(());
            let _ = tokio::task::block_in_place(|| released.recv_timeout(Duration::from_secs(30)));
        });
        app.send_input(&id, b"also rename the module\r").await?;
        wait_for(
            "the Stop about to settle",
            Duration::from_secs(30),
            || async { parked.try_recv().is_ok() },
        )
        .await?;
        wait_for(
            "the transcript's prompt",
            Duration::from_secs(30),
            || async { activity(&app, &id).await == Some(Activity::Working) },
        )
        .await?;
        release.send(())?;
        wait_for(
            "Your turn after the Stop",
            Duration::from_secs(20),
            || async { activity(&app, &id).await == Some(Activity::Waiting) },
        )
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// TSK-010 (#243), SPEC-117 ONE-007: a resumed session's SessionStart hook answered before the
/// supervisor watches the run (a fast TUI, a busy host) still counts: the goal is typed once the
/// TUI is quiet, not after the 45 s fallback for a TUI without hooks.
#[test]
fn a_session_start_before_the_run_is_watched_still_types_the_goal_at_once() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, true).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = continued_goal_task(&app, &sandbox, &user).await?;
        // Well under the hookless fallback (45 s): the goal follows the hook, not the fallback.
        let resumed = Instant::now();
        goal_typed_into_the_resumed_session(&app, &id).await?;
        assert!(
            resumed.elapsed() < Duration::from_secs(30),
            "{:?}",
            resumed.elapsed()
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-011, SPEC-117 ONE-001, SPEC-212 WHL-001: a goal task writes its request file (the request,
/// its attached files and the definition of done) and starts from a `/goal` that is the whole
/// request, each attached file at its place, then the definition of done, given as the launch
/// prompt: one first message, nothing typed into the session.
#[test]
fn a_goal_task_starts_from_its_request_file() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let record = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Make the header blue\n\n@/srv/shots/header.png\n@/srv/notes.txt".into(),
                harness: Harness::ClaudeCode,
                title: Some("Blue header".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
                mode: TaskMode::Goal,
                supertask: false,
            })
            .await?;
        let id = record.id;
        wait_for("the goal to start", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        let file = data
            .join("sandboxes")
            .join(sandbox.as_str())
            .join("home/.yulius/tasks")
            .join(format!("{}.md", record.number));
        let request = std::fs::read_to_string(&file)?;
        assert!(
            request.contains("## Request\n\nMake the header blue\n"),
            "{request}"
        );
        assert!(request.contains("\n- /srv/shots/header.png"), "{request}");
        assert!(request.contains("## Definition of done"), "{request}");
        // The launch prompt is the goal: the whole request, each file at its place, and no
        // request-file path.
        let launch = first_launch(&app, &id).await?;
        assert!(
            launch.contains(
                "prompt: Some(\"/goal Make the header blue\\n\\n[file 1: /srv/shots/header.png]\\n\
                 [file 2: /srv/notes.txt]\\n\\nDone = "
            ),
            "{launch}"
        );
        assert!(!launch.contains(&*file.to_string_lossy()), "{launch}");
        // Nothing was typed before the goal: no first message, no `/goal`.
        let screen = app.runtime(&id).hub.screen_text();
        let set = screen.find("GOAL SET").ok_or_else(|| screen.clone())?;
        assert!(!screen[..set].contains("ECHO:"), "{screen}");
        assert!(!screen[..set].contains("/goal"), "{screen}");
        // Nor after it: the goal turn's own echo is the only answer, and the goal is set once.
        wait_for("the goal turn's answer", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: /goal Make the header blue")
        })
        .await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let screen = app.runtime(&id).hub.screen_text();
        assert_eq!(screen.matches("ECHO:").count(), 1, "{screen}");
        assert_eq!(screen.matches("GOAL SET").count(), 1, "{screen}");
        assert!(!screen.contains("[Image #1]"), "{screen}");
        assert!(!screen.contains("STOP BLOCKED"), "{screen}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-052 AC-QMS-3: a message queued while the task waited reaches the agent with its request
/// when it starts: in the request file and the goal of a goal task, in the first message of a
/// prompt task.
///
/// The wait is real (SPEC-082 TST-002): another task holds the only session slot, so the
/// session limit queues the task and nothing can start it before its message is in; the
/// holder's stop then frees the slot and the queue starts it. Before, the test moved a task to
/// `queued` itself while slots were free: any pass of the queue (the previous task's stop or
/// Done, SES-101) could start it first, and the message was refused about one run in four.
#[test]
fn a_message_queued_before_the_start_reaches_the_agent() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(&data, &fake, agent, 0, None, None, false, Some(1)).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let new_task = |mode: TaskMode, prompt: &str| NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: prompt.into(),
            harness: Harness::ClaudeCode,
            title: Some("Header".into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode,
            supertask: false,
        };
        for mode in [TaskMode::Goal, TaskMode::Prompt] {
            // A prompt task holds the only slot: after its turn it waits for its person and is
            // never Done, so it keeps the slot until it is stopped (SES-101).
            let holder = app
                .create_task_internal(new_task(TaskMode::Prompt, "Hold the slot"))
                .await?
                .id;
            wait_for("the holder's session", Duration::from_secs(60), || async {
                first_launch(&app, &holder).await.is_ok()
                    && task(&app, &holder)
                        .await
                        .is_ok_and(|t| t.status == TaskStatus::Running)
            })
            .await?;
            let record = app
                .create_task_internal(new_task(mode, "Make the header red"))
                .await?;
            let id = record.id;
            // At the session limit the task waits, whatever its start at creation did meanwhile.
            assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
            app.queue_task_message(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::QueueMessage {
                    text: "Blue, not red.".into(),
                },
            )
            .await?;
            // A pass of the queue changes nothing while the slot is held.
            app.drain_queue_now().await;
            assert_eq!(task(&app, &id).await?.status, TaskStatus::Queued);
            // The holder's stop frees the slot: the queue starts the task.
            stop(&app, &holder).await?;
            match mode {
                // A goal task's request file carries the message, and a short request is
                // quoted in the goal itself, the message with it.
                TaskMode::Goal => {
                    wait_for("the goal", Duration::from_secs(60), || {
                        screen_contains(&app, &id, "GOAL SET")
                    })
                    .await?;
                    let file = data
                        .join("sandboxes")
                        .join(sandbox.as_str())
                        .join("home/.yulius/tasks")
                        .join(format!("{}.md", record.number));
                    let request = std::fs::read_to_string(&file)?;
                    let added = "\n\n**Added by admin before the task started (";
                    assert!(
                        request.contains(&format!("## Request\n\nMake the header red{added}")),
                        "{request}"
                    );
                    assert!(
                        request.contains(" UTC):**\n\nBlue, not red.\n"),
                        "{request}"
                    );
                    let launch = first_launch(&app, &id).await?;
                    let goal = launch.find("/goal ").ok_or_else(|| launch.clone())?;
                    assert!(launch[goal..].contains("Blue, not red."), "{launch}");
                }
                TaskMode::Prompt => {
                    // Its session runs (the launch file is written before the spawn; a stop
                    // during the start waits for the whole escalation).
                    wait_for("the first launch", Duration::from_secs(60), || async {
                        first_launch(&app, &id).await.is_ok()
                            && task(&app, &id)
                                .await
                                .is_ok_and(|t| t.status == TaskStatus::Running)
                    })
                    .await?;
                    let launch = first_launch(&app, &id).await?;
                    assert!(launch.contains("Make the header red"), "{launch}");
                    assert!(launch.contains("Blue, not red."), "{launch}");
                }
            }
            // Started: a later message is refused, and the request stays as the agent got it.
            let late = app
                .queue_task_message(
                    &ctx,
                    id.as_str(),
                    &yulius_contracts::api::tasks::QueueMessage {
                        text: "Green.".into(),
                    },
                )
                .await;
            assert!(
                matches!(late, Err(yulius_app::error::AppError::Conflict(_))),
                "{late:?}"
            );
            stop(&app, &id).await?;
        }
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-034 LIM-003, SPEC-072 LRS-004 (#278): a goal session whose turn a usage limit cut moves to
/// another subscription by a continued resume; Claude Code redoes the cut turn first, and the goal
/// is typed again after that turn's Stop, not during it, and once, as one line (SPEC-212 WHL-003).
#[test]
fn the_goal_is_typed_after_the_turn_continued_past_a_limit() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            // The goal turn is the first, the next message hits the limit.
            limit_after: Some(2),
            // The continued turn: silent for longer than the quiet time after which a goal is
            // otherwise typed.
            opening_turn_ms: Some(6_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        app.deps
            .store
            .insert_account(&AccountRecord {
                id: AccountId::parse("b-claude")?,
                provider: Provider::Anthropic,
                label: "Claude second".into(),
                credential: Some(
                    app.deps
                        .crypto
                        .seal(concat!("sk-ant-oat01-", "second-credential").as_bytes())?,
                ),
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
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                // SPEC-212 WHL-003: more than one line; the typed goal is one.
                prompt: "Make the header blue\n\nand the footer".into(),
                harness: Harness::ClaudeCode,
                title: Some("Blue header".into()),
                workflow_id: None,
                steps: Vec::new(),
                model: None,
                effort: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
                mode: TaskMode::Goal,
                supertask: false,
            })
            .await?
            .id;
        wait_for("the goal to start", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the continued run", Duration::from_secs(90), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let launch = std::fs::read_to_string(
            Path::new(&app.deps.store.attempts(&id).await?[1].run_dir).join("fake-launch.txt"),
        )?;
        assert!(launch.contains("continue_interrupted: true"), "{launch}");
        // The new run's screen only: its continued turn, then the goal.
        wait_for(
            "the goal after the continued turn",
            Duration::from_secs(90),
            || {
                let app = app.clone();
                let id = id.clone();
                async move {
                    let screen = app.runtime(&id).hub.screen_text();
                    screen
                        .find("DONE opening")
                        .is_some_and(|done| screen[done..].contains("GOAL SET"))
                }
            },
        )
        .await?;
        let screen = app.runtime(&id).hub.screen_text();
        let done = screen.find("DONE opening").ok_or_else(|| screen.clone())?;
        let typed = screen.find("/goal ").ok_or_else(|| screen.clone())?;
        assert!(
            done < typed,
            "the goal was typed during the continued turn:\n{screen}"
        );
        assert_eq!(screen.matches("GOAL SET").count(), 1, "{screen}");
        // The whole condition reached the goal as one line: none of it was submitted apart.
        wait_for("the typed goal's turn", Duration::from_secs(30), || {
            screen_contains(
                &app,
                &id,
                "ECHO: /goal Make the header blue and the footer | Done = ",
            )
        })
        .await?;
        let screen = app.runtime(&id).hub.screen_text();
        assert!(!screen.contains("ECHO: and the footer"), "{screen}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-072 LRS-001 (review round 2), with hooks like the real CLI: Claude Code continues the turn
/// a limit cut by itself with only its answer; its Stop hook may arrive while the `limit` pause is
/// open (and be held), but the answer ends the pause, the Stop is answered and the task runs on
/// its first run.
#[test]
fn a_continuation_with_hooks_ends_the_limit_pause_and_its_stop_is_answered() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            limit_after: Some(1),
            limit_continues_ms: Some(5_000),
            limit_continues_answer_only: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Make the header blue".into(),
                harness: Harness::ClaudeCode,
                title: Some("Blue header".into()),
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
            .await?
            .id;
        wait_for("the run", Duration::from_secs(60), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for(
            "the pause at the limit",
            Duration::from_secs(30),
            || async {
                matches!(
                    app.deps.store.open_pause(&id).await,
                    Ok(Some(pause)) if pause.kind == yulius_app::model::PauseKind::Limit
                )
            },
        )
        .await?;
        wait_for(
            "the continued turn's Stop answered",
            Duration::from_secs(60),
            || screen_contains(&app, &id, "CONTINUED STOP ANSWERED"),
        )
        .await?;
        // The pause closes, then the task runs: wait for both.
        wait_for("the task to run", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        assert!(app.deps.store.open_pause(&id).await?.is_none(), "no pause");
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-030 PIMG-002: a prompt-mode task with an attached image starts with no command-line
/// prompt; its first message is typed into the TUI with the image pasted (`[Image #1]`, as
/// Claude Code attaches a pasted image) and the other files mentioned. An older prompt's files
/// follow its text, so the image is pasted after it (SPEC-124 INL-007).
#[test]
fn an_attached_image_is_pasted_into_the_first_message() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Make it blue\n\n@/srv/shot.png\n@/srv/notes.txt".into(),
                harness: Harness::ClaudeCode,
                title: Some("Blue".into()),
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
            .await?
            .id;
        // The answer spans lines: wait for all of it, not its first line (under load the
        // lines after `ECHO: Make it blue` reached the screen after it was read).
        wait_for("the first message", Duration::from_secs(90), || {
            let app = app.clone();
            let id = id.clone();
            async move {
                let screen = app.runtime(&id).hub.screen_text();
                screen
                    .find("ECHO: Make it blue")
                    .is_some_and(|echo| screen[echo..].contains("@\"/srv/notes.txt\""))
            }
        })
        .await?;
        let launch = first_launch(&app, &id).await?;
        assert!(launch.contains("prompt: None"), "{launch}");
        let screen = app.runtime(&id).hub.screen_text();
        assert!(screen.contains("[Image #1] "), "{screen}");
        assert!(screen.contains("@\"/srv/notes.txt\""), "{screen}");
        assert!(!screen.contains("@\"/srv/shot.png\""), "{screen}");
        assert_eq!(screen.matches("ECHO: ").count(), 1, "{screen}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-124 INL-007: an image written inside the text (`@'path'`, as the composer writes it) is
/// pasted where it sits: one first message with `[Image #1]` between the words around it.
#[test]
fn an_image_in_the_text_is_pasted_where_it_sits() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "This part @'/srv/my shot.png' looks wrong, see @'/srv/notes.txt'".into(),
                harness: Harness::ClaudeCode,
                title: None,
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
            .await?
            .id;
        wait_for("the first message", Duration::from_secs(90), || {
            screen_contains(
                &app,
                &id,
                "ECHO: This part [Image #1] looks wrong, see @\"/srv/notes.txt\"",
            )
        })
        .await?;
        let screen = app.runtime(&id).hub.screen_text();
        assert_eq!(screen.matches("ECHO: ").count(), 1, "one message: {screen}");
        let task = app.deps.store.task(&id).await?.ok_or("no task")?;
        assert_eq!(task.title, "This part looks wrong, see");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// TSK-014: the goal turn leaves a background shell (Claude Code's Bash tool shape): the task is
/// shown Working with one background shell until the shell ends, then Done.
#[test]
fn a_background_shell_keeps_the_task_working_until_it_ends() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_background: Some(8),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Start the dev server").await?;
        wait_for("the achieved goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "Goal achieved")
        })
        .await?;
        wait_for("one background shell", Duration::from_secs(20), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(_, shells)| shells == Some(1))
        })
        .await?;
        assert!(
            !screen_contains(&app, &id, "BACKGROUND DONE").await,
            "the shell still runs"
        );
        // Claude Code wakes the agent when the shell ends; its Stop settles the task.
        wait_for("the shell's end", Duration::from_secs(30), || {
            screen_contains(&app, &id, "BACKGROUND DONE")
        })
        .await?;
        wait_for("Done without shells", Duration::from_secs(20), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-486 WFL-006: a prompt that reaches a session whose workflow a usage limit paused (here
/// the move off the limited subscription was refused, so the session stays where it is) is told,
/// through the prompt hook, to resume the run by its id and not to start it again. Before the
/// limit, and for a session without a workflow, the hook adds nothing.
#[test]
fn a_prompt_at_a_workflow_paused_by_a_limit_is_told_to_resume_it() -> TestResult {
    use yulius_app::ports::Crypto as _;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            workflow_limit: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let store = &app.deps.store;
        // A second subscription whose credential cannot be opened: the move to it is refused.
        store
            .insert_account(&AccountRecord {
                id: AccountId::parse("a-claude-2")?,
                provider: Provider::Anthropic,
                label: "Claude spare".into(),
                credential: Some(MasterKeyCrypto::from_key(&[9u8; 32])?.seal(b"spare")?),
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
        let id = goal_task(&app, &sandbox, &user, "Audit the code").await?;
        wait_for("the goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        app.send_input(&id, b"before\r").await?;
        wait_for("the first answer", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: before")
        })
        .await?;
        app.send_input(&id, b"/workflow\r").await?;
        wait_for("the refused move", Duration::from_secs(60), || async {
            store
                .switches(&id)
                .await
                .is_ok_and(|s| s.iter().any(|s| s.outcome == "refused"))
        })
        .await?;
        assert!(
            !screen_contains(&app, &id, "PROMPT CONTEXT").await,
            "nothing is added before a limit"
        );
        let run = app.dynamic_workflows(&id);
        assert_eq!(run.len(), 1, "{run:?}");
        app.send_input(&id, b"continue\r").await?;
        wait_for("the answer", Duration::from_secs(30), || {
            screen_contains(&app, &id, "ECHO: continue")
        })
        .await?;
        let told = format!(
            "PROMPT CONTEXT: Note from Yulius: this session's dynamic workflow (run {}) is paused at a usage limit.",
            run[0]
        );
        let seen = app.runtime(&id).hub.screen_text();
        let flat = seen.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(&told), "{seen}");
        assert!(flat.contains("Do not start that workflow again"), "{seen}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-290 GDW-001 (task #272, 2026-10-24: "working 11h 15m" after "Goal achieved"): the goal
/// turn leaves a background shell (its `yulius wait`), so Claude Code's evaluator gives no verdict
/// at its Stop. The shell's end wakes the session, and the woken turn's verdict, achieved, comes
/// seconds after its Stop hook. With `stop_lost` that Stop hook never reached Yulius (the hook
/// client fails open on an error answer or an unreachable server): the verdict and the record
/// that closes the turn still settle it. The task is Done either way, never Working for good.
fn a_goal_achieved_in_a_woken_turn_makes_the_task_done(stop_lost: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_background: Some(4),
            goal_met_on_wake_ms: Some(3_000),
            wake_stop_lost: stop_lost,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the achieved goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "Goal achieved")
        })
        .await?;
        wait_for("Done without shells", Duration::from_secs(30), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_goal_achieved_in_the_turn_a_shell_woke_makes_the_task_done() -> TestResult {
    a_goal_achieved_in_a_woken_turn_makes_the_task_done(false)
}

#[test]
fn a_goal_achieved_after_a_stop_yulius_never_answered_makes_the_task_done() -> TestResult {
    a_goal_achieved_in_a_woken_turn_makes_the_task_done(true)
}

/// The working stretches (`working_since`) the task `id` published while it worked, from the
/// events received so far. A lagging receiver fails: lost events would prove nothing.
fn working_stretches(
    events: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    id: &TaskId,
) -> Result<Vec<Option<String>>, BoxError> {
    let mut stretches = Vec::new();
    loop {
        match events.try_recv() {
            Ok(envelope) => {
                if let UiEvent::TaskUpdated { task } = envelope.event
                    && task.id == id.as_str()
                    && task.activity.as_deref() == Some("working")
                {
                    stretches.push(task.working_since);
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => return Ok(stretches),
            Err(err) => return Err(format!("the task's events were not all read: {err}").into()),
        }
    }
}

/// Whether the task `id` has no working stretch in the store (it ended).
async fn stretch_ended(app: &App, id: &TaskId) -> bool {
    app.deps
        .store
        .task_times(id)
        .await
        .ok()
        .flatten()
        .is_some_and(|t| t.working_since.is_none())
}

/// SPEC-309 WSW-001/WSW-002 (#894, task #295: "working 37m" for a task that had worked for
/// hours): the goal turn leaves a background shell (its `yulius wait`), and Claude Code wakes the
/// session only seconds after the shell exits (it polls), so Yulius's probe sees no background
/// work left first, which ends the working stretch. The woken turn continues that stretch:
/// everything the task publishes while it works carries the first turn's start. With
/// `prompt_lost` the wake's prompt hook never reaches Yulius, which reads the wake-up from the
/// transcript. A prompt the person types afterwards starts a new stretch.
fn a_woken_turn_keeps_the_working_stretch(prompt_lost: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_background: Some(4),
            goal_met_on_wake_ms: Some(1_000),
            wake_prompt_lost: prompt_lost,
            // Well past the probe's period: the probe sees the shell gone before the wake.
            wake_delay_ms: Some(u64::try_from(yulius_app::finish::SHELL_PROBE_MS)? + 5_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let mut events = app.events.subscribe();
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("idle with its shell", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("waiting".into()), Some(1)))
        })
        .await?;
        wait_for("the shell's end, seen", Duration::from_secs(30), || async {
            shown(&app, &id).await.ok() == Some((Some("waiting".into()), Some(0)))
        })
        .await?;
        wait_for(
            "the stretch ended with the shell",
            Duration::from_secs(5),
            || stretch_ended(&app, &id),
        )
        .await?;
        assert!(
            !screen_contains(&app, &id, "BACKGROUND DONE").await,
            "the probe saw the shell's end before Claude Code woke the session"
        );
        wait_for(
            "Done after the woken turn",
            Duration::from_secs(30),
            || async { shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0))) },
        )
        .await?;
        let stretches = working_stretches(&mut events, &id)?;
        let first = stretches.first().cloned().flatten();
        assert!(
            first.is_some(),
            "the task published its work: {stretches:?}"
        );
        assert!(stretches.len() >= 2, "both turns worked: {stretches:?}");
        assert!(
            stretches.iter().all(|s| *s == first),
            "one working stretch through the woken turn: {stretches:?}"
        );

        // The person types: a new stretch (WSW-002), although the task stayed in the state its
        // last stretch ended in.
        wait_for(
            "the stretch ended with the turn",
            Duration::from_secs(5),
            || stretch_ended(&app, &id),
        )
        .await?;
        app.send_input(&id, b"one more\r").await?;
        wait_for("the typed turn", Duration::from_secs(20), || {
            screen_contains(&app, &id, "ECHO: one more")
        })
        .await?;
        wait_for("Your turn", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        let typed = working_stretches(&mut events, &id)?;
        assert!(!typed.is_empty(), "the typed turn worked");
        assert!(
            typed.iter().all(|s| s.is_some() && *s > first),
            "a new stretch after {first:?}: {typed:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_turn_its_shell_wakes_keeps_the_working_stretch() -> TestResult {
    a_woken_turn_keeps_the_working_stretch(false)
}

#[test]
fn a_wake_read_from_the_transcript_keeps_the_working_stretch() -> TestResult {
    a_woken_turn_keeps_the_working_stretch(true)
}

/// SPEC-079 BGW-001 (#305), SES-101: under a limit of one session, a Done task with a background
/// shell still holds the slot; once the shell ends, the queued task starts at once (no tick runs
/// in this test, so without the drain it would never start).
#[test]
fn a_done_task_whose_shell_ends_frees_its_slot_at_once() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_background: Some(10),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(&data, &fake, agent, 0, None, None, false, Some(1)).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let first = goal_task(&app, &sandbox, &user, "Start the dev server").await?;
        wait_for("Done with its shell", Duration::from_secs(90), || async {
            shown(&app, &first)
                .await
                .ok()
                .is_some_and(|(_, shells)| shells == Some(1))
                && activity(&app, &first).await == Some(Activity::Done)
        })
        .await?;
        let second = goal_task(&app, &sandbox, &user, "Second").await?;
        assert_eq!(task(&app, &second).await?.status, TaskStatus::Queued);
        assert!(
            !screen_contains(&app, &first, "BACKGROUND DONE").await,
            "the shell still runs"
        );
        wait_for("the shell's end", Duration::from_secs(30), || {
            screen_contains(&app, &first, "BACKGROUND DONE")
        })
        .await?;
        wait_for(
            "the queued task to start",
            Duration::from_secs(20),
            || async {
                task(&app, &second)
                    .await
                    .is_ok_and(|t| t.status == TaskStatus::Running)
            },
        )
        .await?;
        stop(&app, &second).await?;
        stop(&app, &first).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// STA-001 (SPEC-032, tasks #24 and #25): the goal turn ends with a background agent still
/// working (Claude Code's `turn_duration` says one is pending): the task is not Done, it is
/// Working with that background work said, until the agent finishes and wakes the session.
#[test]
fn a_pending_background_agent_keeps_the_task_working_until_it_finishes() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_agent: Some(20),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Review the merge").await?;
        wait_for("the achieved goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "Goal achieved")
        })
        .await?;
        wait_for("one pending agent", Duration::from_secs(40), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(_, shells)| shells == Some(1))
        })
        .await?;
        assert!(
            !screen_contains(&app, &id, "AGENT DONE").await,
            "the agent still works"
        );
        // The agent ends and wakes the session; the turn after it says nothing is pending.
        wait_for("the agent's end", Duration::from_secs(45), || {
            screen_contains(&app, &id, "AGENT DONE")
        })
        .await?;
        // Nothing pending and not Working: the fake's wake-up turn settles at once, so the goal
        // still counts; a real session's later wake-up turn reads "Your turn" (the prompt-like
        // line resets the goal), which this check does not pin.
        wait_for("nothing pending, idle", Duration::from_secs(60), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(activity, shells)| {
                    shells == Some(0) && matches!(activity.as_deref(), Some("done" | "waiting"))
                })
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-079 BGW-006 (task #74): the goal turn's background agent first stops with background work
/// of its own still running. Its notification (`completed`, the note that its result may be
/// interim) wakes the session, and the turn record after it counts no agent. The task stays
/// Working, the agent counted, until the same agent's final notification; then it is idle with
/// nothing running.
#[test]
fn an_agent_that_stopped_with_its_own_work_running_keeps_the_task_working() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_agent: Some(8),
            goal_agent_interim: Some(10),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Review the merge").await?;
        wait_for("the interim notification", Duration::from_secs(90), || {
            screen_contains(&app, &id, "AGENT INTERIM")
        })
        .await?;
        // Until the final notification the task never reads idle with nothing running (Your
        // turn, or Done), and it reads idle with the agent counted (Working).
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut counted = false;
        while !screen_contains(&app, &id, "AGENT DONE").await {
            let (activity, shells) = shown(&app, &id).await?;
            let idle = matches!(activity.as_deref(), Some("done" | "waiting"));
            if idle && shells == Some(0) {
                return Err(
                    "idle with nothing running before the agent's final notification".into(),
                );
            }
            counted |= idle && shells == Some(1);
            if Instant::now() > deadline {
                return Err("timed out waiting for the agent's final notification".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            counted,
            "the stopped agent was counted while the task was idle"
        );
        wait_for("nothing running, idle", Duration::from_secs(30), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(activity, shells)| {
                    shells == Some(0) && matches!(activity.as_deref(), Some("done" | "waiting"))
                })
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-079 BGW-003 (#265): the goal turn leaves a Monitor stream (no process: only the transcript
/// says it runs). The task is Working with it counted until the stream ends (its notification wakes
/// the session) or, when it never reports, until its expiry.
fn a_monitor_counts_until_it_ends_or_expires(ends: bool) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_monitor: Some(16),
            goal_monitor_ends: ends,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Watch CI").await?;
        wait_for("the achieved goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "Goal achieved")
        })
        .await?;
        wait_for("the monitor counted", Duration::from_secs(10), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(_, shells)| shells == Some(1))
        })
        .await?;
        if ends {
            wait_for("the stream's end", Duration::from_secs(20), || {
                screen_contains(&app, &id, "MONITOR DONE")
            })
            .await?;
        }
        // Ended: at once; expired: after its 16 s and the next probe (every 5 s).
        wait_for("nothing running, idle", Duration::from_secs(30), || async {
            shown(&app, &id)
                .await
                .ok()
                .is_some_and(|(activity, shells)| {
                    shells == Some(0) && matches!(activity.as_deref(), Some("done" | "waiting"))
                })
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_live_monitor_keeps_the_task_working_until_it_ends() -> TestResult {
    a_monitor_counts_until_it_ends_or_expires(true)
}

#[test]
fn a_monitor_that_never_reports_counts_until_it_expires() -> TestResult {
    a_monitor_counts_until_it_ends_or_expires(false)
}

/// A second subscription, `a-claude-2`, with room.
async fn spare_subscription(app: &App) -> TestResult {
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a-claude-2")?,
            provider: Provider::Anthropic,
            label: "Claude spare".into(),
            credential: Some(
                app.deps
                    .crypto
                    .seal(concat!("sk-ant-oat01-", "spare-credential").as_bytes())?,
            ),
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
    Ok(())
}

/// The first subscription, `a-claude`, reaches the threshold at which a turn's end moves its
/// session to another one.
async fn first_subscription_nearly_full(app: &App) -> TestResult {
    let near = yulius_domain::usage::AccountUsage {
        primary: Some(yulius_domain::usage::UsageWindow {
            used_percent: Some(99.0),
            window_minutes: Some(300),
            resets_at: Some(Timestamp(4_000_000_000_000)),
        }),
        ..yulius_domain::usage::AccountUsage::default()
    };
    let near = yulius_app::usage_json::to_json(&near);
    app.deps
        .store
        .set_account_state(
            &AccountId::parse("a-claude")?,
            AccountStatus::Ok,
            Some(&near),
            None,
            app.now(),
        )
        .await?;
    Ok(())
}

/// Whether the task's newest run is live on the spare subscription.
async fn runs_on_the_spare(app: &App, id: &TaskId) -> bool {
    app.deps
        .store
        .attempts(id)
        .await
        .unwrap_or_default()
        .last()
        .is_some_and(|a| {
            a.account_id.as_ref().map(AccountId::as_str) == Some("a-claude-2")
                && !a.state.is_terminal()
        })
}

/// SPEC-483 SWT-001: the look at the session's background work gives no answer (the probe timed
/// out, the process table could not be read) at a turn's end that would move the session to
/// another subscription. Work that cannot be told is taken as running, as everywhere else that
/// asks: the session stays and is answered, and moves at a later turn's end, once the look
/// answers.
#[test]
fn a_switch_waits_while_the_background_work_cannot_be_told() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
            "CARGO_BIN_EXE_yulius"
        ))));
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_delay_ms: Some(3_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(
            &data,
            &fake,
            agent,
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        spare_subscription(&app).await?;
        host.silence_probe(true);
        let id = goal_task(&app, &sandbox, &user, "Review the merge").await?;
        wait_for("the goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        first_subscription_nearly_full(&app).await?;
        let runs = || async { app.deps.store.attempts(&id).await.unwrap_or_default().len() };
        // The turn's end is answered (the task is done) or the session is moved.
        wait_for("the goal turn's end", Duration::from_secs(90), || async {
            activity(&app, &id).await == Some(Activity::Done) || runs().await >= 2
        })
        .await?;
        assert_eq!(
            runs().await,
            1,
            "no move while the background work cannot be told: {}",
            run_states(&app, &id).await
        );
        // The look answers again: the next turn's end moves the session (a turn longer than
        // the time in which its prompt's record may still be unread, SPEC-471 CCQ-004).
        host.silence_probe(false);
        app.send_input(&id, b"/work 3000 go on\r").await?;
        wait_for("the move", Duration::from_secs(60), || {
            runs_on_the_spare(&app, &id)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-079 BGW-004 (#271): the goal turn launches a background agent while the task's
/// subscription is at its threshold. The Stop decides a proactive switch, which would end the
/// agent: it waits while the agent is pending and is made at the Stop after the agent finished.
#[test]
fn a_switch_waits_for_a_background_agent_launched_in_the_turn() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_agent: Some(10),
            goal_delay_ms: Some(3_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let store = &app.deps.store;
        spare_subscription(&app).await?;
        let id = goal_task(&app, &sandbox, &user, "Review the merge").await?;
        wait_for("the goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        let first = store.live_attempt(&id).await?.ok_or("attempt")?;
        assert_eq!(
            first.account_id.as_ref().map(AccountId::as_str),
            Some("a-claude")
        );
        // The task's subscription reaches the switch threshold before the turn ends.
        first_subscription_nearly_full(&app).await?;
        let switched = || async { store.attempts(&id).await.unwrap_or_default().len() >= 2 };
        wait_for("the goal turn's end", Duration::from_secs(90), || async {
            screen_contains(&app, &id, "Goal achieved").await || switched().await
        })
        .await?;
        assert!(
            !switched().await,
            "no switch while the agent launched in the turn is pending"
        );
        assert!(!screen_contains(&app, &id, "AGENT DONE").await);
        wait_for("the agent's end", Duration::from_secs(30), || {
            screen_contains(&app, &id, "AGENT DONE")
        })
        .await?;
        // The wake-up turn's Stop makes the switch.
        wait_for("the switch", Duration::from_secs(60), || {
            runs_on_the_spare(&app, &id)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-140 STP-001 (#464): a stop that comes while the task's run is launching (spawned, Yulius
/// not attached yet) finds no run to ask. The run is asked once it is attached, so it ends at once
/// as stopped, not when the escalation terminates it (here 60 s, beyond the wait).
#[test]
fn a_stop_while_the_run_is_launching_ends_it_once_attached() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
            "CARGO_BIN_EXE_yulius"
        ))));
        host.arm();
        let (app, _) = start_on(
            &data,
            &fake,
            FakeAgentOptions::default(),
            0,
            Some(host.clone()),
            Some(60_000),
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
        app.stop_task_now(&id).await?;
        host.release();
        wait_for("the stop", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled)
        );
        let attempts = app.deps.store.attempts(&id).await?;
        let run = attempts.last().ok_or("no run")?;
        assert_ne!(run.state, AttemptState::Lost, "{run:?}");
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-140 STP-002 (review): a stop that ended no run leaves no intent for the next run. Here a
/// switch's old run has ended and its new run waits for the launch (another task's launch holds
/// it), the task is stopped then, and the switch gives up and rests it (SPEC-250 STW-006: not
/// stopping with no run until a second stop). Continued, the task's resumed run is not shut down
/// when it attaches: its goal is typed and it stays live.
#[test]
fn a_stop_that_ended_no_run_does_not_end_the_next_one() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
            "CARGO_BIN_EXE_yulius"
        ))));
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(
            &data,
            &fake,
            agent,
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        wait_for("the launch goal", Duration::from_secs(60), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        // Another task's launch holds the launch lock while its attach is held.
        host.arm();
        let holder = goal_task(&app, &sandbox, &user, "Hold the launch").await?;
        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
        let switch = {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move { app.switch_task_now(&id, None, "manual", None).await })
        };
        wait_for("the old run's end", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Switching)
                && app
                    .deps
                    .store
                    .live_attempt(&id)
                    .await
                    .is_ok_and(|a| a.is_none())
        })
        .await?;
        app.stop_task_now(&id).await?;
        host.release();
        let refused = tokio::time::timeout(Duration::from_secs(60), switch).await??;
        assert!(refused.is_err(), "{refused:?}");
        // The switch that gives up rests the task its stop left with no run (SPEC-250 STW-006).
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled),
            "{}",
            run_states(&app, &id).await
        );
        let runs = app.deps.store.attempts(&id).await?.len();
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.continue_task(
            &ctx,
            id.as_str(),
            &yulius_contracts::api::tasks::ContinueTask { account_id: None },
        )
        .await?;
        // The resumed run gets its goal typed; ending first is the failure.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let attempts = app.deps.store.attempts(&id).await?;
            if let Some(resumed) = attempts.get(runs) {
                if resumed.state.is_terminal() {
                    return Err(format!("the resumed run ended: {resumed:?}").into());
                }
                if screen_contains(&app, &id, "ECHO: /goal Tidy the parser").await {
                    break;
                }
            }
            if Instant::now() > deadline {
                return Err("timed out waiting for the resumed run's goal".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        support::stop_tasks(&app, &[&id, &holder]).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Waits for `wait` from a pause point's hook, a plain closure the server calls from its async
/// code on a runtime worker: the worker is handed off meanwhile, so the server's other work goes on.
fn wait_in_hook<F: std::future::Future>(handle: &tokio::runtime::Handle, wait: F) -> F::Output {
    tokio::task::block_in_place(|| handle.block_on(wait))
}

/// The task's status and its runs' states and end reasons, for a failure message.
async fn run_states(app: &App, id: &TaskId) -> String {
    let status = task(app, id)
        .await
        .map_or_else(|e| e.to_string(), |t| t.status.as_str().to_owned());
    let runs: Vec<String> = app
        .deps
        .store
        .attempts(id)
        .await
        .unwrap_or_default()
        .iter()
        .map(|a| format!("{:?} {:?}", a.state, a.exit_reason))
        .collect();
    format!("{status} (runs: {})", runs.join(", "))
}

/// Starts a goal task on the gated host and waits for its goal.
async fn gated_goal_task(
    data: &Path,
    fake: &Path,
) -> Result<(Arc<App>, Arc<support::gated::GatedHost>, TaskId), BoxError> {
    let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
        "CARGO_BIN_EXE_yulius"
    ))));
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };
    let (app, _) = start_on(data, fake, agent, 0, Some(host.clone()), None, false, None).await?;
    let (sandbox, user) = seed(&app, data).await?;
    let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
    wait_for("the launch goal", Duration::from_secs(60), || {
        screen_contains(&app, &id, "GOAL SET")
    })
    .await?;
    Ok((app, host, id))
}

/// [`gated_goal_task`] on a server whose switch waits `switch_end_ms` for the old session's end.
async fn gated_goal_task_ending(
    data: &Path,
    fake: &Path,
    switch_end_ms: u64,
) -> Result<(Arc<App>, Arc<support::gated::GatedHost>, TaskId), BoxError> {
    let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
        "CARGO_BIN_EXE_yulius"
    ))));
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };
    let (app, _) = start_holding(
        data,
        fake,
        agent,
        0,
        Some(host.clone()),
        (None, Some(switch_end_ms)),
        false,
        None,
        None,
        None,
    )
    .await?;
    let (sandbox, user) = seed(&app, data).await?;
    let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
    wait_for("the launch goal", Duration::from_secs(60), || {
        screen_contains(&app, &id, "GOAL SET")
    })
    .await?;
    Ok((app, host, id))
}

/// SPEC-483 SWT-002: the session a switch asked to end does not end in time, and its host cannot
/// be terminated (a process that survives the kill). No new session starts beside it: the run
/// stays the task's live run, the task runs and says so to a person, and the switch failed.
/// The run's end is no longer the switch's: a session that leaves by itself afterwards ends as
/// one that left, and closes its task.
#[test]
fn a_switch_whose_old_run_cannot_be_ended_starts_no_new_run() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        let mut watch = app.runtime(&id).hub.watch();
        host.refuse_ends(true);
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        assert_eq!(
            outcome,
            Err("the session could not be ended, so it was not switched".to_owned()),
            "{}",
            run_states(&app, &id).await
        );
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 1, "{}", run_states(&app, &id).await);
        assert_eq!(runs[0].state, AttemptState::Live);
        assert_eq!(
            app.deps.store.live_attempt(&id).await?.map(|a| a.id),
            Some(run.id)
        );
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let why = support::attention(&app, id.as_str()).await?;
        assert!(
            why["problem"]
                .as_str()
                .is_some_and(|p| p.starts_with("Yulius could not end this session")),
            "{why}"
        );
        assert_eq!(why["action"], "Stop the task to try again.");
        let switches = app.deps.store.switches(&id).await?;
        assert_eq!(
            switches.last().map(|s| s.outcome.as_str()),
            Some("failed"),
            "the switch is recorded as failed"
        );
        let said = notices_of(&mut watch);
        assert!(
            said.iter()
                .any(|line| line == "↻ The session was not switched: it could not be ended."),
            "{said:?}"
        );
        assert!(
            app.switch_cooling(&id),
            "the next turn's end stays on this subscription"
        );
        host.refuse_ends(false);
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the task at rest", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Closed,
            "{}",
            run_states(&app, &id).await
        );
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].exit_reason.as_deref(), Some("exited"));
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002: the session a switch asked to end does not end in time and its host is
/// terminated. The switch failed and starts no new run: the task rests as
/// `stopped_switch_timeout` with its one run ended.
#[test]
fn a_switch_whose_old_run_had_to_be_killed_starts_no_new_run() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        host.deafen(true);
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        timed_out_switch(&app, &id, &outcome, "lost").await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// What a switch that gave its old run up leaves, whoever recorded that run's end (as
/// `exit_reason`): the switch failed saying so, the task has its one run, ended, and rests as
/// `stopped_switch_timeout`.
async fn timed_out_switch(
    app: &Arc<App>,
    id: &TaskId,
    outcome: &Result<(), String>,
    exit_reason: &str,
) -> TestResult {
    let states = run_states(app, id).await;
    assert_eq!(
        outcome,
        &Err("the session did not end in time during the switch".to_owned()),
        "{states}"
    );
    let runs = app.deps.store.attempts(id).await?;
    assert_eq!(runs.len(), 1, "{states}");
    assert!(runs[0].state.is_terminal(), "{states}");
    assert_eq!(
        runs[0].exit_reason.as_deref(),
        Some(exit_reason),
        "{states}"
    );
    assert_eq!(
        task(app, id).await?.status,
        TaskStatus::Stopped(StopReason::SwitchTimeout),
        "{states}"
    );
    assert!(app.deps.store.live_attempt(id).await?.is_none());
    let switches = app.deps.store.switches(id).await?;
    assert_eq!(switches.last().map(|s| s.outcome.as_str()), Some("failed"));
    Ok(())
}

/// Runs `then` in the switch of `id`, once it gave its old run up and before it terminates that
/// run's host; `then` returns once the run's end is recorded.
fn as_the_switch_ends_its_run<F, Fut>(app: &Arc<App>, id: &TaskId, then: F)
where
    F: FnOnce(Arc<App>, TaskId) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = TestResult>,
{
    let (app, key, id) = (app.clone(), id.clone(), id.clone());
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(
        yulius_app::race::Point::SwitchEndsRun,
        Path::new(key.as_str()),
        move || {
            let recorded = wait_in_hook(&handle, async {
                let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
                then(app.clone(), id.clone()).await?;
                wait_for("the run's end", Duration::from_secs(30), || async {
                    matches!(
                        app.deps.store.attempt(&run.id).await,
                        Ok(Some(a)) if a.state.is_terminal()
                    )
                })
                .await
            });
            if let Err(err) = recorded {
                eprintln!("{err}");
            }
        },
    );
}

/// SPEC-483 SWT-002 (review): the run's reader sees its host go before the switch that gave the
/// run up records the loss. The loss is that switch's all the same: the task rests as
/// `stopped_switch_timeout`, not `stopped_lost`.
#[test]
fn a_run_its_switch_gave_up_is_that_switchs_loss_whoever_sees_it_first() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        host.deafen(true);
        as_the_switch_ends_its_run(&app, &id, |app, id| async move {
            kill_run_host(&app, &id).await
        });
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        timed_out_switch(&app, &id, &outcome, "lost").await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002 (review): the run exits by itself after the switch gave it up, before its
/// host is terminated. Its exit is recorded as the switch's timeout and rests the task the same
/// way: it is not left switching with no run.
#[test]
fn a_run_that_exits_as_its_switch_gives_it_up_rests_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        host.deafen(true);
        as_the_switch_ends_its_run(&app, &id, |app, id| async move {
            Ok(app.send_input(&id, b"/exit\r").await?)
        });
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        timed_out_switch(&app, &id, &outcome, "switch_timeout").await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002 (review): the same exit, and the host then cannot be terminated (the test
/// host refuses). The run ended, so nothing is said about a session that could not be ended and
/// the task is not put back to running with no run.
#[test]
fn a_run_that_exits_before_a_refused_kill_rests_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        let mut watch = app.runtime(&id).hub.watch();
        host.refuse_ends(true);
        as_the_switch_ends_its_run(&app, &id, |app, id| async move {
            Ok(app.send_input(&id, b"/exit\r").await?)
        });
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        timed_out_switch(&app, &id, &outcome, "switch_timeout").await?;
        let said = notices_of(&mut watch);
        assert!(
            !said.iter().any(|line| line.contains("could not")),
            "{said:?}"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002 (review): the old run ends by itself just as the switch's wait runs out: its
/// end is being recorded (the switch's intent was taken) when the switch looks. The switch waits
/// for that word and goes on: the run ended as switched and the new run starts, instead of a
/// task left switching with no run.
#[test]
fn a_run_whose_end_is_being_recorded_as_the_wait_runs_out_is_switched() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        // Long enough for the session to end at the switch's request before the wait is over:
        // the test's one assumption about time (a session slower than that fails it).
        let (app, _host, id) = gated_goal_task_ending(&data, &fake, 5_000).await?;
        let (waited, wait_over) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let held_until_then = held.clone();
        yulius_app::race::on(
            yulius_app::race::Point::RunEndTaken,
            Path::new(id.as_str()),
            move || {
                // The worker is handed off meanwhile: the switch's timer runs on.
                let over =
                    tokio::task::block_in_place(|| wait_over.recv_timeout(Duration::from_secs(60)));
                held_until_then.store(over.is_ok(), std::sync::atomic::Ordering::SeqCst);
            },
        );
        yulius_app::race::on(
            yulius_app::race::Point::SwitchWaitOver,
            Path::new(id.as_str()),
            move || {
                let _ = waited.send(());
            },
        );
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        assert!(
            held.load(std::sync::atomic::Ordering::SeqCst),
            "the run's end was held until the switch's wait ran out"
        );
        assert_eq!(outcome, Ok(()), "{}", run_states(&app, &id).await);
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 2, "{}", run_states(&app, &id).await);
        assert_eq!(runs[0].exit_reason.as_deref(), Some("switched"));
        assert_eq!(runs[1].state, AttemptState::Live);
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        stop(&app, &id).await?;
        expect_stopped(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002 (review, round 2): the session leaves by itself after the switch made the
/// task `switching` and before the switch claims the run's end. Its end closes the task, as it
/// does without a switch, and the switch gives way: the task is not left switching with no run,
/// and nothing says a session did not end in time.
#[test]
fn a_run_that_ends_before_its_switch_claims_it_closes_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        let mut watch = app.runtime(&id).hub.watch();
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::SwitchDecided,
                Path::new(key.as_str()),
                move || {
                    let ended = wait_in_hook(&handle, async {
                        app.send_input(&id, b"/exit\r").await?;
                        wait_for("the run's end", Duration::from_secs(30), || async {
                            matches!(
                                app.deps.store.attempt(&run.id).await,
                                Ok(Some(a)) if a.state.is_terminal()
                            )
                        })
                        .await
                    });
                    if let Err(err) = ended {
                        eprintln!("{err}");
                    }
                },
            );
        }
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        let states = run_states(&app, &id).await;
        assert_eq!(
            outcome,
            Err("the task was closed during the switch".to_owned()),
            "{states}"
        );
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Closed,
            "{states}"
        );
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 1, "{states}");
        assert_eq!(runs[0].exit_reason.as_deref(), Some("exited"), "{states}");
        let said = notices_of(&mut watch);
        assert!(
            !said.iter().any(|line| line.contains("in time")),
            "{said:?}"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-483 SWT-002 (review, round 2): the run's exit is being recorded (its recorder took the
/// given-up intent) when the kill is refused and the switch puts the task back to running. The
/// switch waits for that record before it asks whether the run lives: the run ended, so nothing
/// says a session could not be ended, and the task rests as after the timeout.
#[test]
fn a_run_whose_exit_is_being_recorded_at_a_refused_kill_rests_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task_ending(&data, &fake, 300).await?;
        let mut watch = app.runtime(&id).hub.watch();
        host.refuse_ends(true);
        let (taken, end_taken) = std::sync::mpsc::channel::<()>();
        {
            // The run's recorder holds the intent it took until the task runs again.
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::RunEndTaken,
                Path::new(key.as_str()),
                move || {
                    let _ = taken.send(());
                    let running = wait_in_hook(&handle, async {
                        wait_for(
                            "the task running again",
                            Duration::from_secs(30),
                            || async {
                                task(&app, &id)
                                    .await
                                    .is_ok_and(|t| t.status == TaskStatus::Running)
                            },
                        )
                        .await
                    });
                    if let Err(err) = running {
                        eprintln!("{err}");
                    }
                },
            );
        }
        {
            // The agent dies once the switch gave its run up; the kill is tried (and refused)
            // once the run's recorder has taken the intent.
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::SwitchEndsRun,
                Path::new(key.as_str()),
                move || {
                    // The agent dies: an exit that is not a session leaving.
                    let died = wait_in_hook(&handle, async {
                        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
                        let agent = run.child_pid.ok_or("no agent pid")?;
                        nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(i32::try_from(agent)?),
                            nix::sys::signal::Signal::SIGKILL,
                        )?;
                        Ok::<(), BoxError>(())
                    });
                    let seen = tokio::task::block_in_place(|| {
                        end_taken.recv_timeout(Duration::from_secs(30))
                    });
                    if died.is_err() || seen.is_err() {
                        eprintln!("the run's end was not taken");
                    }
                },
            );
        }
        let outcome = switch_result(switch_on_worker(&app, &id)).await?;
        timed_out_switch(&app, &id, &outcome, "switch_timeout").await?;
        let said = notices_of(&mut watch);
        assert!(
            !said.iter().any(|line| line.contains("could not")),
            "{said:?}"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The notices said so far on a watch of a task's terminal.
fn notices_of(
    watch: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>,
) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
    said
}

/// Stops `id` on a runtime worker, so a pause point's hook can wait there.
async fn stop_on_worker(app: &Arc<App>, id: &TaskId) -> TestResult {
    let stop = {
        let (app, id) = (app.clone(), id.clone());
        tokio::spawn(async move { app.stop_task_now(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(90), stop).await???;
    Ok(())
}

/// Waits until `id` rests: it must be stopped, with no live run.
async fn expect_stopped(app: &Arc<App>, id: &TaskId) -> TestResult {
    let rests = wait_for("the stop", Duration::from_secs(30), || async {
        task(app, id).await.is_ok_and(|t| !t.status.is_live())
    })
    .await;
    if let Err(err) = rests {
        return Err(format!("{err}: {}", run_states(app, id).await).into());
    }
    assert_eq!(
        task(app, id).await?.status,
        TaskStatus::Stopped(StopReason::Cancelled),
        "{}",
        run_states(app, id).await
    );
    assert!(app.deps.store.live_attempt(id).await?.is_none());
    Ok(())
}

/// SPEC-250 STW-001 (#527): a stop reads the task while its switch's new run is launching
/// (`Switching`), and the switch completes (`Running`) before the stop's status change. The stop
/// reads the task again and stops it from there: the task ends stopped, not running with no run.
#[test]
fn a_stop_that_loses_to_a_completing_switch_still_ends_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task(&data, &fake).await?;
        // The switch's new run is held at its spawn (the task reads Switching), then at its attach
        // (the switch is completed, the run not attached: nothing ends it yet).
        host.arm_spawn();
        host.arm();
        let switch = {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move { app.switch_task_now(&id, None, "manual", None).await })
        };
        tokio::time::timeout(Duration::from_secs(60), host.spawn_reached()).await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Switching);
        // Between the stop's decision and its status change, the switch completes.
        {
            let (app, key, id, host) = (app.clone(), id.clone(), id.clone(), host.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::StopDecided,
                Path::new(key.as_str()),
                move || {
                    host.release_spawn();
                    let completed = wait_in_hook(&handle, async {
                        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
                        let status = task(&app, &id).await?.status;
                        if status == TaskStatus::Running {
                            Ok(())
                        } else {
                            Err(BoxError::from(format!("the switch left {status:?}")))
                        }
                    });
                    if let Err(err) = completed {
                        eprintln!("the switch did not complete: {err}");
                    }
                },
            );
        }
        stop_on_worker(&app, &id).await?;
        // The stop made its change from Running: the run is ended once attached.
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopping,
            "{}",
            run_states(&app, &id).await
        );
        host.release();
        expect_stopped(&app, &id).await?;
        tokio::time::timeout(Duration::from_secs(60), switch).await???;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-003: the run ends by itself after a stop decided to end it, before the stop's
/// status change (the agent exited at that moment). Its end rests the task, so the task ends
/// stopped, not stopping with no run left to end.
#[test]
fn a_run_that_ends_before_the_stops_change_still_ends_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::StopDecided,
                Path::new(key.as_str()),
                move || {
                    let ended = wait_in_hook(&handle, async {
                        app.send_input(&id, b"/exit\r").await?;
                        wait_for("the run's end", Duration::from_secs(30), || async {
                            matches!(
                                app.deps.store.attempt(&run.id).await,
                                Ok(Some(a)) if a.state.is_terminal()
                            )
                        })
                        .await?;
                        // Its end is recorded: the task rests (only with the fix).
                        wait_for("the task at rest", Duration::from_secs(10), || async {
                            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
                        })
                        .await
                    });
                    if let Err(err) = ended {
                        eprintln!("{err}");
                    }
                },
            );
        }
        stop_on_worker(&app, &id).await?;
        expect_stopped(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-006: a stop lands while a switch waits for its old run to end. The stop takes
/// over that end, so the switch starts no new run: the task ends stopped with its one run ended,
/// not stopped (or running) with a new run nobody ends.
#[test]
fn a_stop_while_a_switch_ends_its_old_run_starts_no_new_run() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task(&data, &fake).await?;
        // The old run cannot be ended: the switch waits for it.
        host.refuse_ends(true);
        let (switched, mut switch_done) = tokio::sync::watch::channel(None);
        {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move {
                let result = app.switch_task_now(&id, None, "manual", None).await;
                switched.send_replace(Some(result.map_err(|e| e.to_string())));
            });
        }
        // The switch set its intent and asked the old run to end (the request was dropped).
        tokio::time::timeout(Duration::from_secs(30), host.shutdown_refused()).await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Switching);
        // The stop's decision replaces the switch's: the switch either gives up or starts a run.
        {
            let (app, key, id, host) = (app.clone(), id.clone(), id.clone(), host.clone());
            let mut switch_done = switch_done.clone();
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::StopDecided,
                Path::new(key.as_str()),
                move || {
                    wait_in_hook(&handle, async {
                        let new_run = wait_for("a new run", Duration::from_secs(30), || async {
                            app.deps.store.attempts(&id).await.unwrap_or_default().len() >= 2
                        });
                        tokio::select! {
                            _ = switch_done.wait_for(Option::is_some) => {}
                            _ = new_run => {}
                        }
                    });
                    host.refuse_ends(false);
                },
            );
        }
        stop_on_worker(&app, &id).await?;
        expect_stopped(&app, &id).await?;
        tokio::time::timeout(
            Duration::from_secs(60),
            switch_done.wait_for(Option::is_some),
        )
        .await??;
        let outcome = switch_done.borrow().clone();
        assert!(matches!(outcome, Some(Err(_))), "{outcome:?}");
        assert_eq!(
            app.deps.store.attempts(&id).await?.len(),
            1,
            "{}",
            run_states(&app, &id).await
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Switches `id` on a runtime worker; the watch gets the switch's result.
fn switch_on_worker(
    app: &Arc<App>,
    id: &TaskId,
) -> tokio::sync::watch::Receiver<Option<Result<(), String>>> {
    let (done, result) = tokio::sync::watch::channel(None);
    let (app, id) = (app.clone(), id.clone());
    tokio::spawn(async move {
        let switched = app.switch_task_now(&id, None, "manual", None).await;
        done.send_replace(Some(switched.map_err(|e| e.to_string())));
    });
    result
}

/// Waits for the switch's result.
async fn switch_result(
    mut result: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
) -> Result<Result<(), String>, BoxError> {
    tokio::time::timeout(Duration::from_secs(90), result.wait_for(Option::is_some)).await??;
    let outcome = result.borrow().clone();
    outcome.ok_or_else(|| "no switch result".into())
}

/// SPEC-250 STW-006 (review): a stop lands after the switch's status change, before the switch
/// sets its intent for the old run. The switch keeps the stop's intent and gives up: the task ends
/// stopped with its one run, not stopping with no run (or queued again).
#[test]
fn a_stop_before_a_switch_sets_its_intent_starts_no_new_run() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let (stopped, mut stop_made) = tokio::sync::watch::channel(None);
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::SwitchDecided,
                Path::new(key.as_str()),
                move || {
                    let result = wait_in_hook(&handle, app.stop_task_now(&id));
                    stopped.send_replace(Some(result.map_err(|e| e.to_string())));
                },
            );
        }
        let switched = switch_result(switch_on_worker(&app, &id)).await?;
        assert!(
            switched.as_ref().is_err_and(|e| e.contains("stopped")),
            "{switched:?}"
        );
        tokio::time::timeout(Duration::from_secs(30), stop_made.wait_for(Option::is_some))
            .await??;
        let made = stop_made.borrow().clone();
        assert_eq!(made, Some(Ok(())));
        expect_stopped(&app, &id).await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-006 (review): a close reads the task running, a switch starts and asks the old
/// run to end, then the close sets its intent. The switch gives up and the old run's end closes the
/// task from `Switching`: it ends closed, not switching with no run. The run's end is held until
/// the switch answered, so the switch says why from what the close told it, not from a status the
/// end has not changed yet.
#[test]
fn a_close_that_overtakes_a_switch_closes_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task(&data, &fake).await?;
        host.refuse_ends(true);
        let (started, mut switch) = tokio::sync::watch::channel(None);
        {
            let (app, key, id, host) = (app.clone(), id.clone(), id.clone(), host.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::CloseDecided,
                Path::new(key.as_str()),
                move || {
                    let result = switch_on_worker(&app, &id);
                    started.send_replace(Some(result));
                    let asked = wait_in_hook(
                        &handle,
                        tokio::time::timeout(Duration::from_secs(30), host.shutdown_refused()),
                    );
                    if asked.is_err() {
                        eprintln!("the switch did not ask the old run to end");
                    }
                    host.refuse_ends(false);
                },
            );
        }
        // The switch says why only once the old run's end took the close's intent (and is held
        // before it closes the task).
        let (end_decided, mut decided) = tokio::sync::watch::channel(false);
        {
            let key = id.clone();
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::SwitchOvertaken,
                Path::new(key.as_str()),
                move || {
                    let reached = wait_in_hook(
                        &handle,
                        tokio::time::timeout(Duration::from_secs(30), decided.wait_for(|d| *d)),
                    );
                    if reached.is_err() {
                        eprintln!("the old run's end was not reached");
                    }
                },
            );
        }
        {
            let (key, mut started) = (id.clone(), switch.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::EndDecided,
                Path::new(key.as_str()),
                move || {
                    end_decided.send_replace(true);
                    let answered = wait_in_hook(&handle, async {
                        started.wait_for(Option::is_some).await?;
                        let mut result = started.borrow().clone().ok_or("no switch")?;
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            result.wait_for(Option::is_some),
                        )
                        .await??;
                        Ok::<(), BoxError>(())
                    });
                    if let Err(err) = answered {
                        eprintln!("the switch did not answer: {err}");
                    }
                },
            );
        }
        {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move { app.close_task_now(&id).await }).await??;
        }
        tokio::time::timeout(Duration::from_secs(5), switch.wait_for(Option::is_some)).await??;
        let result = switch.borrow().clone().ok_or("no switch")?;
        let switched = switch_result(result).await?;
        assert!(
            switched.as_ref().is_err_and(|e| e.contains("closed")),
            "{switched:?}"
        );
        let rests = wait_for("the close", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await;
        assert!(rests.is_ok(), "{}", run_states(&app, &id).await);
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Closed);
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Moves `id` from `from` to `to` in the store from a pause point's hook, as the server's own
/// writer of that change would; the watch says whether the change was made.
fn change_at(
    app: &Arc<App>,
    id: &TaskId,
    point: yulius_app::race::Point,
    from: TaskStatus,
    to: TaskStatus,
) -> tokio::sync::watch::Receiver<Option<bool>> {
    let (made, result) = tokio::sync::watch::channel(None);
    let (app, key, id) = (app.clone(), id.clone(), id.clone());
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(point, Path::new(key.as_str()), move || {
        let changed = wait_in_hook(
            &handle,
            app.deps
                .store
                .transition_task(&id, Some(from), to, None, app.now()),
        );
        made.send_replace(Some(matches!(changed, Ok(true))));
    });
    result
}

/// Pauses `id` for capacity when its switch reaches its decision, as `pause_task` does for a
/// Stop hook's wait.
fn pause_at_switch(app: &Arc<App>, id: &TaskId) -> tokio::sync::watch::Receiver<Option<bool>> {
    change_at(
        app,
        id,
        yulius_app::race::Point::SwitchDecided,
        TaskStatus::Switching,
        TaskStatus::WaitingCapacity,
    )
}

/// SPEC-250 STW-006 (review round 3): the switch goes on from `WaitingCapacity`, and before its
/// launch takes the lock, the capacity loop's start runs the task (`Running`). That start won: the
/// switch is completed, not overtaken.
#[test]
fn a_switch_whose_paused_task_another_start_runs_is_completed() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let paused = pause_at_switch(&app, &id);
        // The status the capacity loop's start leaves (its run is not simulated).
        let resumed = change_at(
            &app,
            &id,
            yulius_app::race::Point::SwitchLaunching,
            TaskStatus::WaitingCapacity,
            TaskStatus::Running,
        );
        let switched = switch_result(switch_on_worker(&app, &id)).await?;
        assert_eq!(
            paused.borrow().clone(),
            Some(true),
            "the hook paused the task"
        );
        assert_eq!(
            resumed.borrow().clone(),
            Some(true),
            "the hook ran the task"
        );
        assert_eq!(switched, Ok(()), "{}", run_states(&app, &id).await);
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        // No run is live (the start was simulated): a stop rests it after a second one.
        app.stop_task_now(&id).await?;
        app.stop_task_now(&id).await?;
        expect_stopped(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-006 (review round 2): a task paused for capacity during its switch (a Stop
/// hook's wait moves `Switching` to `WaitingCapacity`) is not overtaken: the switch goes on and
/// launches, as before SPEC-250.
#[test]
fn a_switch_paused_for_capacity_goes_on() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let paused = pause_at_switch(&app, &id);
        let switched = switch_result(switch_on_worker(&app, &id)).await?;
        assert_eq!(
            paused.borrow().clone(),
            Some(true),
            "the hook paused the task"
        );
        assert_eq!(switched, Ok(()), "{}", run_states(&app, &id).await);
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 2, "{}", run_states(&app, &id).await);
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Kills the host of `id`'s live run, as a crash would: the run is lost.
async fn kill_run_host(app: &App, id: &TaskId) -> TestResult {
    let run = app.deps.store.live_attempt(id).await?.ok_or("no run")?;
    let pid = run.ptyhost_pid.ok_or("no host pid")?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(pid)?),
        nix::sys::signal::Signal::SIGKILL,
    )?;
    Ok(())
}

/// SPEC-301 LRS-001 (#686): a run's connection is lost while the server runs, after output that
/// came since its last settled turn. The loss saves the hub's screen: the run's `final-screen`
/// shows that output, not the screen its last turn settled on.
#[test]
fn a_run_lost_while_the_server_runs_saves_its_live_screen() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        wait_for("Done", Duration::from_secs(90), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        app.send_input(&id, b"one more\r").await?;
        let run_dir = PathBuf::from(
            &app.deps
                .store
                .live_attempt(&id)
                .await?
                .ok_or("no run")?
                .run_dir,
        );
        // The settled turn's save, so the output below comes after it.
        wait_for("the settled turn's screen", Duration::from_secs(20), || {
            let saved = std::fs::read(run_dir.join("final-screen"))
                .is_ok_and(|s| String::from_utf8_lossy(&s).contains("ECHO: one more"));
            async move { saved }
        })
        .await?;
        assert_eq!(activity(&app, &id).await, Some(Activity::Waiting));
        // Output since the settled turn, fed to the hub as the run's reader would (the idle fake
        // agent writes none by itself).
        let hub = &app.runtime(&id).hub;
        hub.output(hub.output_seq() + 1, b"\r\nwritten after the settled turn");

        kill_run_host(&app, &id).await?;
        let rests = wait_for("the loss", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await;
        assert!(rests.is_ok(), "{}", run_states(&app, &id).await);
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(StopReason::Lost)
        );
        let screen = std::fs::read(run_dir.join("final-screen"))?;
        assert!(
            String::from_utf8_lossy(&screen).contains("written after the settled turn"),
            "the lost run's saved screen is its last one: {:?}",
            String::from_utf8_lossy(&screen)
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A continued task whose new run's attach is held: the first run (its screen `GOAL SET`, still in
/// the hub) stopped, the second spawned. The continue waits for its launch on a worker; stops arm
/// a 5 s escalation.
struct HeldContinue {
    app: Arc<App>,
    host: Arc<support::gated::GatedHost>,
    id: TaskId,
    second: yulius_app::model::AttemptRecord,
    continued: tokio::task::JoinHandle<Result<(), yulius_app::error::AppError>>,
}

async fn continue_on_held_attach(data: &Path, fake: &Path) -> Result<HeldContinue, BoxError> {
    let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
        "CARGO_BIN_EXE_yulius"
    ))));
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };
    let (app, _) = start_on(
        data,
        fake,
        agent,
        0,
        Some(host.clone()),
        Some(5_000),
        false,
        None,
    )
    .await?;
    let (sandbox, user) = seed(&app, data).await?;
    let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
    wait_for("the launch goal", Duration::from_secs(60), || {
        screen_contains(&app, &id, "GOAL SET")
    })
    .await?;
    stop(&app, &id).await?;
    assert!(screen_contains(&app, &id, "GOAL SET").await);
    host.arm();
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(&user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let continued = {
        let (app, id) = (app.clone(), id.clone());
        tokio::spawn(async move {
            app.continue_task(
                &ctx,
                id.as_str(),
                &yulius_contracts::api::tasks::ContinueTask { account_id: None },
            )
            .await
            .map(drop)
        })
    };
    tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
    let second = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
    Ok(HeldContinue {
        app,
        host,
        id,
        second,
        continued,
    })
}

/// Waits until `attempt` is recorded lost.
async fn lost(app: &App, attempt: &yulius_domain::ids::AttemptId) -> TestResult {
    wait_for("the run's loss", Duration::from_secs(30), || async {
        app.deps
            .store
            .attempt(attempt)
            .await
            .is_ok_and(|a| a.is_some_and(|a| a.state == AttemptState::Lost))
    })
    .await
}

/// The run's saved screen, when it has one, is not the run before's (`GOAL SET`).
fn not_the_run_befores_screen(run_dir: &str) -> TestResult {
    let saved = Path::new(run_dir).join("final-screen");
    if let Ok(screen) = std::fs::read(&saved) {
        let screen = String::from_utf8_lossy(&screen);
        assert!(
            !screen.contains("GOAL SET"),
            "the lost run took the run before's screen: {screen:?}"
        );
    }
    Ok(())
}

/// SPEC-301 LRS-001 (review): a continued task's new run is lost before Yulius attached to it
/// (its stop's escalation ends it). The hub still shows the previous run's screen, which is not
/// the new run's: the new run saves no screen.
#[test]
fn a_run_lost_before_it_attached_saves_no_screen_of_the_run_before() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let held = continue_on_held_attach(&data, &fake).await?;
        let app = &held.app;
        app.stop_task_now(&held.id).await?;
        let ended = lost(app, &held.second.id).await;
        held.host.release();
        let _ = tokio::time::timeout(Duration::from_secs(30), held.continued).await?;
        assert!(ended.is_ok(), "{}", run_states(app, &held.id).await);
        assert!(
            !Path::new(&held.second.run_dir)
                .join("final-screen")
                .exists(),
            "the run that never attached saved a screen"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-301 LRS-001 (review round 2): the new run is lost just as Yulius attaches to it, once it
/// counts as the task's attached run. Its hub shows its own screen by then: the run before's is
/// not saved as its own.
#[test]
fn a_run_lost_as_it_attaches_saves_no_screen_of_the_run_before() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let held = continue_on_held_attach(&data, &fake).await?;
        let app = &held.app;
        // At the attach, the stop's escalation loses the run before the attach goes on.
        let (seen, mut hooked) = tokio::sync::watch::channel(None);
        {
            let (app, attempt) = (app.clone(), held.second.id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::RunAttached,
                Path::new(held.id.as_str()),
                move || {
                    // Lost already, the attach would test nothing: the loss must come here.
                    let ended = wait_in_hook(&handle, async {
                        let before = app.deps.store.attempt(&attempt).await?;
                        if before.is_some_and(|a| a.state == AttemptState::Lost) {
                            return Err("the run was lost before it attached".into());
                        }
                        lost(&app, &attempt).await
                    });
                    seen.send_replace(Some(ended.map_err(|e| e.to_string())));
                },
            );
        }
        app.stop_task_now(&held.id).await?;
        held.host.release();
        tokio::time::timeout(Duration::from_secs(60), hooked.wait_for(Option::is_some)).await??;
        let ended = hooked.borrow().clone();
        assert_eq!(ended, Some(Ok(())), "{}", run_states(app, &held.id).await);
        let _ = tokio::time::timeout(Duration::from_secs(30), held.continued).await?;
        not_the_run_befores_screen(&held.second.run_dir)?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-301 (review round 3), TSK-006: a viewer controls the size as a continued run attaches and
/// changes it just after the attach took it. The run ends at the viewer's new size, not the one
/// the attach read.
#[test]
fn a_size_changed_as_the_run_attaches_is_kept() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let held = continue_on_held_attach(&data, &fake).await?;
        let app = &held.app;
        let hub = &app.runtime(&held.id).hub;
        let viewer = hub.new_viewer_id();
        hub.join_writer(viewer, Some((100, 30)));
        let (resized, mut hooked) = tokio::sync::watch::channel(None);
        {
            let (app, id) = (app.clone(), held.id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::RunAttached,
                Path::new(held.id.as_str()),
                move || {
                    app.runtime(&id).hub.report_size(viewer, (90, 25));
                    let done = wait_in_hook(&handle, app.resize_run(&id, 90, 25));
                    resized.send_replace(Some(done.map_err(|e| e.to_string())));
                },
            );
        }
        held.host.release();
        tokio::time::timeout(Duration::from_secs(60), hooked.wait_for(Option::is_some)).await??;
        let done = hooked.borrow().clone();
        assert_eq!(done, Some(Ok(())));
        tokio::time::timeout(Duration::from_secs(30), held.continued).await???;
        assert_eq!(app.runtime(&held.id).hub.size(), (90, 25));
        // The PTY follows too: the agent reports the size it reads.
        app.send_input(&held.id, b"/size\r").await?;
        wait_for("the PTY's size", Duration::from_secs(20), || {
            screen_contains(app, &held.id, "SIZE 25 90")
        })
        .await?;
        hub.leave(viewer);
        stop(app, &held.id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-006 (review): the old run is lost while the switch waits for its end. The switch
/// gives up and says the session ended: the task rests lost with its one run, and nothing
/// starts that no one asked for.
#[test]
fn a_switch_whose_old_run_is_lost_starts_no_new_run() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, host, id) = gated_goal_task(&data, &fake).await?;
        host.refuse_ends(true);
        let result = switch_on_worker(&app, &id);
        tokio::time::timeout(Duration::from_secs(30), host.shutdown_refused()).await?;
        host.refuse_ends(false);
        kill_run_host(&app, &id).await?;
        let switched = switch_result(result).await?;
        assert!(
            switched
                .as_ref()
                .is_err_and(|e| e.contains("the session ended")),
            "{switched:?}"
        );
        let rests = wait_for("the loss", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await;
        assert!(rests.is_ok(), "{}", run_states(&app, &id).await);
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(StopReason::Lost)
        );
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-005 (review): a stop lands while a lost run's loss is recorded, between its read
/// of the task and its status change. The loss is recorded again from `Stopping`: the task rests,
/// not stopping with no run.
#[test]
fn a_stop_while_a_runs_loss_is_recorded_still_ends_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let (stopped, mut stop_made) = tokio::sync::watch::channel(None);
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::EndDecided,
                Path::new(key.as_str()),
                move || {
                    let result = wait_in_hook(&handle, app.stop_task_now(&id));
                    stopped.send_replace(Some(result.map_err(|e| e.to_string())));
                },
            );
        }
        kill_run_host(&app, &id).await?;
        tokio::time::timeout(Duration::from_secs(30), stop_made.wait_for(Option::is_some))
            .await??;
        let made = stop_made.borrow().clone();
        assert_eq!(made, Some(Ok(())));
        let rests = wait_for("the loss", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await;
        assert!(rests.is_ok(), "{}", run_states(&app, &id).await);
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(StopReason::Lost)
        );
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Rests `id` as the startup sweep of #439 did, whatever run it has.
async fn rest_as_server_lost(app: &App, id: &TaskId) -> TestResult {
    let from = task(app, id).await?.status;
    let lost = TaskStatus::Stopped(StopReason::ServerLost);
    assert!(
        app.deps
            .store
            .transition_task(id, Some(from), lost, None, app.now())
            .await?
    );
    Ok(())
}

/// Resumes `id` as Continue and Restart do.
async fn resume(app: &Arc<App>, id: &TaskId) -> AppResult<()> {
    app.start_run(id, RunStart::Resume { prompt: None }, None)
        .await
}

/// SPEC-458 RUN-002, RUN-003 (#439): a task that rests while its run lives under this server. A
/// start is refused and changes nothing (it used to leave the task `failed_precondition`, which
/// takes no message, with the run going on); a stop ends that run, and then the task continues.
#[test]
fn a_refused_start_leaves_a_resting_task_with_a_live_run_as_it_was() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        rest_as_server_lost(&app, &id).await?;
        let refused = resume(&app, &id).await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("live run")),
            "{refused:?}"
        );
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(StopReason::ServerLost)
        );
        assert!(app.deps.store.live_attempt(&id).await?.is_some());
        app.stop_task_now(&id).await?;
        let ended = wait_for("the run's end", Duration::from_secs(60), || async {
            matches!(app.deps.store.live_attempt(&id).await, Ok(None))
        })
        .await;
        assert!(ended.is_ok(), "{}", run_states(&app, &id).await);
        assert!(task(&app, &id).await?.status.is_resting());
        resume(&app, &id).await?;
        wait_for("the new run", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 2);
        app.stop_task_now(&id).await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A resting task whose run an earlier server started and this one never attached to, its host
/// killed: the server on `data`, the task and the run.
fn left_by_a_dead_server(data: &Path, fake: &Path) -> Result<(TaskId, i32), BoxError> {
    let first = runtime()?;
    let (id, host_pid) = first.block_on(async {
        let (app, _host, id) = gated_goal_task(data, fake).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        let pid = i32::try_from(run.ptyhost_pid.ok_or("no host pid")?)?;
        rest_as_server_lost(&app, &id).await?;
        Ok::<_, BoxError>((id, pid))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    Ok((id, host_pid))
}

/// A second server on `data` that reconciled nothing.
async fn second_server(data: &Path, fake: &Path) -> Result<Arc<App>, BoxError> {
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };
    let (app, _) = start_on(data, fake, agent, 0, None, None, false, None).await?;
    Ok(app)
}

/// SPEC-458 RUN-002 (ADR-0050): a start finds a live run on record that this server holds no
/// connection to, its host killed from outside. It ends what is left of it, records it lost and
/// starts the new run, instead of answering "task already has a live run" for ever.
#[test]
fn a_start_settles_a_run_no_server_holds_before_it_starts_the_next() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let (id, host_pid) = left_by_a_dead_server(&data, &fake)?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(host_pid),
        nix::sys::signal::Signal::SIGKILL,
    )?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = second_server(&data, &fake).await?;
        resume(&app, &id).await?;
        wait_for("the new run", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        let runs = app.deps.store.attempts(&id).await?;
        let states: Vec<AttemptState> = runs.iter().map(|a| a.state).collect();
        assert_eq!(runs.len(), 2, "{states:?}");
        assert!(states.contains(&AttemptState::Lost), "{states:?}");
        app.stop_task_now(&id).await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-458 RUN-003: Stop on such a task ends the run (its host still runs here) and records it
/// lost, instead of answering "task is stopped_server_lost" and changing nothing.
#[test]
fn a_stop_settles_a_run_no_server_holds() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let (id, host_pid) = left_by_a_dead_server(&data, &fake)?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = second_server(&data, &fake).await?;
        // The branch under test: a host that still runs is ended first, then recorded lost.
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(host_pid), None).is_ok(),
            "the run's host must still run before the stop"
        );
        app.stop_task_now(&id).await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        let runs = app.deps.store.attempts(&id).await?;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, AttemptState::Lost);
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Stopped(StopReason::ServerLost)
        );
        // Proven gone before it was recorded (AGENTS.md: a timeout is no proof).
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(host_pid), None).is_err(),
            "the run's host still runs"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-458 RUN-001 (#439): the start-up reconciliation runs while a task launches (the queue
/// starts one as soon as a lost run frees its slot), between `Starting` and its run's record. The
/// task is not taken for one a dead server left starting: it used to rest `stopped_server_lost`
/// with its run going on.
#[test]
fn the_startup_reconciliation_leaves_a_launching_task_alone() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        stop_on_worker(&app, &id).await?;
        expect_stopped(&app, &id).await?;
        let (reached, at) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::AccountChosen,
            Path::new(id.as_str()),
            move || {
                let _ = reached.send(());
                let _ = wait.recv_timeout(Duration::from_secs(60));
            },
        );
        let start = {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move { resume(&app, &id).await })
        };
        tokio::task::spawn_blocking(move || at.recv_timeout(Duration::from_secs(60)))
            .await?
            .map_err(|_| "the launch never chose its subscription")?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Starting);
        app.reconcile().await?;
        let during = task(&app, &id).await?.status;
        let _ = go.send(());
        start.await??;
        assert_eq!(during, TaskStatus::Starting);
        wait_for("the run", Duration::from_secs(60), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Running)
        })
        .await?;
        app.stop_task_now(&id).await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Review of SPEC-458 (low): between a launch's record of its run and the attach, the run is on
/// record but not attached, watched or ending. A task can rest under its own launch (two stops
/// before the run is recorded), and a start then must not take that run for one no server
/// holds: it would record it lost while the launch goes on and attaches a running agent.
#[test]
fn a_start_during_a_launch_leaves_the_run_being_launched_alone() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        stop_on_worker(&app, &id).await?;
        expect_stopped(&app, &id).await?;
        let (reached, at) = std::sync::mpsc::channel();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::RunRecorded,
            Path::new(id.as_str()),
            move || {
                let _ = reached.send(());
                let _ = wait.recv_timeout(Duration::from_secs(60));
            },
        );
        let start = {
            let (app, id) = (app.clone(), id.clone());
            tokio::spawn(async move { resume(&app, &id).await })
        };
        tokio::task::spawn_blocking(move || at.recv_timeout(Duration::from_secs(60)))
            .await?
            .map_err(|_| "the launch never recorded its run")?;
        let run = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no run on record")?;
        rest_as_server_lost(&app, &id).await?;
        let second = tokio::time::timeout(Duration::from_secs(20), resume(&app, &id)).await;
        let state = app.deps.store.attempt(&run.id).await?.map(|a| a.state);
        let _ = go.send(());
        let _ = start.await?;
        assert!(
            matches!(&second, Ok(Err(e)) if e.to_string().contains("live run")),
            "{second:?}"
        );
        assert!(
            state.is_some_and(|state| !state.is_terminal()),
            "the run being launched was settled: {state:?}"
        );
        let _ = app.stop_task_now(&id).await;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Review of SPEC-458 (RUN-001): the start-up sweep rests only what the server before this one
/// left live. A task that was resting when the reconciliation began and that this server moved
/// on meanwhile is this server's, also in the moment it has no run on record.
#[test]
fn the_startup_sweep_rests_only_what_the_earlier_server_left() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        stop_on_worker(&app, &id).await?;
        expect_stopped(&app, &id).await?;
        let resting = task(&app, &id).await?.status;
        {
            let (app, id) = (app.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(yulius_app::race::Point::ReconcileSweep, &data, move || {
                // This server moves the task on while it reconciles: live, no run yet.
                let _ = wait_in_hook(
                    &handle,
                    app.deps.store.transition_task(
                        &id,
                        Some(resting),
                        TaskStatus::Stopping,
                        None,
                        app.now(),
                    ),
                );
            });
        }
        app.reconcile().await?;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Stopping);
        assert!(
            app.deps
                .store
                .transition_task(&id, Some(TaskStatus::Stopping), resting, None, app.now())
                .await?
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Review of SPEC-458 (RUN-001): a run this server attached to again is its own. When that run
/// ends while the reconciliation still goes on, the task is for a moment live without a run on
/// record; the sweep must leave it to the run's own end, not rest it as a dead server's.
#[test]
fn the_startup_sweep_leaves_a_task_whose_run_this_server_holds_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (id, host_pid) = first.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        let pid = i32::try_from(run.ptyhost_pid.ok_or("no host pid")?)?;
        Ok::<_, BoxError>((id, pid))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    rt.block_on(async {
        let app = second_server(&data, &fake).await?;
        let run = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        {
            let (app, run_id) = (app.clone(), run.id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(yulius_app::race::Point::ReconcileSweep, &data, move || {
                // The run's end reaches the record before the task's status follows.
                let update = yulius_app::ports::store::AttemptUpdate {
                    ended_at: Some(app.now()),
                    exit_reason: Some("exited".into()),
                    ..yulius_app::ports::store::AttemptUpdate::default()
                };
                let _ = wait_in_hook(
                    &handle,
                    app.deps
                        .store
                        .transition_attempt(&run_id, AttemptState::Exited, &update),
                );
            });
        }
        app.reconcile().await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        let status = task(&app, &id).await?.status;
        assert_ne!(
            status,
            TaskStatus::Stopped(StopReason::ServerLost),
            "rested as a dead server's although this server holds its run"
        );
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(host_pid),
            nix::sys::signal::Signal::SIGKILL,
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-250 STW-005: a stop lands while the end of the task's run is recorded, between the end's
/// read of the task and its status change (the agent exited at that moment). The end's change is
/// made again from the stop's status: the task ends stopped, not stopping with no run.
#[test]
fn a_stop_while_a_runs_end_is_recorded_still_ends_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _host, id) = gated_goal_task(&data, &fake).await?;
        let (stopped, mut stop_made) = tokio::sync::watch::channel(None);
        {
            let (app, key, id) = (app.clone(), id.clone(), id.clone());
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                yulius_app::race::Point::EndDecided,
                Path::new(key.as_str()),
                move || {
                    let result = wait_in_hook(&handle, app.stop_task_now(&id));
                    stopped.send_replace(Some(result.map_err(|e| e.to_string())));
                },
            );
        }
        app.send_input(&id, b"/exit\r").await?;
        tokio::time::timeout(Duration::from_secs(30), stop_made.wait_for(Option::is_some))
            .await??;
        let made = stop_made.borrow().clone();
        assert_eq!(made, Some(Ok(())));
        expect_stopped(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-079 BGW-005 (#264): Yulius's Stop hook runs before the goal evaluator. While the goal is
/// open the turn waits for the verdict: the verdict "not met" sends the agent on, and the task
/// never reads Your turn or Done in between; the verdict "met" makes it Done.
#[test]
fn a_goal_not_met_never_reads_idle_between_turns() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_delay_ms: Some(3_000),
            goal_continue_ms: Some(4_000),
            goal_continue_verdict: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the goal", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        // The activities the UI is told, collected as they come: the store reads Done a moment
        // before its update is published, so one drain at Done can miss it.
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut events = app.events.subscribe();
        let collector = tokio::spawn({
            let (seen, id) = (Arc::clone(&seen), id.as_str().to_owned());
            async move {
                loop {
                    match events.recv().await {
                        Ok(envelope) => {
                            if let UiEvent::TaskUpdated { task } = envelope.event
                                && task.id == id
                            {
                                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                                if seen.last() != task.activity.as_ref() {
                                    seen.push(task.activity.unwrap_or_default());
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        let seen_now = || seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        wait_for("the verdict", Duration::from_secs(30), || {
            screen_contains(&app, &id, "GOAL NOT MET")
        })
        .await?;
        wait_for("Done", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        wait_for("the Done update", Duration::from_secs(10), || async {
            seen_now().last().map(String::as_str) == Some("done")
        })
        .await?;
        collector.abort();
        let seen = seen_now();
        let idle = seen
            .iter()
            .position(|a| a == "waiting" || a == "done")
            .ok_or_else(|| format!("never idle: {seen:?}"))?;
        assert_eq!(
            &seen[idle..],
            ["done".to_owned()],
            "the only idle state is the final Done: {seen:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-079 BGW-005 (#264): the goal turn's Stop waits for the verdict. "failed" makes the task
/// wait for its person before the turn's record (6 s later) comes; no verdict at all settles it at
/// that record as before, well before the 20 s bound. Neither reads Done.
fn an_unmet_goal_leaves_the_task_waiting(verdict: &'static str) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_verdict: Some(verdict),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Ship it").await?;
        let settled_by = if verdict == "failed" {
            "GOAL FAILED"
        } else {
            "GOAL UNMET"
        };
        wait_for("the verdict or the record", Duration::from_secs(90), || {
            screen_contains(&app, &id, settled_by)
        })
        .await?;
        wait_for("Your turn", Duration::from_secs(4), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        if verdict == "failed" {
            assert!(
                !screen_contains(&app, &id, "GOAL UNMET").await,
                "the verdict settled it, not the record"
            );
        }
        wait_for("the turn's record", Duration::from_secs(20), || {
            screen_contains(&app, &id, "GOAL UNMET")
        })
        .await?;
        assert_eq!(
            activity(&app, &id).await,
            Some(Activity::Waiting),
            "never Done"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_failed_goal_leaves_the_task_waiting() -> TestResult {
    an_unmet_goal_leaves_the_task_waiting("failed")
}

#[test]
fn a_turn_closed_without_a_verdict_settles_as_before() -> TestResult {
    an_unmet_goal_leaves_the_task_waiting("none")
}

/// STA-004, STA-005 (SPEC-032, task #24 between 15:55:22 and 15:55:30): Yulius's Stop hook ends
/// the goal turn (the task is idle), then Claude Code sends the agent on: the evaluator's "not
/// met" verdict, or another Stop hook that continues the turn. The task is Working while the
/// agent goes on, and Done once its goal is achieved. One test per way, each within the
/// integration suite's time limit.
fn a_session_goes_on_from_its_stop(verdict: bool, marker: &str) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_continue_ms: Some(10_000),
            goal_continue_verdict: verdict,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for(marker, Duration::from_secs(90), || {
            screen_contains(&app, &id, marker)
        })
        .await?;
        // Well before the agent's next record (the verdict case: 10 s), the task works.
        wait_for(
            "Working while the session goes on",
            Duration::from_secs(7),
            || async { activity(&app, &id).await == Some(Activity::Working) },
        )
        .await?;
        wait_for("Done", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_goal_not_met_does_not_leave_the_task_waiting() -> TestResult {
    a_session_goes_on_from_its_stop(true, "GOAL NOT MET")
}

#[test]
fn a_stop_a_hook_continued_does_not_leave_the_task_waiting() -> TestResult {
    a_session_goes_on_from_its_stop(false, "STOP CONTINUED")
}

/// STA-004 (review of SPEC-032): the goal verdict "not met" sends the agent on, then an API error
/// ends that turn and Claude Code runs no Stop hook. The task does not stay Working for good: the
/// record that closes the turn makes it wait for the person again.
#[test]
fn a_continued_turn_that_dies_without_a_stop_leaves_the_task_waiting() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_continue_ms: Some(6_000),
            goal_continue_verdict: true,
            goal_continue_dies: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the verdict", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL NOT MET")
        })
        .await?;
        wait_for("Working", Duration::from_secs(4), || async {
            activity(&app, &id).await == Some(Activity::Working)
        })
        .await?;
        wait_for("the turn's death", Duration::from_secs(30), || {
            screen_contains(&app, &id, "CONTINUATION DIED")
        })
        .await?;
        wait_for("Your turn again", Duration::from_secs(10), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The task's activities, each change once, until it reads `target` (bounded by `timeout`).
async fn activities_until(
    app: &App,
    id: &TaskId,
    target: Activity,
    timeout: Duration,
) -> Result<Vec<Option<Activity>>, BoxError> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        let now = activity(app, id).await;
        if seen.last() != Some(&now) {
            seen.push(now);
        }
        if now == Some(target) {
            return Ok(seen);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("timed out waiting for {target:?}; seen {seen:?}").into())
}

/// SPEC-333 AER-001 (task #33's "Connection lost mid-response"): an API error ends the goal turn
/// and Claude Code retries it by itself. The task reads Working from the error to the retry that
/// achieves the goal, never "Your turn": nobody has to type `continue`.
#[test]
fn a_goal_turn_claude_code_retries_after_an_api_error_stays_working() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_api_errors: Some(2),
            goal_retry_ms: Some(3_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the API error", Duration::from_secs(90), || {
            screen_contains(&app, &id, "API ERROR")
        })
        .await?;
        let seen = activities_until(&app, &id, Activity::Done, Duration::from_secs(60)).await?;
        assert!(
            screen_contains(&app, &id, "RETRY DONE").await,
            "the goal was achieved by a retry"
        );
        assert_eq!(
            seen,
            vec![Some(Activity::Working), Some(Activity::Done)],
            "Working from the error to the retry's Done"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-333 AER-002: the goal turn and Claude Code's three retries all fail, and Claude Code gives
/// up ("Goal paused after 3 automatic retries · <cause>"). The task is in Needs you, with a request
/// that says what happened and to send a message, and stays there once the record that closes the
/// turn is read; it read Working while the retries ran. With `check_errors` each turn ended cleanly
/// and its goal check failed (its Stop hook ran, so a verdict was pending).
fn spent_goal_retries_put_the_task_in_needs_you(check_errors: bool, cause: &str) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_api_errors: Some(4),
            goal_retry_ms: Some(1_500),
            goal_check_errors: check_errors,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the first failure", Duration::from_secs(90), || {
            screen_contains(&app, &id, "API ERROR")
        })
        .await?;
        let seen =
            activities_until(&app, &id, Activity::NeedsAttention, Duration::from_secs(60)).await?;
        assert!(screen_contains(&app, &id, "GOAL PAUSED").await);
        assert_eq!(
            seen,
            vec![Some(Activity::Working), Some(Activity::NeedsAttention)],
            "Working while Claude Code retried, then Needs you"
        );
        // The record that closes the turn was written with the notice: two more rounds read it.
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for(
            "the turn's record read",
            Duration::from_secs(20),
            || async { runtime.transcript_rounds() >= rounds + 2 },
        )
        .await?;
        assert_eq!(
            activity(&app, &id).await,
            Some(Activity::NeedsAttention),
            "the turn's record leaves Needs you"
        );
        let summary = app.task_summary(&task(&app, &id).await?).await?;
        let request = summary.needs_you.ok_or("no Needs you request")?;
        assert!(
            request
                .happened
                .as_deref()
                .is_some_and(|h| h.contains(cause)),
            "{request:?}"
        );
        assert!(request.action.contains("message"), "{request:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn spent_goal_retries_after_api_errors_put_the_task_in_needs_you() -> TestResult {
    spent_goal_retries_put_the_task_in_needs_you(
        false,
        "the API was unavailable or the connection dropped",
    )
}

#[test]
fn spent_goal_check_retries_put_the_task_in_needs_you() -> TestResult {
    spent_goal_retries_put_the_task_in_needs_you(true, "the goal check could not complete")
}

/// The person clears the goal (`/goal clear`) while Claude Code's retry is armed: no check-in
/// comes, and the task waits for them instead of reading Working for good.
#[test]
fn a_goal_cleared_while_a_retry_is_armed_leaves_the_task_waiting() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            goal_api_errors: Some(2),
            goal_cleared_on_retry: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the goal cleared", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL CLEARED")
        })
        .await?;
        let seen = activities_until(&app, &id, Activity::Waiting, Duration::from_secs(20)).await?;
        assert_eq!(seen.last(), Some(&Some(Activity::Waiting)));
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-333 AER-001/AER-002 after a restart: Yulius is down from the goal turn's API error until
/// the transcript holds `marker` (and the record that closes that turn). The new server replays
/// the transcript and checks the task with `check`.
fn a_restart_after_an_api_error(
    api_errors: u32,
    retry_ms: u64,
    marker: &'static str,
    check: impl AsyncFnOnce(&Arc<App>, &TaskId) -> TestResult,
) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        goal_api_errors: Some(api_errors),
        goal_retry_ms: Some(retry_ms),
        ..FakeAgentOptions::default()
    };
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let (app, port) = start(&data, &fake, agent.clone(), 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("the API error", Duration::from_secs(90), || {
            screen_contains(&app, &id, "API ERROR")
        })
        .await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        let record = TaskStore::task(&store, &id).await?.ok_or("task")?;
        let worktree = PathBuf::from(record.worktree_path.ok_or("no worktree")?);
        let attempt = TaskStore::live_attempt(&store, &id)
            .await?
            .ok_or("attempt")?;
        let session = attempt.provider_session_id.ok_or("session")?;
        let adapter = HarnessAdapter::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius")), "x");
        let home = data.join("sandboxes").join("s-done").join("home");
        let transcript = adapter.claude_transcript(&home, &worktree, &session);
        wait_for(marker, Duration::from_secs(60), || async {
            std::fs::read_to_string(&transcript).is_ok_and(|t| {
                t.rsplit_once(marker)
                    .is_some_and(|(_, after)| after.contains("turn_duration"))
            })
        })
        .await?;
        drop(store);
        let (app, _) = start(&data, &fake, agent, port).await?;
        app.reconcile().await?;
        check(&app, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A retry Claude Code armed while Yulius was down: the replay leaves the task Working, and the
/// retry makes it Done.
#[test]
fn a_restart_while_claude_code_retries_keeps_the_task_working() -> TestResult {
    a_restart_after_an_api_error(1, 15_000, "retrying in", async |app, id| {
        let seen = activities_until(app, id, Activity::Done, Duration::from_secs(60)).await?;
        assert_eq!(seen, vec![Some(Activity::Working), Some(Activity::Done)]);
        Ok(())
    })
}

/// Claude Code spent its retries while Yulius was down: the replay puts the task in Needs you.
#[test]
fn a_restart_after_spent_goal_retries_puts_the_task_in_needs_you() -> TestResult {
    a_restart_after_an_api_error(4, 1_000, "automatic retries", async |app, id| {
        wait_for("Needs you", Duration::from_secs(20), || async {
            activity(app, id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let summary = app.task_summary(&task(app, id).await?).await?;
        let request = summary.needs_you.ok_or("no Needs you request")?;
        assert!(
            request.happened.as_deref().is_some_and(|h| {
                h.contains("Goal paused after 3 automatic retries")
                    && h.contains("the API was unavailable or the connection dropped")
            }),
            "the replay keeps Claude Code's words: {request:?}"
        );
        Ok(())
    })
}

/// The fake agent of a goal task whose retries Claude Code spends (SPEC-469): four API errors,
/// then its pause.
fn pausing_agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        hooks: true,
        goal_api_errors: Some(4),
        goal_retry_ms: Some(1_000),
        ..FakeAgentOptions::default()
    }
}

/// The request Yulius opened for the goal's pause ("Send the task a message…"), while it is open.
async fn goal_pause_request(
    app: &App,
    id: &TaskId,
) -> Result<Option<yulius_contracts::api::tasks::NeedsYouView>, BoxError> {
    let summary = app.task_summary(&task(app, id).await?).await?;
    Ok(summary
        .needs_you
        .filter(|request| request.action.contains("Send the task a message")))
}

/// The newest audit entry of a request of the task that ended without a person or its agent
/// (`task.needs_you.resolved`).
async fn resolved_audit(
    app: &App,
    id: &TaskId,
) -> AppResult<Option<yulius_app::automations::AuditEntry>> {
    app.deps
        .store
        .last_audit(id.as_str(), "task.needs_you.resolved", Timestamp(0))
        .await
}

/// Whether the request for the goal's pause is still open (an error reads as open).
async fn pause_asks(app: &App, id: &TaskId) -> bool {
    !matches!(goal_pause_request(app, id).await, Ok(None))
}

/// A goal task Claude Code stopped retrying: it reads Needs you, with the request for the pause.
async fn paused_goal_task(app: &Arc<App>, data: &Path) -> Result<TaskId, BoxError> {
    let (sandbox, user) = seed(app, data).await?;
    let id = goal_task(app, &sandbox, &user, "Merge on green").await?;
    wait_for("the goal's pause", Duration::from_secs(120), || async {
        activity(app, &id).await == Some(Activity::NeedsAttention)
    })
    .await?;
    let request = goal_pause_request(app, &id).await?.ok_or("no request")?;
    assert!(
        request
            .happened
            .as_deref()
            .is_some_and(|h| h.contains("Goal paused after 3 automatic retries")),
        "{request:?}"
    );
    Ok(id)
}

/// SPEC-469 GPW-001 (task #154 of another project, 2026-11-11: "even if it woke up, it still has
/// needs you status"): Claude Code paused the goal, so the task asks a person to send a message.
/// Then Claude Code wakes the session itself (background work ended) and the agent works. The
/// request ends with the agent's first line, the task reads Working while the turn runs, and it
/// asks nobody once the turn ended. Before, the request stayed open for good.
#[test]
fn a_session_claude_code_wakes_after_its_goal_pause_no_longer_needs_you() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let agent = FakeAgentOptions {
            paused_wake_file: Some(wake.clone()),
            turn_hold_file: Some(hold.clone()),
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        std::fs::write(&wake, "")?;
        wait_for("the woken turn's work", Duration::from_secs(60), || {
            screen_contains(&app, &id, "WOKEN")
        })
        .await?;
        wait_for(
            "the request to end with the agent's work",
            Duration::from_secs(20),
            || async { !pause_asks(&app, &id).await },
        )
        .await?;
        assert_eq!(
            activity(&app, &id).await,
            Some(Activity::Working),
            "the woken turn runs"
        );
        // The end is audited for `system`; the row is written after the change is published.
        wait_for(
            "the audit of the request's end",
            Duration::from_secs(20),
            || async {
                matches!(
                    resolved_audit(&app, &id).await,
                    Ok(Some(entry)) if entry.actor == "system"
                        && entry.detail.as_deref().is_some_and(|d| d.contains("works again"))
                )
            },
        )
        .await?;
        // The turn ends (no goal verdict): the task waits, and still asks nobody.
        std::fs::write(&hold, "")?;
        wait_for("the woken turn's end", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        assert!(!pause_asks(&app, &id).await, "the request stays ended");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-001: the paused session goes on with no prompt anyone sees (a hidden
/// continuation: no prompt hook, no prompt record that starts a turn). The agent's work alone
/// says so: the request ends, and the task that read Needs you from the pause reads Working, as
/// it does after a visible wake-up.
#[test]
fn a_session_that_goes_on_unprompted_after_its_goal_pause_no_longer_needs_you() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let agent = FakeAgentOptions {
            paused_wake_file: Some(wake.clone()),
            paused_wake_hidden: true,
            turn_hold_file: Some(hold.clone()),
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        std::fs::write(&wake, "")?;
        wait_for("the session's work", Duration::from_secs(60), || {
            screen_contains(&app, &id, "WOKEN")
        })
        .await?;
        wait_for(
            "the request to end with the agent's work",
            Duration::from_secs(20),
            || async { !pause_asks(&app, &id).await },
        )
        .await?;
        wait_for("Working", Duration::from_secs(10), || async {
            activity(&app, &id).await == Some(Activity::Working)
        })
        .await?;
        std::fs::write(&hold, "")?;
        wait_for("the turn's end", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        assert!(!pause_asks(&app, &id).await, "the request stays ended");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-001 (tasks #446, #450 and #452 on 2026-11-11: paused "the request was rate
/// limited", then resumed by Yulius 12 to 28 minutes later and working with the request still
/// open): Yulius restarts the paused session and the API answers again in the resumed one. The
/// restart alone ends nothing (the resumed session has not answered yet); its first answer does.
#[test]
fn a_session_yulius_restarts_after_its_goal_pause_no_longer_needs_you() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let hold = data.join("hold-turn");
        let agent = FakeAgentOptions {
            api_recovers_on_resume: true,
            no_response_line: true,
            turn_hold_file: Some(hold.clone()),
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        app.switch_task_now(&id, None, "manual", None).await?;
        // The resumed session took its goal; its turn is held before its first line. Its own
        // screen: until the run attaches, the task still shows the paused run's.
        wait_for(
            "the resumed session's goal",
            Duration::from_secs(90),
            || async {
                let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                attempts.len() == 2
                    && attempts[1].state == AttemptState::Live
                    && screen_contains(&app, &id, "GOAL SET").await
                    && !screen_contains(&app, &id, "GOAL PAUSED").await
            },
        )
        .await?;
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for(
            "the resumed session read",
            Duration::from_secs(20),
            || async { runtime.transcript_rounds() >= rounds + 2 },
        )
        .await?;
        // Claude Code's own "No response requested." of the resumed session is no answer either.
        assert!(
            pause_asks(&app, &id).await,
            "a session that has not answered yet shows nothing"
        );
        std::fs::write(&hold, "")?;
        wait_for(
            "the request to end with the resumed session's answer",
            Duration::from_secs(30),
            || async { !pause_asks(&app, &id).await },
        )
        .await?;
        wait_for("Done", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        assert!(!pause_asks(&app, &id).await, "the request stays ended");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-001 and SPEC-333 AER-002 together: the session Yulius restarted works (a tool
/// call), which ends the request, and then the API fails again until Claude Code pauses the goal
/// once more. The task is in Needs you again, with a request asked anew: the one it had ended
/// with the work, so its wait does not count from the first pause.
#[test]
fn a_session_paused_again_after_it_worked_asks_again() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            no_response_line: true,
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        let first = goal_pause_request(&app, &id).await?.ok_or("no request")?;
        app.switch_task_now(&id, None, "manual", None).await?;
        // The resumed run's own screen first: until it attaches, the task still shows the
        // paused run's, with its pause.
        wait_for("the resumed session", Duration::from_secs(90), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2
                && attempts[1].state == AttemptState::Live
                && screen_contains(&app, &id, "FAKE-AGENT READY").await
                && !screen_contains(&app, &id, "GOAL PAUSED").await
        })
        .await?;
        wait_for(
            "the resumed session's pause",
            Duration::from_secs(120),
            || screen_contains(&app, &id, "GOAL PAUSED"),
        )
        .await?;
        wait_for("Needs you again", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let again = goal_pause_request(&app, &id).await?.ok_or("no request")?;
        assert!(
            again.at > first.at,
            "asked anew after the work: {again:?} after {first:?}"
        );
        let ended = app
            .deps
            .store
            .audit_details(id.as_str(), "task.needs_you.resolved", 5)
            .await?;
        assert_eq!(ended.len(), 1, "the first request ended once: {ended:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-002 (task #438's session, 2026-11-11 15:05: a goal check-in woke the paused
/// session, the API refused again within a second and Claude Code closed the turn without a new
/// notice): a woken turn that brings no work leaves the request open. The person still has to
/// act, and still reads why.
#[test]
fn a_woken_turn_the_api_refuses_again_still_needs_you() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let wake = data.join("wake");
        let agent = FakeAgentOptions {
            paused_wake_file: Some(wake.clone()),
            paused_wake_dies: true,
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        std::fs::write(&wake, "")?;
        wait_for("the refused turn", Duration::from_secs(60), || {
            screen_contains(&app, &id, "WAKE DIED")
        })
        .await?;
        // The record that closes the refused turn settles it (no Stop hook ran): Your turn.
        wait_for(
            "the refused turn's end",
            Duration::from_secs(30),
            || async { activity(&app, &id).await == Some(Activity::Waiting) },
        )
        .await?;
        let request = goal_pause_request(&app, &id)
            .await?
            .ok_or("the request ended although the session did not work")?;
        assert!(
            request
                .happened
                .as_deref()
                .is_some_and(|h| h.contains("Goal paused after 3 automatic retries")),
            "{request:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-006: the store refuses the write that ends the request (a trigger aborts it)
/// when the agent's first work is read. Nothing is lost: the request stays, and the agent's
/// next work ends it once the store takes the write.
#[test]
fn a_request_the_store_could_not_end_ends_at_the_agents_next_work() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let agent = FakeAgentOptions {
            paused_wake_file: Some(wake.clone()),
            turn_hold_file: Some(hold.clone()),
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute_batch(
            "CREATE TRIGGER refuse_the_end BEFORE UPDATE OF needs_you ON tasks \
             WHEN NEW.needs_you IS NULL \
             BEGIN SELECT RAISE(ABORT, 'refused for the test'); END",
        )?;
        std::fs::write(&wake, "")?;
        wait_for("the woken turn's work", Duration::from_secs(60), || {
            screen_contains(&app, &id, "WOKEN")
        })
        .await?;
        // The work line was written before the marker: two more rounds read it.
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for("the work read", Duration::from_secs(20), || async {
            runtime.transcript_rounds() >= rounds + 2
        })
        .await?;
        assert!(
            pause_asks(&app, &id).await,
            "the store refused the write: the request stays"
        );
        db.execute_batch("DROP TRIGGER refuse_the_end")?;
        // The answer is the agent's next work.
        std::fs::write(&hold, "")?;
        wait_for(
            "the request to end at the agent's next work",
            Duration::from_secs(30),
            || async { !pause_asks(&app, &id).await },
        )
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-003: the agent asked a person for something before Claude Code paused the goal
/// (Yulius opens no second request). The session's work after the pause is no answer to the
/// agent's question: its request stays, as SPEC-134 says.
#[test]
fn the_agents_own_request_stays_when_the_session_works_after_a_goal_pause() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let agent = FakeAgentOptions {
            paused_wake_file: Some(wake.clone()),
            turn_hold_file: Some(hold.clone()),
            ..pausing_agent()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        let asked = yulius_app::needs_you::NeedsYouRecord {
            action: "Add DEV_SSH_KEY on the Secrets page".into(),
            happened: None,
            problem: Some("The deploy needs it.".into()),
            decision: false,
            link: None,
            at: 1,
        };
        app.deps
            .store
            .set_task_needs_you(&id, Some(&asked), Timestamp(1))
            .await?;
        wait_for("the goal's pause", Duration::from_secs(120), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        std::fs::write(&wake, "")?;
        wait_for("the woken turn's work", Duration::from_secs(60), || {
            screen_contains(&app, &id, "WOKEN")
        })
        .await?;
        let runtime = app.runtime(&id);
        let rounds = runtime.transcript_rounds();
        wait_for("the work read", Duration::from_secs(20), || async {
            runtime.transcript_rounds() >= rounds + 2
        })
        .await?;
        let summary = app.task_summary(&task(&app, &id).await?).await?;
        assert_eq!(
            summary.needs_you.map(|request| request.action),
            Some(asked.action.clone()),
            "the agent's request is its own to end"
        );
        assert_eq!(resolved_audit(&app, &id).await?, None, "nothing ended");
        std::fs::write(&hold, "")?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Two more rounds of the transcript watcher: every line written before is read and applied.
async fn transcript_read(app: &App, id: &TaskId) -> TestResult {
    let runtime = app.runtime(id);
    let rounds = runtime.transcript_rounds();
    wait_for("the transcript read", Duration::from_secs(20), || async {
        runtime.transcript_rounds() >= rounds + 2
    })
    .await
}

/// A goal task that another matter flagged (Needs you) before Claude Code paused its goal: the
/// pause is held (`hold`) until the flag is in place, and the request for the pause is asked
/// beside that flag.
async fn flagged_then_paused_goal_task(
    app: &Arc<App>,
    data: &Path,
    hold: &Path,
) -> Result<TaskId, BoxError> {
    let (sandbox, user) = seed(app, data).await?;
    let id = goal_task(app, &sandbox, &user, "Merge on green").await?;
    wait_for("the held pause", Duration::from_secs(120), || {
        screen_contains(app, &id, "PAUSE HELD")
    })
    .await?;
    // The last failed turn's prompt and work are read: nothing sets the activity after them.
    transcript_read(app, &id).await?;
    app.deps
        .store
        .set_activity(&id, Some(Activity::NeedsAttention), app.now())
        .await?;
    std::fs::write(hold, "")?;
    wait_for(
        "the request for the pause",
        Duration::from_secs(60),
        || async { matches!(goal_pause_request(app, &id).await, Ok(Some(_))) },
    )
    .await?;
    assert_eq!(activity(app, &id).await, Some(Activity::NeedsAttention));
    Ok(id)
}

/// The session goes on with no visible prompt (`wake`), the agent's work ends the request for
/// the pause, and the task still reads Needs you once that work is applied: the flag it reads is
/// not the pause's to lift.
async fn the_request_ends_and_the_flag_stays(
    app: &Arc<App>,
    id: &TaskId,
    wake: &Path,
) -> TestResult {
    std::fs::write(wake, "")?;
    wait_for("the session's work", Duration::from_secs(60), || {
        screen_contains(app, id, "WOKEN")
    })
    .await?;
    wait_for(
        "the request to end with the agent's work",
        Duration::from_secs(20),
        || async { !pause_asks(app, id).await },
    )
    .await?;
    transcript_read(app, id).await?;
    assert_eq!(
        activity(app, id).await,
        Some(Activity::NeedsAttention),
        "another matter's flag stays"
    );
    Ok(())
}

/// The fake agent of a session that goes on after its goal pause with no visible prompt, its
/// turn held (`hold`) after its first work.
fn unprompted_agent(wake: &Path, hold: &Path) -> FakeAgentOptions {
    FakeAgentOptions {
        paused_wake_file: Some(wake.to_owned()),
        paused_wake_hidden: true,
        turn_hold_file: Some(hold.to_owned()),
        ..pausing_agent()
    }
}

/// SPEC-469 GPW-003 (review round 1): a task that another matter flagged before Claude Code
/// paused the goal keeps that flag. The pause asks its request beside it; the session's work
/// (no visible prompt) ends the request, and the task still reads Needs you.
#[test]
fn a_task_flagged_before_the_goal_pause_keeps_that_flag_through_the_agents_work() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let pause = data.join("hold-pause");
        let agent = FakeAgentOptions {
            pause_hold_file: Some(pause.clone()),
            ..unprompted_agent(&wake, &hold)
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = flagged_then_paused_goal_task(&app, &data, &pause).await?;
        the_request_ends_and_the_flag_stays(&app, &id, &wake).await?;
        std::fs::write(&hold, "")?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-003 and GPW-004 (review round 1): the same across a restart of Yulius. The new
/// server takes the time of the pause's flag from the task's state only when the task reads
/// Needs you since the request was asked; this task was flagged before.
#[test]
fn a_task_flagged_before_the_goal_pause_keeps_that_flag_after_a_restart() -> TestResult {
    let (_keep, files) = data_dir()?;
    let (wake, hold) = (files.join("wake"), files.join("hold-turn"));
    let pause = files.join("hold-pause");
    let agent = FakeAgentOptions {
        pause_hold_file: Some(pause.clone()),
        ..unprompted_agent(&wake, &hold)
    };
    a_restart_after(
        agent,
        async |app, data| flagged_then_paused_goal_task(app, data, &pause).await,
        async |_| Ok(()),
        async |app, id| {
            // The replay is applied (the transcript ends in the pause): as it was.
            let runtime = app.runtime(id);
            wait_for(
                "the transcript read again",
                Duration::from_secs(30),
                || async { runtime.transcript_rounds() >= 2 },
            )
            .await?;
            assert!(pause_asks(app, id).await, "still paused: still asked");
            the_request_ends_and_the_flag_stays(app, id, &wake).await?;
            std::fs::write(&hold, "")?;
            Ok(())
        },
    )
}

/// SPEC-469 (review round 1): the flag the pause raised is replaced without a word (a status
/// change does that, as when a held turn ends), and another matter flags the task afterwards.
/// The time of the pause's flag says nothing about that later one: the session's work ends the
/// request for the pause and leaves the task in Needs you.
#[test]
fn a_flag_raised_after_the_goal_pause_is_not_lifted_by_the_agents_work() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (wake, hold) = (data.join("wake"), data.join("hold-turn"));
        let (app, _) = start(&data, &fake, unprompted_agent(&wake, &hold), 0).await?;
        let id = paused_goal_task(&app, &data).await?;
        // Later than the pause's flag by more than the clock's step.
        transcript_read(&app, &id).await?;
        let store = &app.deps.store;
        store
            .set_activity(&id, Some(Activity::Working), app.now())
            .await?;
        store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        the_request_ends_and_the_flag_stays(&app, &id, &wake).await?;
        std::fs::write(&hold, "")?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A finished goal task whose session then asks the person something (Claude Code's permission
/// prompt, through its `Notification` hook): it reads Needs you, flagged by that notice.
async fn task_flagged_by_a_notice(app: &Arc<App>, data: &Path) -> Result<TaskId, BoxError> {
    task_flagged_by_a_notice_of(app, data, "permission_prompt").await
}

/// [`task_flagged_by_a_notice`], for a notice of `kind` (its `notification_type`).
async fn task_flagged_by_a_notice_of(
    app: &Arc<App>,
    data: &Path,
    kind: &str,
) -> Result<TaskId, BoxError> {
    let (sandbox, user) = seed(app, data).await?;
    let id = goal_task(app, &sandbox, &user, "Merge on green").await?;
    wait_for("Done", Duration::from_secs(90), || async {
        activity(app, &id).await == Some(Activity::Done)
    })
    .await?;
    app.send_input(&id, format!("/notify {kind}\r").as_bytes())
        .await?;
    wait_for("the notice's flag", Duration::from_secs(30), || async {
        activity(app, &id).await == Some(Activity::NeedsAttention)
            && screen_contains(app, &id, "NOTIFIED").await
    })
    .await?;
    // The hook has returned and the notice's time is stored. What the agent does next is later
    // than it by more than the clock's step: work in the notice's own millisecond is not after it.
    transcript_read(app, &id).await?;
    Ok(id)
}

/// STA-003, end to end (SPEC-469, review round 2: no test sent a notice and then agent work):
/// the person answers the permission prompt, which is no prompt of theirs to the agent, and the
/// agent goes on. Its work after the notice makes the task Working.
#[test]
fn a_notice_is_answered_by_the_agents_next_work() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = task_flagged_by_a_notice(&app, &data).await?;
        app.send_input(&id, b"/tool\r").await?;
        wait_for(
            "Working with the agent's work",
            Duration::from_secs(30),
            || async { activity(&app, &id).await == Some(Activity::Working) },
        )
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 (review round 2), the notice's side of the rule the goal pause shares: the notice's
/// flag is replaced without a word (a status change) and another matter flags the task later.
/// The agent's next work does not lift that flag: the notice's time is stale.
#[test]
fn a_flag_raised_after_a_notices_is_not_lifted_by_the_agents_work() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let id = task_flagged_by_a_notice(&app, &data).await?;
        let store = &app.deps.store;
        store
            .set_activity(&id, Some(Activity::Working), app.now())
            .await?;
        store
            .set_activity(&id, Some(Activity::NeedsAttention), app.now())
            .await?;
        app.send_input(&id, b"/tool\r").await?;
        wait_for("the agent's work", Duration::from_secs(30), || {
            screen_contains(&app, &id, "TOOL")
        })
        .await?;
        transcript_read(&app, &id).await?;
        assert_eq!(
            activity(&app, &id).await,
            Some(Activity::NeedsAttention),
            "another matter's flag stays"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The task token of the run whose agent lives under `data`, read from that agent's own
/// environment (`YULIUS_TASK_TOKEN`): what its tools authenticate with.
#[cfg(target_os = "linux")]
fn agent_token(data: &Path) -> Result<String, BoxError> {
    let under = data.to_string_lossy().into_owned();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Ok(env) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        let vars: Vec<String> = env
            .split(|byte| *byte == 0)
            .map(|var| String::from_utf8_lossy(var).into_owned())
            .collect();
        if !vars.iter().any(|var| var.contains(&under)) {
            continue;
        }
        if let Some(token) = vars
            .iter()
            .find_map(|var| var.strip_prefix("YULIUS_TASK_TOKEN="))
        {
            return Ok(token.to_owned());
        }
    }
    Err("no process of this run holds a task token".into())
}

/// SPEC-472 ATN-011 (review round 2): the agent asked for a secret (Needs you: answer it on the
/// Secrets page), then Claude Code asks the person something. The flag is the question's from
/// then on. The person answers the question first and the agent works on: that work answers
/// the flag, as it does for a question on an unflagged task, so the task does not keep reading
/// "its question is in the terminal" over no question. The secret's answer afterwards finds no
/// flag to end.
#[cfg(target_os = "linux")]
#[test]
fn a_question_that_took_a_secret_requests_flag_over_is_answered_by_the_agents_work() -> TestResult {
    use yulius_app::auth::AuthContext;
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _) = start(&data, &fake, noticing_agent(), 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = goal_task(&app, &sandbox, &user, "Merge on green").await?;
        wait_for("Done", Duration::from_secs(90), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let secret = app
            .create_agent_secret(
                &ctx,
                sandbox.as_str(),
                &serde_json::from_value(serde_json::json!({
                    "name": "DEPLOY_KEY",
                    "description": "Deploy key",
                    "value": "deploy-key-123456",
                    "auto_allow": false,
                }))?,
            )
            .await?;
        // The run's agent asks for the secret's hosts, with its own token.
        let asked = app
            .mcp_handle(
                &agent_token(&data)?,
                serde_json::from_value(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "secret_request_hosts",
                        "arguments": {"secret": "DEPLOY_KEY", "hosts": ["api.deploy.test"]},
                    },
                }))?,
            )
            .await
            .ok_or("no answer to the tool call")?;
        let answer = serde_json::to_value(&asked)?;
        assert!(
            answer["error"].is_null() && answer["result"]["isError"] != true,
            "{answer}"
        );
        let why = || support::attention(&app, id.as_str());
        assert_eq!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        assert_eq!(
            why().await?["action"],
            "Answer it on the Secrets page.",
            "the request's flag"
        );

        app.send_input(&id, b"/notify permission_prompt\r").await?;
        wait_for("the question's reason", Duration::from_secs(30), || async {
            screen_contains(&app, &id, "NOTIFIED").await
                && why().await.is_ok_and(|why| why["kind"] == "terminal")
        })
        .await?;
        // What the agent does next is later than the notice by more than the clock's step.
        transcript_read(&app, &id).await?;
        app.send_input(&id, b"/tool\r").await?;
        wait_for(
            "Working with the agent's work",
            Duration::from_secs(30),
            || async { activity(&app, &id).await == Some(Activity::Working) },
        )
        .await?;
        assert!(why().await?.is_null(), "no flag, no reason");

        // The request still waits on the Secrets page; its answer changes nothing of the task
        // (a dismissal: allowing needs the person's two-factor unlock, ADR-0317).
        app.answer_host_request(
            &ctx,
            &secret.id,
            &serde_json::from_value(
                serde_json::json!({"allow": false, "hosts": ["api.deploy.test"]}),
            )?,
        )
        .await?;
        assert_eq!(activity(&app, &id).await, Some(Activity::Working));
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A hooked agent with a transcript, as the notice tests run it.
fn noticing_agent() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    }
}

/// The new server read the session's transcript again and the task still reads Needs you: a
/// replay lifts no flag.
async fn still_flagged_after_the_replay(app: &Arc<App>, id: &TaskId) -> TestResult {
    let runtime = app.runtime(id);
    wait_for(
        "the transcript read again",
        Duration::from_secs(30),
        || async { runtime.transcript_rounds() >= 2 },
    )
    .await?;
    assert_eq!(activity(app, id).await, Some(Activity::NeedsAttention));
    Ok(())
}

/// SPEC-472 ATN-010 (#1221): Yulius restarts (an update freezes every agent mid-turn) while a
/// session asks its person something. The new server never saw the notice that flagged the
/// task; the person answers, the agent goes on, and its work still makes the task Working: it
/// does not read "Needs your action · its question is in the terminal" over a working session
/// until the turn ends.
#[test]
fn a_notice_is_answered_by_the_agents_work_after_a_restart() -> TestResult {
    a_restart_after(
        noticing_agent(),
        async |app, data| task_flagged_by_a_notice(app, data).await,
        async |_| Ok(()),
        async |app, id| {
            still_flagged_after_the_replay(app, id).await?;
            assert_eq!(
                support::attention(app, id.as_str()).await?["kind"],
                "terminal",
                "the flag still says what it is"
            );
            app.send_input(id, b"/tool\r").await?;
            wait_for(
                "Working with the agent's work",
                Duration::from_secs(30),
                || async { activity(app, id).await == Some(Activity::Working) },
            )
            .await?;
            assert!(support::attention(app, id.as_str()).await?.is_null());
            Ok(())
        },
    )
}

/// SPEC-472 ATN-010: what the session's work does not answer before a restart, it does not
/// answer after one: a worker's permission prompt stays through the main session's work, as
/// while Yulius watches (STA-003).
#[test]
fn a_workers_notice_is_not_answered_by_the_agents_work_after_a_restart() -> TestResult {
    a_restart_after(
        noticing_agent(),
        async |app, data| task_flagged_by_a_notice_of(app, data, "worker_permission_prompt").await,
        async |_| Ok(()),
        async |app, id| {
            still_flagged_after_the_replay(app, id).await?;
            app.send_input(id, b"/tool\r").await?;
            wait_for("the agent's work", Duration::from_secs(30), || {
                screen_contains(app, id, "TOOL")
            })
            .await?;
            transcript_read(app, id).await?;
            assert_eq!(
                activity(app, id).await,
                Some(Activity::NeedsAttention),
                "a worker's ask is not the session's to answer"
            );
            Ok(())
        },
    )
}

/// SPEC-472 ATN-010: the flag a notice raised before a restart was replaced without a word and
/// another matter flagged the task afterwards. The agent's work after the restart does not lift
/// that flag: the reason kept with the notice's flag speaks for that flag only.
#[test]
fn a_flag_raised_after_a_notices_is_not_lifted_after_a_restart() -> TestResult {
    a_restart_after(
        noticing_agent(),
        async |app, data| {
            let id = task_flagged_by_a_notice(app, data).await?;
            let store = &app.deps.store;
            store
                .set_activity(&id, Some(Activity::Working), app.now())
                .await?;
            store
                .set_activity(&id, Some(Activity::NeedsAttention), app.now())
                .await?;
            Ok(id)
        },
        async |_| Ok(()),
        async |app, id| {
            still_flagged_after_the_replay(app, id).await?;
            assert!(support::attention(app, id.as_str()).await?.is_null());
            app.send_input(id, b"/tool\r").await?;
            wait_for("the agent's work", Duration::from_secs(30), || {
                screen_contains(app, id, "TOOL")
            })
            .await?;
            transcript_read(app, id).await?;
            assert_eq!(
                activity(app, id).await,
                Some(Activity::NeedsAttention),
                "another matter's flag stays"
            );
            Ok(())
        },
    )
}

/// SPEC-469 GPW-004: Yulius restarts while a session whose goal Claude Code paused waits for a
/// person (Needs you, with the request). `down` runs while no server does, with the session's
/// transcript; `check` runs with the new server, once it reattached the run.
fn a_restart_while_a_goal_is_paused(
    agent: FakeAgentOptions,
    down: impl AsyncFnOnce(&Path) -> TestResult,
    check: impl AsyncFnOnce(&Arc<App>, &TaskId) -> TestResult,
) -> TestResult {
    a_restart_after(
        agent,
        async |app, data| paused_goal_task(app, data).await,
        down,
        check,
    )
}

/// [`a_restart_while_a_goal_is_paused`], with `pause` bringing the first server's task to its
/// paused goal.
fn a_restart_after(
    agent: FakeAgentOptions,
    pause: impl AsyncFnOnce(&Arc<App>, &Path) -> Result<TaskId, BoxError>,
    down: impl AsyncFnOnce(&Path) -> TestResult,
    check: impl AsyncFnOnce(&Arc<App>, &TaskId) -> TestResult,
) -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let (app, port) = start(&data, &fake, agent.clone(), 0).await?;
        let id = pause(&app, &data).await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        let record = TaskStore::task(&store, &id).await?.ok_or("task")?;
        let worktree = PathBuf::from(record.worktree_path.ok_or("no worktree")?);
        let attempt = TaskStore::live_attempt(&store, &id)
            .await?
            .ok_or("attempt")?;
        let session = attempt.provider_session_id.ok_or("session")?;
        let adapter = HarnessAdapter::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius")), "x");
        let home = data.join("sandboxes").join("s-done").join("home");
        let transcript = adapter.claude_transcript(&home, &worktree, &session);
        drop(store);
        down(&transcript).await?;
        let (app, _) = start(&data, &fake, agent, port).await?;
        app.reconcile().await?;
        check(&app, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-469 GPW-004: the session is still paused when Yulius starts again, and keeps its request.
/// Claude Code wakes it afterwards: the request ends then, although the new server never saw the
/// pause itself.
#[test]
fn a_session_woken_after_a_restart_no_longer_needs_you() -> TestResult {
    let (_keep, files) = data_dir()?;
    let (wake, hold) = (files.join("wake"), files.join("hold-turn"));
    let agent = FakeAgentOptions {
        paused_wake_file: Some(wake.clone()),
        turn_hold_file: Some(hold.clone()),
        ..pausing_agent()
    };
    a_restart_while_a_goal_is_paused(
        agent,
        async |_| Ok(()),
        async |app, id| {
            // The replay is applied (the transcript ends in the pause): still Needs you.
            let runtime = app.runtime(id);
            wait_for(
                "the transcript read again",
                Duration::from_secs(30),
                || async { runtime.transcript_rounds() >= 2 },
            )
            .await?;
            assert_eq!(activity(app, id).await, Some(Activity::NeedsAttention));
            assert!(pause_asks(app, id).await, "still paused: still asked");
            std::fs::write(&wake, "")?;
            wait_for(
                "the request to end with the agent's work",
                Duration::from_secs(60),
                || async { !pause_asks(app, id).await },
            )
            .await?;
            assert_eq!(activity(app, id).await, Some(Activity::Working));
            std::fs::write(&hold, "")?;
            wait_for("the woken turn's end", Duration::from_secs(60), || async {
                activity(app, id).await == Some(Activity::Waiting)
            })
            .await?;
            assert!(!pause_asks(app, id).await, "the request stays ended");
            Ok(())
        },
    )
}

/// SPEC-469 GPW-004 (review round 1): Yulius restarts while the session is paused, and the
/// session then goes on with no prompt anyone sees. The new server never saw the pause flag the
/// task, and still the agent's work is past that flag: the request ends and the task reads
/// Working, not Needs you until the turn ends.
#[test]
fn a_session_that_goes_on_unprompted_after_a_restart_no_longer_needs_you() -> TestResult {
    let (_keep, files) = data_dir()?;
    let (wake, hold) = (files.join("wake"), files.join("hold-turn"));
    let agent = FakeAgentOptions {
        paused_wake_file: Some(wake.clone()),
        paused_wake_hidden: true,
        turn_hold_file: Some(hold.clone()),
        ..pausing_agent()
    };
    a_restart_while_a_goal_is_paused(
        agent,
        async |_| Ok(()),
        async |app, id| {
            let runtime = app.runtime(id);
            wait_for(
                "the transcript read again",
                Duration::from_secs(30),
                || async { runtime.transcript_rounds() >= 2 },
            )
            .await?;
            assert_eq!(activity(app, id).await, Some(Activity::NeedsAttention));
            assert!(pause_asks(app, id).await, "still paused: still asked");
            std::fs::write(&wake, "")?;
            wait_for(
                "the request to end with the agent's work",
                Duration::from_secs(60),
                || async { !pause_asks(app, id).await },
            )
            .await?;
            wait_for("Working", Duration::from_secs(10), || async {
                activity(app, id).await == Some(Activity::Working)
            })
            .await?;
            std::fs::write(&hold, "")?;
            wait_for("the turn's end", Duration::from_secs(60), || async {
                activity(app, id).await == Some(Activity::Waiting)
            })
            .await?;
            assert!(!pause_asks(app, id).await, "the request stays ended");
            Ok(())
        },
    )
}

/// SPEC-469 GPW-004: Claude Code wakes the paused session while Yulius is down, and the agent
/// works. The start reads that from the transcript: the request ends and the task follows its
/// session (Working while the woken turn runs, waiting once it ended), not Needs you for good.
#[test]
fn a_session_woken_while_yulius_was_down_no_longer_needs_you() -> TestResult {
    let (_keep, files) = data_dir()?;
    let (wake, hold) = (files.join("wake"), files.join("hold-turn"));
    let agent = FakeAgentOptions {
        paused_wake_file: Some(wake.clone()),
        turn_hold_file: Some(hold.clone()),
        ..pausing_agent()
    };
    a_restart_while_a_goal_is_paused(
        agent,
        async |transcript| {
            std::fs::write(&wake, "")?;
            wait_for("the woken turn's work", Duration::from_secs(60), || async {
                std::fs::read_to_string(transcript).is_ok_and(|t| {
                    t.rsplit_once("bfakepaused")
                        .is_some_and(|(_, after)| after.contains(r#""stop_reason":"tool_use""#))
                })
            })
            .await
        },
        async |app, id| {
            wait_for(
                "the request to end at the start",
                Duration::from_secs(30),
                || async { !pause_asks(app, id).await },
            )
            .await?;
            wait_for("Working", Duration::from_secs(10), || async {
                activity(app, id).await == Some(Activity::Working)
            })
            .await?;
            std::fs::write(&hold, "")?;
            wait_for("the woken turn's end", Duration::from_secs(60), || async {
                activity(app, id).await == Some(Activity::Waiting)
            })
            .await?;
            assert!(!pause_asks(app, id).await, "the request stays ended");
            Ok(())
        },
    )
}

/// TSK-014 recovery: a restart replays the transcripts of reattached runs. A Done task stays Done
/// (a replayed prompt used to put it back to Working), and a goal achieved while Yulius was down
/// (its Stop hook unanswered) makes that task Done.
#[test]
fn a_restart_keeps_done_tasks_done_and_applies_goals_achieved_meanwhile() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        goal_delay_ms: Some(4_000),
        ..FakeAgentOptions::default()
    };

    // Phase 1: one task finishes; the other's goal turn is still running at the "crash".
    let first = runtime()?;
    let (port, done, late) = first.block_on(async {
        let (app, port) = start(&data, &fake, agent.clone(), 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let done = goal_task(&app, &sandbox, &user, "Finish before the restart").await?;
        wait_for("Done", Duration::from_secs(90), || async {
            activity(&app, &done).await == Some(Activity::Done)
        })
        .await?;
        let late = goal_task(&app, &sandbox, &user, "Finish during the restart").await?;
        wait_for("the second goal", Duration::from_secs(90), || {
            screen_contains(&app, &late, "GOAL SET")
        })
        .await?;
        Ok::<_, BoxError>((port, done, late))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    // While Yulius is down the second agent achieves its goal; its Stop hook goes unanswered.
    let rt = runtime()?;
    rt.block_on(async {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        let record = TaskStore::task(&store, &late).await?.ok_or("task")?;
        let worktree = PathBuf::from(record.worktree_path.ok_or("no worktree")?);
        let attempt = TaskStore::live_attempt(&store, &late)
            .await?
            .ok_or("attempt")?;
        let session = attempt.provider_session_id.ok_or("session")?;
        let adapter = HarnessAdapter::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius")), "x");
        let home = data.join("sandboxes").join("s-done").join("home");
        let transcript = adapter.claude_transcript(&home, &worktree, &session);
        wait_for(
            "the goal recorded meanwhile",
            Duration::from_secs(30),
            || async {
                std::fs::read_to_string(&transcript)
                    .is_ok_and(|t| t.contains("\"met\":true") && t.contains("turn_duration"))
            },
        )
        .await?;
        assert_eq!(
            record.activity,
            Some(Activity::Working),
            "the unanswered Stop left it Working"
        );
        drop(store);

        // Phase 2: a new server on the same port reconciles and reattaches.
        let (app, _) = start(&data, &fake, agent, port).await?;
        let mut events = app.events.subscribe();
        app.reconcile().await?;
        wait_for("the late goal applied", Duration::from_secs(20), || async {
            activity(&app, &late).await == Some(Activity::Done)
        })
        .await?;
        // Give the first task's replay time to have been read.
        tokio::time::sleep(Duration::from_millis(3_000)).await;
        assert_eq!(
            activity(&app, &done).await,
            Some(Activity::Done),
            "still Done"
        );
        while let Ok(envelope) = events.try_recv() {
            if let UiEvent::TaskUpdated { task } = envelope.event
                && task.id == done.as_str()
            {
                assert_ne!(
                    task.activity.as_deref(),
                    Some("working"),
                    "the replay put a Done task back to Working"
                );
            }
        }
        // Both sessions are still the same processes and keep working.
        app.send_input(&done, b"one more\r").await?;
        wait_for(
            "Your turn after typing",
            Duration::from_secs(20),
            || async { activity(&app, &done).await == Some(Activity::Waiting) },
        )
        .await?;
        stop(&app, &done).await?;
        stop(&app, &late).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The ptyhost and agent pids a run recorded.
fn run_pids(run_dir: &Path) -> Result<(i32, i32), BoxError> {
    let status: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("status.json"))?)?;
    let host = status["ptyhost_pid"].as_i64().ok_or("ptyhost_pid")?;
    let child = status["child_pid"].as_i64().ok_or("child_pid")?;
    Ok((i32::try_from(host)?, i32::try_from(child)?))
}

/// SPEC-229 (task #215, 2026-10-18: the host rebooted and #213/#214, Done with their pull requests
/// merged, read "Stopped (server restarted)" with a blank terminal): a reboot ends every session
/// while Yulius is down. A task whose goal was achieved lost nothing and is Closed (RBT-001); one
/// whose turn ended otherwise is Stopped (server restarted); both keep the screen their last turn
/// ended on (RBT-002).
#[test]
fn a_reboot_closes_a_done_task_and_keeps_every_screen() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let agent = FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    };

    // Phase 1: one task Done, the other's turn ended after a prompt typed after its goal.
    let first = runtime()?;
    let (done, waiting, sandbox, user) = first.block_on(async {
        let (app, _) = start(&data, &fake, agent.clone(), 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let done = goal_task(&app, &sandbox, &user, "Finish before the reboot").await?;
        let waiting = goal_task(&app, &sandbox, &user, "Wait at the reboot").await?;
        for id in [&done, &waiting] {
            wait_for("Done", Duration::from_secs(90), || async {
                activity(&app, id).await == Some(Activity::Done)
            })
            .await?;
        }
        app.send_input(&waiting, b"one more\r").await?;
        wait_for("Your turn", Duration::from_secs(20), || async {
            activity(&app, &waiting).await == Some(Activity::Waiting)
        })
        .await?;
        Ok::<_, BoxError>((done, waiting, sandbox, user))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    // An update first (the processes live on) to a server that saves screens, from one that did
    // not: the reattached idle runs save theirs at once, before any new turn.
    let update = runtime()?;
    update.block_on(async {
        let (app, _) = start(&data, &fake, agent.clone(), 0).await?;
        let mut run_dirs = Vec::new();
        for id in [&done, &waiting] {
            let attempt = app.deps.store.live_attempt(id).await?.ok_or("attempt")?;
            let run_dir = PathBuf::from(&attempt.run_dir);
            std::fs::remove_file(run_dir.join("final-screen"))?;
            run_dirs.push(run_dir);
        }
        app.reconcile().await?;
        for run_dir in &run_dirs {
            wait_for(
                "the reattached run's screen",
                Duration::from_secs(10),
                || {
                    let saved = run_dir.join("final-screen").exists();
                    async move { saved }
                },
            )
            .await?;
        }
        assert_eq!(
            activity(&app, &done).await,
            Some(Activity::Done),
            "still Done"
        );
        Ok::<_, BoxError>(())
    })?;
    update.shutdown_timeout(Duration::from_secs(5));

    // The reboot: every process ends with no exit recorded (the host first, so nobody records it).
    let rt = runtime()?;
    rt.block_on(async {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        let mut run_dirs = Vec::new();
        for id in [&done, &waiting] {
            let attempt = TaskStore::live_attempt(&store, id)
                .await?
                .ok_or("attempt")?;
            let run_dir = PathBuf::from(&attempt.run_dir);
            let (host, child) = run_pids(&run_dir)?;
            for pid in [host, child] {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            for pid in [host, child] {
                wait_for("the process gone", Duration::from_secs(10), || async move {
                    process_gone(pid.unsigned_abs())
                })
                .await?;
            }
            run_dirs.push(run_dir);
        }
        drop(store);

        // Phase 2: the server after the reboot. A Done task whose attempt was never recorded
        // live is swept by the same rule.
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let unattached = app
            .deps
            .store
            .insert_task(&TaskRecord {
                id: TaskId::parse("t-unattached")?,
                sandbox_id: sandbox.clone(),
                number: 0,
                title: "Done without a live attempt".into(),
                prompt: String::new(),
                harness: Harness::ClaudeCode,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Running,
                activity: Some(Activity::Done),
                branch: None,
                worktree_path: None,
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: user.clone(),
                owner: user.clone(),
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: TaskMode::Goal,
                supertask: false,
            })
            .await?;
        app.reconcile().await?;
        let swept = task(&app, &unattached.id).await?;
        assert_eq!(
            (swept.status, swept.closed_at.is_some()),
            (TaskStatus::Closed, true),
            "the sweep closes a Done task with no live attempt"
        );
        let closed = task(&app, &done).await?;
        assert_eq!(
            (closed.status, closed.closed_at.is_some()),
            (TaskStatus::Closed, true),
            "a finished task the reboot ended is Closed"
        );
        assert_eq!(
            task(&app, &waiting).await?.status,
            TaskStatus::Stopped(StopReason::ServerLost),
            "a task whose turn ended without its goal is Stopped (server restarted)"
        );
        for (run_dir, text) in run_dirs
            .iter()
            .zip(["ECHO: /goal Finish before the reboot", "ECHO: one more"])
        {
            let screen = std::fs::read(run_dir.join("final-screen"))
                .map_err(|e| format!("no saved screen in {}: {e}", run_dir.display()))?;
            assert!(
                String::from_utf8_lossy(&screen).contains(text),
                "the saved screen shows how the last turn ended ({text}): {:?}",
                String::from_utf8_lossy(&screen)
            );
        }
        Ok::<_, BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

async fn chain_status(h: &support::Harness, id: &TaskId) -> Option<TaskStatus> {
    TaskStore::task(&h.store, id)
        .await
        .ok()
        .flatten()
        .map(|t| t.status)
}

/// TSK-012 with TSK-014: a task chained after another starts once its predecessor is Done — and
/// an achieved goal is Done without a pull request — but not while the predecessor only waits
/// for the user.
#[tokio::test]
async fn a_chain_starts_when_its_predecessor_achieves_its_goal() -> TestResult {
    use support::{HarnessOptions, call, harness_with, perms, seed_github_user, seed_sandbox};
    use yulius_app::ports::store::AccountStore;

    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("acc-1")?,
            provider: Provider::Anthropic,
            label: "acc-1".into(),
            credential: Some(h.app.deps.crypto.seal(b"oauth-token")?),
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
    let pred = support::seed_task(&h, "t-pred", "s1", "u-alice", TaskStatus::Running, None).await?;
    let next = call(
        &h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(serde_json::json!({"prompt": "next step", "after_task_id": "t-pred"})),
        &alice,
    )
    .await?;
    let next_id = TaskId::parse(next.body["id"].as_str().ok_or("id")?)?;
    assert_eq!(chain_status(&h, &next_id).await, Some(TaskStatus::Chained));

    // Waiting for the user, no pull request: not Done.
    h.store
        .set_activity(&pred.id, Some(Activity::Waiting), h.app.now())
        .await?;
    h.app.advance_chains(&pred.id).await;
    assert_eq!(chain_status(&h, &next_id).await, Some(TaskStatus::Chained));

    // The goal is achieved: Done, and the chained task is released into the normal start.
    h.store
        .set_activity(&pred.id, Some(Activity::Done), h.app.now())
        .await?;
    h.app.advance_chains(&pred.id).await;
    assert_eq!(chain_status(&h, &next_id).await, Some(TaskStatus::Queued));
    Ok(())
}

/// TSK-014, UI-006: a Done task (goal achieved, no shell left) can be archived: the session is
/// closed as it is archived. A task that only waits for the user still has to be stopped first.
#[test]
fn a_done_task_can_be_archived() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        wait_for("the shell probe", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        let archived = app.archive_task(&ctx, id.as_str(), true).await?;
        assert!(archived.archived);
        // Closed, not stopped: the finished session ends like one the user left.
        wait_for("the close", Duration::from_secs(30), || async {
            task(&app, &id)
                .await
                .is_ok_and(|t| t.status == TaskStatus::Closed)
        })
        .await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-153 OLC-003 (#431, SPEC-106 OTR-004): a live, Done onboarding task with plan steps open,
/// in a `preparing` sandbox whose project validates, is archived: closed, its run's end awaited
/// (an open plan does not make the sandbox ready while the run lives), then archived, and the
/// sandbox is `ready`.
#[test]
fn a_done_onboarding_task_is_archived_and_the_sandbox_is_ready() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        // The project validates in the sandbox's clone.
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        for (path, contents) in yulius_domain::spec_format::scaffold_files(
            "done",
            yulius_domain::spec_format::TemplateKind::Empty,
        ) {
            let target = repo.join(path);
            std::fs::create_dir_all(target.parent().ok_or("no parent")?)?;
            std::fs::write(target, contents)?;
        }
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Prepare this repository and verify it".into(),
                harness: Harness::ClaudeCode,
                title: Some("Prepare this repository for Yulius".into()),
                workflow_id: None,
                steps: vec!["Write the docs".into(), "Lay the gate".into()],
                model: None,
                effort: None,
                origin: TaskOrigin::Onboarding,
                automation_run: None,
                start: true,
                mode: TaskMode::Goal,
                supertask: false,
            })
            .await?
            .id;
        app.deps
            .store
            .set_onboarding(&sandbox, "preparing", Some(&id))
            .await?;
        wait_for("the shell probe", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        let record = app.deps.store.sandbox(&sandbox).await?.ok_or("sandbox")?;
        assert_eq!(record.onboarding_state, "preparing", "the plan is open");
        let archived = app.archive_task(&ctx, id.as_str(), true).await?;
        assert!(archived.archived);
        assert_eq!(
            archived.status, "closed",
            "the run ended before the archive"
        );
        let record = app.deps.store.sandbox(&sandbox).await?.ok_or("sandbox")?;
        assert_eq!(record.onboarding_state, "ready");
        let stored = task(&app, &id).await?;
        assert_eq!(stored.status, TaskStatus::Closed);
        assert!(stored.archived_at.is_some());
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The details of the sandbox's onboarding audit entries (`<from> -> <to>`), oldest first.
fn onboarding_audit(data: &Path, sandbox: &SandboxId) -> Result<Vec<String>, BoxError> {
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    let mut query = db.prepare(
        "SELECT detail FROM audit_log WHERE action = 'sandbox.onboarding' AND target = ?1 \
         ORDER BY id",
    )?;
    let rows = query.query_map([sandbox.as_str()], |row| row.get::<_, Option<String>>(0))?;
    Ok(rows.filter_map(Result::ok).flatten().collect())
}

/// SPEC-153 OLC-001 (#406): an admin's Skip that lands while Start onboarding is starting the
/// onboarding task (held at its run's attach) is never undone: the sandbox stays `skipped`.
#[test]
fn a_skip_during_start_onboarding_stays_skipped() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
            "CARGO_BIN_EXE_yulius"
        ))));
        let (app, _) = start_on(
            &data,
            &fake,
            FakeAgentOptions::default(),
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        app.deps
            .store
            .set_onboarding(&sandbox, "pending", None)
            .await?;
        let person = app.deps.store.user(&user).await?.ok_or("user")?;
        let browser = yulius_app::auth::AuthContext {
            user: person.clone(),
            csrf_token: Some("csrf".into()),
            session_hash: Some("session".into()),
            token: None,
        };
        let admin = yulius_app::auth::AuthContext {
            user: person,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        host.arm();
        let starter = Arc::clone(&app);
        let sid = sandbox.to_string();
        let start =
            tokio::spawn(async move { starter.approve_onboarding(&browser, &sid, None).await });
        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
        // The start is inside `start_run`; the admin skips now.
        let skipper = Arc::clone(&app);
        let sid = sandbox.to_string();
        let skip = tokio::spawn(async move { skipper.skip_onboarding(&admin, &sid).await });
        wait_for("the skip", Duration::from_secs(30), || async {
            app.deps
                .store
                .sandbox(&sandbox)
                .await
                .ok()
                .flatten()
                .is_some_and(|s| s.onboarding_state == "skipped")
        })
        .await?;
        host.release();
        let started = tokio::time::timeout(Duration::from_secs(60), start).await??;
        tokio::time::timeout(Duration::from_secs(60), skip).await???;
        started?;
        let record = app.deps.store.sandbox(&sandbox).await?.ok_or("sandbox")?;
        assert_eq!(
            record.onboarding_state,
            "skipped",
            "a start that ran during the skip wrote over it: {:?}",
            onboarding_audit(&data, &sandbox)?
        );
        let audit = onboarding_audit(&data, &sandbox)?;
        let skipped_at = audit
            .iter()
            .position(|entry| entry.ends_with("-> skipped"))
            .ok_or("no skip audited")?;
        assert_eq!(skipped_at, audit.len() - 1, "{audit:?}");
        let prep = record.onboarding_task_id.ok_or("no onboarding task")?;
        stop(&app, &prep).await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-153 OLC-001 (review): a refresh that sees the onboarding task start writes `preparing`
/// (as `system`) before Start onboarding does; the start's own write then loses, and the person
/// who pressed Start onboarding is still in the audit log.
#[test]
fn a_start_onboarding_a_refresh_got_ahead_of_is_audited_as_its_person() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(support::gated::GatedHost::new(PathBuf::from(env!(
            "CARGO_BIN_EXE_yulius"
        ))));
        let (app, _) = start_on(
            &data,
            &fake,
            FakeAgentOptions::default(),
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        app.deps
            .store
            .set_onboarding(&sandbox, "pending", None)
            .await?;
        let browser = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: Some("csrf".into()),
            session_hash: Some("session".into()),
            token: None,
        };
        host.arm();
        let starter = Arc::clone(&app);
        let sid = sandbox.to_string();
        let start =
            tokio::spawn(async move { starter.approve_onboarding(&browser, &sid, None).await });
        tokio::time::timeout(Duration::from_secs(60), host.reached()).await?;
        // The task runs (its attach is held): a read refreshes onboarding first.
        assert_eq!(app.refresh_onboarding(&sandbox).await?, "preparing");
        host.release();
        tokio::time::timeout(Duration::from_secs(60), start).await???;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        let mut query = db.prepare(
            "SELECT actor FROM audit_log WHERE action LIKE 'sandbox.onboarding%' AND target = ?1 \
             ORDER BY id",
        )?;
        let actors: Vec<String> = query
            .query_map([sandbox.as_str()], |row| row.get::<_, String>(0))?
            .filter_map(Result::ok)
            .collect();
        assert!(
            actors.iter().any(|actor| actor == "admin"),
            "who started onboarding is not audited: {actors:?}"
        );
        let record = app.deps.store.sandbox(&sandbox).await?.ok_or("sandbox")?;
        assert_eq!(record.onboarding_state, "preparing");
        stop(
            &app,
            &record.onboarding_task_id.ok_or("no onboarding task")?,
        )
        .await?;
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-039 (DDL-001): a Done task can be deleted like an ended one: its idle session is closed
/// first, and the deletion returns only once the run has ended, so nothing of it is left behind.
#[test]
fn a_done_task_can_be_deleted_its_session_is_closed_first() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        wait_for("the shell probe", Duration::from_secs(90), || async {
            shown(&app, &id).await.ok() == Some((Some("done".into()), Some(0)))
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("the Done task has no live session")?;
        let agent_pid = attempt.child_pid.ok_or("no agent pid")?;
        let mut events = app.events.subscribe();
        let started = Instant::now();
        app.delete_task(&ctx, id.as_str()).await?;
        // Closed gracefully, not stopped and not lost through the 20 s escalation.
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the close took {:?}",
            started.elapsed()
        );
        let mut closed = false;
        while let Ok(envelope) = events.try_recv() {
            if let UiEvent::TaskUpdated { task } = envelope.event
                && task.id == id.as_str()
                && task.status == "closed"
            {
                closed = true;
            }
        }
        assert!(closed, "the task was closed before it was deleted");
        assert!(
            app.deps.store.task(&id).await?.is_none(),
            "the task is deleted"
        );
        assert!(
            !Path::new(&attempt.run_dir).exists(),
            "the run directory is removed"
        );
        assert!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(i32::try_from(agent_pid)?), None)
                .is_err(),
            "the session ended before the task was deleted"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The real process host, except that asking a run to shut down never completes (a host that
/// stopped reading its socket) and, with `survivor`, its agent outlives termination: only the
/// host (`survivor_host`, its pid) is killed, `terminate` fails and the run reads as an agent
/// still alive without its host.
#[derive(Default)]
struct StuckHost {
    inner: Option<PtySessionHost>,
    survivor: bool,
    survivor_host: std::sync::atomic::AtomicU32,
    killed: std::sync::atomic::AtomicBool,
    terminations: std::sync::atomic::AtomicU32,
}

impl StuckHost {
    fn new(survivor: bool) -> Self {
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        Self {
            inner: Some(PtySessionHost::new(exe)),
            survivor,
            ..Self::default()
        }
    }

    fn inner(&self) -> yulius_app::error::AppResult<&PtySessionHost> {
        self.inner
            .as_ref()
            .ok_or_else(|| yulius_app::error::AppError::Upstream("no host".into()))
    }
}

struct StuckShutdownWriter(Box<dyn yulius_app::ports::session::RunWriter>);

#[async_trait::async_trait]
impl yulius_app::ports::session::RunWriter for StuckShutdownWriter {
    async fn input(&mut self, bytes: &[u8]) -> yulius_app::error::AppResult<()> {
        self.0.input(bytes).await
    }
    async fn resize(&mut self, cols: u16, rows: u16) -> yulius_app::error::AppResult<()> {
        self.0.resize(cols, rows).await
    }
    async fn signal(
        &mut self,
        signal: yulius_app::ports::session::RunSignal,
    ) -> yulius_app::error::AppResult<()> {
        self.0.signal(signal).await
    }
    async fn shutdown(&mut self, _: u32) -> yulius_app::error::AppResult<()> {
        std::future::pending().await
    }
}

#[async_trait::async_trait]
impl SessionHost for StuckHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &yulius_app::ports::session::LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::SpawnedRun> {
        self.inner()?
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }
    async fn attach(
        &self,
        run_dir: &Path,
        token: &[u8; 32],
    ) -> yulius_app::error::AppResult<yulius_app::ports::session::AttachedRun> {
        let mut run = self.inner()?.attach(run_dir, token).await?;
        run.writer = Box::new(StuckShutdownWriter(run.writer));
        Ok(run)
    }
    fn inspect(&self, run_dir: &Path) -> yulius_app::ports::session::RunInspection {
        if self.killed.load(std::sync::atomic::Ordering::SeqCst) {
            return yulius_app::ports::session::RunInspection::Recorded {
                host_alive: false,
                child_alive: true,
                same_boot: true,
            };
        }
        self.inner().map_or(
            yulius_app::ports::session::RunInspection::Missing,
            |inner| inner.inspect(run_dir),
        )
    }
    fn terminate(&self, run_dir: &Path) -> yulius_app::error::AppResult<()> {
        self.terminations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.survivor {
            return self.inner()?.terminate(run_dir);
        }
        self.killed.store(true, std::sync::atomic::Ordering::SeqCst);
        let host = self.survivor_host.load(std::sync::atomic::Ordering::SeqCst);
        if let Ok(pid) = i32::try_from(host)
            && pid > 0
        {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        Err(yulius_app::error::AppError::Upstream(
            "process 4242 still alive after SIGKILL".into(),
        ))
    }
}

/// Kills a process group when dropped.
struct KillGroupOnDrop(Option<i32>);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

/// Whether process `pid` is gone: no longer there, or a zombie waiting to be reaped.
fn process_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
    })
}

/// A running goal task on `host` (escalation 300 ms), its auth context and its live run.
async fn running_task(
    data: &Path,
    fake: &Path,
    host: Arc<StuckHost>,
) -> Result<
    (
        Arc<App>,
        yulius_app::auth::AuthContext,
        TaskId,
        yulius_app::model::AttemptRecord,
    ),
    BoxError,
> {
    let (app, _) = start_on(
        data,
        fake,
        FakeAgentOptions::default(),
        0,
        Some(host),
        Some(300),
        false,
        None,
    )
    .await?;
    let (sandbox, user) = seed(&app, data).await?;
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(&user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
    wait_for("the session", Duration::from_secs(60), || async {
        app.deps
            .store
            .live_attempt(&id)
            .await
            .is_ok_and(|a| a.is_some_and(|a| a.child_pid.is_some()))
            // Output arrives once Yulius is attached to the run (it holds its writer).
            && !app.runtime(&id).hub.screen_text().trim().is_empty()
    })
    .await?;
    let attempt = app
        .deps
        .store
        .live_attempt(&id)
        .await?
        .ok_or("no live run")?;
    Ok((app, ctx, id, attempt))
}

/// SPEC-065 END-001 (#284, review): when the agent survives termination but its host is gone,
/// the connection to the run is lost; the run is still not recorded as ended, so the task stays
/// undeletable.
#[test]
fn a_run_whose_agent_survives_termination_is_not_ended_when_its_connection_is_lost() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let host = Arc::new(StuckHost::new(true));
        let (app, ctx, id, attempt) = running_task(&data, &fake, host.clone()).await?;
        host.survivor_host.store(
            attempt.ptyhost_pid.ok_or("no host pid")?,
            std::sync::atomic::Ordering::SeqCst,
        );
        // What remains of the run goes with the test, whatever its outcome.
        let _agent = KillGroupOnDrop(attempt.child_pid.and_then(|p| i32::try_from(p).ok()));
        app.stop_task_now(&id).await?;
        // The escalation's terminate, then the lost connection's.
        wait_for(
            "the second termination",
            Duration::from_secs(20),
            || async { host.terminations.load(std::sync::atomic::Ordering::SeqCst) >= 2 },
        )
        .await?;
        assert!(
            app.deps.store.live_attempt(&id).await?.is_some(),
            "the run is not recorded as ended"
        );
        let refused = app.delete_task(&ctx, id.as_str()).await;
        assert!(
            matches!(refused, Err(yulius_app::error::AppError::Conflict(_))),
            "{refused:?}"
        );
        assert!(
            app.deps.store.task(&id).await?.is_some(),
            "nothing is deleted"
        );
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-065 END-005 and END-003 (#290, #285): a run whose host never takes the shutdown request
/// is still escalated and ended when it is closed for deletion; when the close returns, the run's
/// ending is fully recorded, so the task chained after it already has its outcome, and deleting
/// the predecessor then gives it one outcome: "#N was deleted".
#[test]
fn a_close_whose_shutdown_blocks_still_ends_the_run_and_its_successor_sees_one_outcome()
-> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, ctx, id, attempt) =
            running_task(&data, &fake, Arc::new(StuckHost::new(false))).await?;
        let agent_pid = attempt.child_pid.ok_or("no agent pid")?;
        let before = task(&app, &id).await?;

        // A task waiting for this one ("Run after #N").
        let now = app.now();
        let mut waiting = before.clone();
        waiting.id = TaskId::parse("t-next")?;
        waiting.title = "Next".into();
        waiting.status = TaskStatus::Chained;
        waiting.activity = None;
        waiting.branch = None;
        waiting.worktree_path = None;
        let waiting = app.deps.store.insert_task(&waiting).await?;
        yulius_app::ports::store_ext::IntakeStore::insert_chain(
            app.deps.store.as_ref(),
            &yulius_app::intake::ChainRecord {
                task_id: waiting.id.clone(),
                after_task_id: Some(id.clone()),
                after_number: before.number,
                blocked_reason: None,
                created_at: now,
                rule: None,
            },
        )
        .await?;
        let chain = || async {
            yulius_app::ports::store_ext::IntakeStore::chain(app.deps.store.as_ref(), &waiting.id)
                .await
                .ok()
                .flatten()
                .and_then(|c| c.blocked_reason)
        };

        tokio::time::timeout(Duration::from_secs(10), app.close_task_and_wait(&id)).await??;
        assert!(
            app.deps.store.live_attempt(&id).await?.is_none(),
            "the run ended"
        );
        assert!(process_gone(agent_pid), "the agent process is gone");
        let decided = chain().await.ok_or("the waiting task has no outcome yet")?;
        assert!(
            decided.starts_with(&format!("#{} ", before.number)),
            "{decided}"
        );

        app.delete_task(&ctx, id.as_str()).await?;
        assert_eq!(
            chain().await.as_deref(),
            Some(
                format!(
                    "#{} was deleted. Start this task now or cancel it.",
                    before.number
                )
                .as_str()
            )
        );
        assert_eq!(task(&app, &waiting.id).await?.status, TaskStatus::Chained);
        Ok::<(), BoxError>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The real session host whose spawns wait, their launch plan computed, until the test lets them
/// go ahead.
struct HeldSpawn {
    inner: PtySessionHost,
    reached: tokio::sync::watch::Sender<bool>,
    released: tokio::sync::watch::Sender<bool>,
}

#[async_trait::async_trait]
impl SessionHost for HeldSpawn {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        self.reached.send_replace(true);
        let mut released = self.released.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(60), released.wait_for(|go| *go)).await;
        self.inner
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        self.inner.attach(run_dir, token).await
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        self.inner.background_shells(child_pid)
    }
}

/// SPEC-248 FUP-002 (review round 1): control passing while a run is launched — its fingerprint
/// file computed for the previous owner, not yet written — still leaves the run the new owner's
/// fingerprints once it is live.
#[test]
fn a_hand_over_during_a_launch_leaves_the_new_owners_fingerprints() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let host = Arc::new(HeldSpawn {
            inner: PtySessionHost::new(exe),
            reached: tokio::sync::watch::channel(false).0,
            released: tokio::sync::watch::channel(false).0,
        });
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(
            &data,
            &fake,
            agent,
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let bob = UserRecord {
            id: UserId::parse("u-bob")?,
            github_login: Some("bob".into()),
            github_id: None,
            display_name: "Bob".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
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
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.create_agent_secret(
            &ctx,
            sandbox.as_str(),
            &yulius_contracts::api::secrets::AgentSecretInput {
                scope: Some("personal".into()),
                name: "MY_TOKEN".into(),
                value: "admin-personal-token-0123456".into(),
                hosts: vec!["api.mine.test".into()],
                injection: Some("bearer".into()),
                ..Default::default()
            },
        )
        .await?;
        let bobs = yulius_app::auth::AuthContext {
            user: bob.clone(),
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.create_agent_secret(
            &bobs,
            sandbox.as_str(),
            &yulius_contracts::api::secrets::AgentSecretInput {
                scope: Some("personal".into()),
                name: "BOB_TOKEN".into(),
                value: "bob-personal-token-0b0b5ec7".into(),
                hosts: vec!["api.bob.test".into()],
                injection: Some("bearer".into()),
                ..Default::default()
            },
        )
        .await?;
        let launch = goal_task(&app, &sandbox, &user, "Hand over");
        let hand_over = async {
            let mut reached = host.reached.subscribe();
            tokio::time::timeout(Duration::from_secs(60), reached.wait_for(|r| *r)).await??;
            let attempt = app
                .deps
                .store
                .live_attempts()
                .await?
                .pop()
                .ok_or("no launching run")?;
            // Control passes now: the launch already computed the previous owner's file.
            assert!(
                app.deps
                    .store
                    .take_control(&attempt.task_id, &user, &bob.id, app.now())
                    .await?
            );
            host.released.send_replace(true);
            let file = Path::new(&attempt.run_dir).join(yulius_app::vault::FINGERPRINTS_FILE);
            let names = || {
                std::fs::read(&file)
                    .ok()
                    .and_then(|b| {
                        serde_json::from_slice::<yulius_contracts::scrub::FingerprintSet>(&b).ok()
                    })
                    .map(|set| set.names)
            };
            let live = || async {
                app.deps
                    .store
                    .live_attempt(&attempt.task_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|a| a.state == AttemptState::Live)
            };
            wait_for("the live run", Duration::from_secs(30), live).await?;
            wait_for(
                "the new owner's fingerprints",
                Duration::from_secs(10),
                || async {
                    names().is_some_and(|n| {
                        n.iter().any(|n| n == "BOB_TOKEN") && !n.iter().any(|n| n == "MY_TOKEN")
                    })
                },
            )
            .await
            .map_err(|e| format!("{e}: {:?}", names()))?;
            Ok::<_, BoxError>(attempt.task_id)
        };
        let (launched, handed) = tokio::join!(launch, hand_over);
        let id = handed?;
        assert_eq!(launched?, id);
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// The server names in a run's own MCP configuration (`mcp.json` in its run directory).
fn run_mcp_servers(run_dir: &str) -> Result<Vec<String>, BoxError> {
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(Path::new(run_dir).join("mcp.json"))?)?;
    let mut names: Vec<String> = mcp["mcpServers"]
        .as_object()
        .ok_or("mcpServers")?
        .keys()
        .cloned()
        .collect();
    names.sort_unstable();
    Ok(names)
}

/// Review of SPEC-332 (round 3): control passing while a run is launched with its owner's
/// personal MCP server — the run's configuration already made, its attempt not yet live — still
/// leaves no run holding that server once it is live: the run resumes for the new owner.
#[test]
fn a_hand_over_during_a_launch_restarts_a_run_holding_personal_mcp_servers() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
        let host = Arc::new(HeldSpawn {
            inner: PtySessionHost::new(exe),
            reached: tokio::sync::watch::channel(false).0,
            released: tokio::sync::watch::channel(false).0,
        });
        let agent = FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start_on(
            &data,
            &fake,
            agent,
            0,
            Some(host.clone()),
            None,
            false,
            None,
        )
        .await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let bob = UserId::parse("u-bob")?;
        app.deps
            .store
            .insert_user(&UserRecord {
                id: bob.clone(),
                github_login: Some("bob".into()),
                github_id: None,
                display_name: "Bob".into(),
                avatar_url: None,
                is_superadmin: false,
                is_instance_admin: false,
                password_hash: None,
                created_at: Timestamp(1),
                disabled_at: None,
            })
            .await?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        app.create_mcp_server(
            &ctx,
            sandbox.as_str(),
            &yulius_contracts::api::mcp::McpServerInput {
                name: "fleet".into(),
                transport: "http".into(),
                command: None,
                args: Vec::new(),
                url: Some("https://fleet.test/mcp".into()),
                env: None,
                headers: None,
                enabled: None,
                scope: Some("instance".into()),
                audience: Some("personal".into()),
                sandbox: None,
            },
        )
        .await?;
        let launch = goal_task(&app, &sandbox, &user, "Hand over");
        let hand_over = async {
            let mut reached = host.reached.subscribe();
            tokio::time::timeout(Duration::from_secs(60), reached.wait_for(|r| *r)).await??;
            let attempt = app
                .deps
                .store
                .live_attempts()
                .await?
                .pop()
                .ok_or("no launching run")?;
            // Control passes now: the launch already made the previous owner's configuration.
            assert!(
                app.deps
                    .store
                    .take_control(&attempt.task_id, &user, &bob, app.now())
                    .await?
            );
            host.released.send_replace(true);
            let id = attempt.task_id.clone();
            wait_for("the resumed run", Duration::from_secs(60), || async {
                let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
                attempts.len() == 2 && attempts[1].state == AttemptState::Live
            })
            .await?;
            let attempts = app.deps.store.attempts(&id).await?;
            assert_eq!(run_mcp_servers(&attempts[0].run_dir)?, ["fleet", "yulius"]);
            assert_eq!(run_mcp_servers(&attempts[1].run_dir)?, ["yulius"]);
            Ok::<_, BoxError>(id)
        };
        let (launched, handed) = tokio::join!(launch, hand_over);
        let id = handed?;
        assert_eq!(launched?, id);
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 CHN-002 (13 November 2026: #466, #467 and #468 were started "after #465" while it
/// went from one merged slice to the next): a merged pull request makes a running task Done only
/// once the task was told of the merge and the turn that news starts has ended. A task chained
/// after it waits through all of it: while the news is not typed, and (review round 1, H1) while
/// it is typed and its turn has not begun. Real hooks: the prompt hook of the news is held at
/// its check, where the task still reads idle.
#[test]
fn a_chained_task_waits_until_its_predecessor_heard_of_its_merge() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_app::race::Point;
    use yulius_domain::delivery::DeliveryStage;

    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            answers_first_message: true,
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let new_task = |title: &'static str| NewTask {
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
        };
        let id = app.create_task_internal(new_task("Sliced")).await?.id;
        wait_for("the first answer", Duration::from_secs(90), || {
            screen_contains(&app, &id, "ECHO: Sliced: answer with an echo")
        })
        .await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let bounds = WatchBounds::default();
        // The watch sees the pull request open: its merge will be told.
        assert!(app.watch_prs(&bounds).await.is_empty());

        let before = task(&app, &id).await?;
        let next = app
            .create_task_with(new_task("Next"), Some(&before))
            .await?
            .id;
        let status = || async { task(&app, &next).await.ok().map(|task| task.status) };
        let waits = || async {
            let task = task(&app, &id).await?;
            let summary = app.task_summary(&task).await?;
            Ok::<_, BoxError>((task.activity, summary.yulius_wait))
        };
        assert_eq!(status().await, Some(TaskStatus::Chained));

        // Merged while its agent is idle with nothing open: what read Done at once.
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
            ..open
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
        app.advance_chains(&id).await;
        app.advance_all_chains().await;
        assert_eq!(
            status().await,
            Some(TaskStatus::Chained),
            "its agent has not heard of the merge yet"
        );
        assert_eq!(waits().await?, (Some(Activity::Waiting), true));

        // The news is typed, and its prompt hook waits at its check: the agent has the news,
        // and the task still reads idle.
        let (at_check, go_on) = support::hold_point(Point::PromptTurnChecked, id.as_str());
        let typed = app.watch_prs(&bounds).await;
        assert!(
            typed
                .iter()
                .any(|(task, text)| task == &id && text.contains("Pull request #7 is merged")),
            "{typed:?}"
        );
        support::point_reached(at_check).await?;
        app.advance_chains(&id).await;
        assert_eq!(
            status().await,
            Some(TaskStatus::Chained),
            "the turn the news starts has not ended"
        );
        assert_eq!(waits().await?, (Some(Activity::Waiting), true));

        // That turn runs and ends: Done, and the chain goes on by the turn's end alone.
        let _ = go_on.send(());
        wait_for(
            "the chained task's start",
            Duration::from_secs(60),
            || async { status().await != Some(TaskStatus::Chained) },
        )
        .await?;
        assert!(
            screen_contains(&app, &id, "ECHO: Yulius, about your pull request #7").await,
            "the news' turn ran"
        );
        assert_eq!(waits().await?, (Some(Activity::Waiting), false));
        stop(&app, &next).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 CHN-002 (review round 2, R2-1): news the watch types while a turn runs waits in
/// Claude Code's queue (its prompt hook ran at the Enter) and is handed on after that turn's
/// Stop. Between that Stop and the handed-on turn the task reads idle and told; it still goes
/// on by itself, so a task chained after it keeps waiting.
#[test]
fn news_typed_while_a_turn_runs_is_due_until_its_own_turn() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::delivery::DeliveryStage;

    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    // A turn ends only while the first file exists, a handed-on turn starts only with the second.
    let (may_end, may_turn) = (data.join("turn-may-end"), data.join("queue-may-turn"));
    std::fs::write(&may_end, b"")?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            answers_first_message: true,
            queue_prompts: true,
            turn_hold_file: Some(may_end.clone()),
            queue_turn_file: Some(may_turn.clone()),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let new_task = |title: &'static str| NewTask {
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
        };
        let id = app.create_task_internal(new_task("Sliced")).await?.id;
        wait_for("the first answer", Duration::from_secs(90), || {
            screen_contains(&app, &id, "ECHO: Sliced: answer with an echo")
        })
        .await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let bounds = WatchBounds::default();
        assert!(app.watch_prs(&bounds).await.is_empty());
        let before = task(&app, &id).await?;
        let next = app
            .create_task_with(new_task("Next"), Some(&before))
            .await?
            .id;
        let status = || async { task(&app, &next).await.ok().map(|task| task.status) };

        // A turn runs (held), and the merge lands meanwhile: its news is typed into the queue.
        std::fs::remove_file(&may_end)?;
        app.send_input(&id, b"Go on with the next slice\r").await?;
        wait_for("the running turn", Duration::from_secs(30), || {
            screen_contains(&app, &id, "TURN HELD")
        })
        .await?;
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
            ..open
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
        let typed = app.watch_prs(&bounds).await;
        assert!(
            typed
                .iter()
                .any(|(task, text)| task == &id && text.contains("Pull request #7 is merged")),
            "{typed:?}"
        );
        wait_for("the queued news", Duration::from_secs(30), || {
            screen_contains(&app, &id, "QUEUED: Yulius, about your pull request #7")
        })
        .await?;

        // That turn ends; the queue is not handed on yet. Idle and told, and still going on.
        std::fs::write(&may_end, b"")?;
        wait_for("the held hand-over", Duration::from_secs(30), || {
            screen_contains(&app, &id, "QUEUED TURN HELD")
        })
        .await?;
        wait_for("the turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        app.advance_chains(&id).await;
        assert_eq!(
            status().await,
            Some(TaskStatus::Chained),
            "the news' own turn has not begun"
        );
        let predecessor = task(&app, &id).await?;
        assert!(app.task_summary(&predecessor).await?.yulius_wait);

        // The news' turn runs and ends: the chain goes on.
        std::fs::write(&may_turn, b"")?;
        // (That turn's own end may look before its lines were read: the watch's next pass, run
        // here as its 20 s loop would, lets the chain go on.)
        wait_for(
            "the chained task's start",
            Duration::from_secs(60),
            || async {
                app.watch_prs(&bounds).await;
                status().await != Some(TaskStatus::Chained)
            },
        )
        .await?;
        let predecessor = task(&app, &id).await?;
        assert!(
            !app.task_summary(&predecessor).await?.yulius_wait,
            "its news' turn ran"
        );
        stop(&app, &next).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-487 CHN-002 (review round 3, R3-1): Claude Code hands a notification of its own, a
/// slash command or a `!` line on alone, and a prompt behind it at the next turn's end
/// (SPEC-479). News that waits in the queue behind such an entry is still due after that
/// entry's turn: the queue holds it, whatever turn began after it was typed.
#[test]
fn queued_news_stays_due_while_another_entry_gets_its_turn_first() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::delivery::DeliveryStage;

    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    // A turn ends only while the first file exists, a handed-on turn starts only with the second.
    let (may_end, may_turn) = (data.join("turn-may-end"), data.join("queue-may-turn"));
    let may_hand = data.join("queue-may-hand");
    std::fs::write(&may_hand, b"")?;
    std::fs::write(&may_turn, b"")?;
    std::fs::write(&may_end, b"")?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            answers_first_message: true,
            queue_prompts: true,
            turn_hold_file: Some(may_end.clone()),
            queue_turn_file: Some(may_turn.clone()),
            queue_handover_file: Some(may_hand.clone()),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let new_task = |title: &'static str| NewTask {
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
        };
        let id = app.create_task_internal(new_task("Sliced")).await?.id;
        wait_for("the first answer", Duration::from_secs(90), || {
            screen_contains(&app, &id, "ECHO: Sliced: answer with an echo")
        })
        .await?;
        wait_for("the first turn's end", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Waiting)
        })
        .await?;
        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(7),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;
        let bounds = WatchBounds::default();
        assert!(app.watch_prs(&bounds).await.is_empty());
        let before = task(&app, &id).await?;
        let next = app
            .create_task_with(new_task("Next"), Some(&before))
            .await?
            .id;
        let status = || async { task(&app, &next).await.ok().map(|task| task.status) };
        let act = |want: Activity| {
            let (app, id) = (app.clone(), id.clone());
            async move {
                wait_for("the activity", Duration::from_secs(30), || async {
                    activity(&app, &id).await == Some(want)
                })
                .await
            }
        };
        // A turn runs (held). An entry Claude Code hands on alone is queued, then the news.
        std::fs::remove_file(&may_end)?;
        app.send_input(&id, b"Go on with the next slice\r").await?;
        wait_for("the running turn", Duration::from_secs(30), || {
            screen_contains(&app, &id, "TURN HELD")
        })
        .await?;
        app.send_input(&id, b"/alone a notification of its own\r")
            .await?;
        wait_for("the queued entry", Duration::from_secs(30), || {
            screen_contains(&app, &id, "QUEUED: ")
        })
        .await?;
        let merged = DeliveryRecord {
            stage: DeliveryStage::MergedIntegration,
            updated_at: app.now(),
            ..open
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::PrOpen, &merged)
            .await?;
        let typed = app.watch_prs(&bounds).await;
        assert!(typed.iter().any(|(task, _)| task == &id), "{typed:?}");
        wait_for("the queued news", Duration::from_secs(30), || {
            screen_contains(&app, &id, "Yulius, about your pull request #7")
        })
        .await?;
        // The running turn ends; the hand-over is held.
        std::fs::remove_file(&may_hand)?;
        std::fs::write(&may_end, b"")?;
        act(Activity::Waiting).await?;
        app.advance_chains(&id).await;
        assert_eq!(
            status().await,
            Some(TaskStatus::Chained),
            "nothing is handed on yet"
        );
        // The entry that leaves alone gets its turn (held); the news still waits.
        std::fs::remove_file(&may_end)?;
        std::fs::write(&may_hand, b"")?;
        act(Activity::Working).await?;
        std::fs::remove_file(&may_hand)?;
        std::fs::write(&may_end, b"")?;
        act(Activity::Waiting).await?;
        // The watcher read what that turn wrote: the queue still holds the news.
        wait_for("the queue as read", Duration::from_secs(30), || async {
            app.claude_queue_holds(&id)
        })
        .await?;
        app.advance_chains(&id).await;
        assert_eq!(
            status().await,
            Some(TaskStatus::Chained),
            "another entry's turn is not the news' turn"
        );
        let predecessor = task(&app, &id).await?;
        assert!(app.task_summary(&predecessor).await?.yulius_wait);

        // The news' own turn runs and ends: the chain goes on (at the watch's next pass when
        // that turn's own end looked before its lines were read).
        std::fs::write(&may_hand, b"")?;
        wait_for(
            "the chained task's start",
            Duration::from_secs(60),
            || async {
                app.watch_prs(&bounds).await;
                status().await != Some(TaskStatus::Chained)
            },
        )
        .await?;
        assert!(
            screen_contains(&app, &id, "ECHO: Yulius, about your pull request #7").await,
            "the news' turn ran"
        );
        stop(&app, &next).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
