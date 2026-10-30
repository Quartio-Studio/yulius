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
use yulius::wiring::git::GitAdapter;
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
        escalation_ms,
        attach_late,
        sessions,
        None,
    )
    .await
}

/// A `/goal` prompt hook the test server handles only after `hold` and, within 30 s, once a Stop
/// that arrived meanwhile was handled: the first one once `armed`. `stop_handled` says that Stop
/// was handled first (the late path), `handled` when the prompt was.
#[derive(Clone)]
struct LateHook {
    hold: Duration,
    armed: Arc<std::sync::atomic::AtomicBool>,
    holding: Arc<std::sync::atomic::AtomicBool>,
    stop_handled: tokio::sync::watch::Sender<bool>,
    handled: tokio::sync::watch::Sender<bool>,
}

/// [`start_on`], and with `late_goal` the server sees a `/goal` prompt hook (a resumed session's
/// typed goal) only after its hold, then handles it to its end, as a loaded host does once the
/// hook client gave up waiting (SPEC-346).
#[allow(clippy::too_many_arguments)]
async fn start_holding(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    host: Option<Arc<dyn SessionHost>>,
    escalation_ms: Option<u64>,
    attach_late: bool,
    sessions: Option<u32>,
    late_goal: Option<LateHook>,
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
        git: Arc::new(GitAdapter::new("git")),
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
                        late.stop_handled.send_replace(true);
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
    let internal = yulius_server::internal_router(app.clone())
        .layer(goal_held)
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
/// for the user (the achieved goal no longer counts).
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
        stop(&app, &id).await
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
    let id = goal_task(app, sandbox, user, "Tidy the parser").await?;
    wait_for("the launch goal", Duration::from_secs(60), || {
        screen_contains(app, &id, "GOAL SET")
    })
    .await?;
    stop(app, &id).await?;
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
            None,
            false,
            None,
            Some(late.clone()),
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
                credential: Some(app.deps.crypto.seal(b"sk-ant-oat01-second-credential")?),
                status: AccountStatus::Ok,
                usage_json: None,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                email: None,
                email_checked_at: None,
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
        store
            .insert_account(&AccountRecord {
                id: AccountId::parse("a-claude-2")?,
                provider: Provider::Anthropic,
                label: "Claude spare".into(),
                credential: Some(app.deps.crypto.seal(b"sk-ant-oat01-spare-credential")?),
                status: AccountStatus::Ok,
                usage_json: None,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                email: None,
                email_checked_at: None,
            })
            .await?;
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
        let near = yulius_domain::usage::AccountUsage {
            primary: Some(yulius_domain::usage::UsageWindow {
                used_percent: Some(99.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(4_000_000_000_000)),
            }),
            ..yulius_domain::usage::AccountUsage::default()
        };
        let near = yulius_app::usage_json::to_json(&near);
        store
            .set_account_state(
                &AccountId::parse("a-claude")?,
                AccountStatus::Ok,
                Some(&near),
                None,
                app.now(),
            )
            .await?;
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
        wait_for("the switch", Duration::from_secs(60), || async {
            store
                .attempts(&id)
                .await
                .unwrap_or_default()
                .last()
                .is_some_and(|a| {
                    a.account_id.as_ref().map(AccountId::as_str) == Some("a-claude-2")
                        && !a.state.is_terminal()
                })
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
