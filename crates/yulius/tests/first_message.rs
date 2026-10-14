//! A session's first message and a person's typing (SPEC-068, SPEC-117): only keystrokes count as
//! typing, so a continued goal session leaves a person's draft alone; a goal task's `/goal` waits
//! behind a folder-trust dialog; a first message Yulius types with images is never lost by a
//! server restart or carried without its images after a capacity hold.
//!
//! End to end: supervisor → detached `yulius ptyhost` → fake agent running Claude Code's hooks
//! through the real `yulius hook` client against the internal listener; real worktrees; a
//! scripted usage probe for the subscription pool.

use crate::pool_supervision::{HOUR, ScriptedUsage, refresh, seed, usage};
use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::TestResult;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::hub::HubFrame;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{LaunchKind, PauseKind, TaskMode, TaskOrigin};
use yulius_app::ports::store::TaskStore;
use yulius_app::updates::UpdateRunRecord;
use yulius_contracts::api::tasks::ContinueTask;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState};
use yulius_domain::time::Timestamp;
use yulius_domain::update::RunState;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

type BoxError = Box<dyn std::error::Error>;

/// A server: the app and its internal listener on `port` (0 = any free port), as `yulius serve`
/// would start it, with `probe` reading the subscriptions' usage.
async fn start(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    probe: Arc<ScriptedUsage>,
) -> Result<(Arc<App>, u16), BoxError> {
    start_with(data, fake, agent, port, probe, |_| {}).await
}

/// [`start`] with `configure` applied to the app's configuration.
async fn start_with(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
    port: u16,
    probe: Arc<ScriptedUsage>,
    configure: impl FnOnce(&mut AppConfig),
) -> Result<(Arc<App>, u16), BoxError> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    let listener = socket.listen(64)?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let mut config = AppConfig::new(data.to_path_buf(), "https://yulius.test", &url, exe.clone());
    config.contained = false;
    configure(&mut config);
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
        usage: probe,
    };
    let app = App::new(config, deps);
    let internal = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
    Ok((app, port))
}

fn hooked() -> FakeAgentOptions {
    FakeAgentOptions {
        transcript: true,
        hooks: true,
        ..FakeAgentOptions::default()
    }
}

async fn new_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    prompt: &str,
    mode: TaskMode,
) -> Result<TaskId, BoxError> {
    Ok(app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: prompt.into(),
            harness: Harness::ClaudeCode,
            title: Some("First message".into()),
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
        .await?
        .id)
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

fn screen(app: &App, id: &TaskId) -> String {
    app.runtime(id).hub.screen_text()
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

/// The notices sent to the task's viewers so far on `watch`.
fn notices(watch: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let HubFrame::Notice(text) = frame {
            out.push(text);
        }
    }
    out
}

async fn stop(app: &Arc<App>, id: &TaskId) -> TestResult {
    app.stop_task_now(id).await?;
    wait_for("the stop", Duration::from_secs(30), || async {
        TaskStore::task(app.deps.store.as_ref(), id)
            .await
            .ok()
            .flatten()
            .is_some_and(|t| !t.status.is_live())
    })
    .await
}

fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

fn data_dir() -> Result<(tempfile::TempDir, PathBuf), BoxError> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yf").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// A goal task that got its goal (its launch prompt, SPEC-117), then moved by a person to
/// another subscription: the new session opens with a turn on "Continue the task …" of 12 s
/// (`--opening-turn-ms`), after which its goal is typed again (ONE-007). Returns the task, its
/// creator's context and a writer's terminal on the new session during that turn.
async fn goal_in_a_continued_turn(
    app: &Arc<App>,
    data: &Path,
) -> Result<(TaskId, AuthContext, yulius_app::terminal::TerminalAttach), BoxError> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    let id = new_task(app, &sandbox, &user, "Make the header blue", TaskMode::Goal).await?;
    wait_for("the first goal", Duration::from_secs(60), || async {
        screen(app, &id).contains("GOAL SET")
    })
    .await?;
    let ctx = AuthContext {
        user: app.deps.store.user(&user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
    };
    app.switch_task(
        &ctx,
        id.as_str(),
        &ContinueTask {
            account_id: Some("b-second".into()),
        },
    )
    .await?;
    // The new run's screen replaced the first one's: its opening turn runs, no goal yet.
    wait_for("the continued turn", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
        attempts.len() == 2
            && attempts[1].state == AttemptState::Live
            && screen(app, &id).contains("WORKING")
            && !screen(app, &id).contains("GOAL SET")
    })
    .await?;
    let attach = app
        .terminal_open(&ctx, id.as_str(), Some((120, 40)))
        .await?;
    Ok((id, ctx, attach))
}

fn opening_turn() -> FakeAgentOptions {
    FakeAgentOptions {
        // Long enough that a busy test machine still types into the running turn.
        opening_turn_ms: Some(12_000),
        ..hooked()
    }
}

/// SPEC-068 TYP-002, AC-TYP-2: clicking into and out of the terminal during a continued goal
/// session's opening turn (focus and mouse reports through `terminal_input`) is not typing: the
/// goal is still set, once, after that turn.
#[test]
fn focus_and_mouse_reports_do_not_keep_the_goal_out() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        for report in [
            b"\x1b[I".as_slice(),
            b"\x1b[<0;10;5M",
            b"\x1b[<0;10;5m",
            b"\x1b[M #%",
            b"\x1b[O",
        ] {
            app.terminal_input(&ctx, &attach, report, false).await?;
        }
        wait_for("the goal", Duration::from_secs(90), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let shown = screen(&app, &id);
        assert_eq!(shown.matches("GOAL SET").count(), 1, "{shown}");
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-068 TYP-003, AC-TYP-3 (#252): a person's draft typed during a continued goal session's
/// opening turn does not get `/goal` appended; the task is flagged and told why.
#[test]
fn a_draft_typed_during_the_opening_turn_is_left_alone() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"my own draft", false)
            .await?;
        wait_for("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        let shown = screen(&app, &id);
        assert!(!shown.contains("/goal"), "the goal was typed:\n{shown}");
        assert!(!shown.contains("GOAL SET"), "{shown}");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-106 OTR-002 (#425): Claude Code opens on its folder-trust prompt and a person answers it
/// before the goal. Yulius types nothing into the prompt, the answer is not a draft, and the goal
/// is set once the prompt is gone, with no flag.
#[test]
fn a_trust_prompt_answered_before_the_goal_keeps_the_goal() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start(&data, &fake, agent, 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        wait_for("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        // Longer than the goal's quiet time: a Yulius that typed into the prompt would have now.
        tokio::time::sleep(Duration::from_secs(3)).await;
        app.terminal_input(&ctx, &attach, b"2\r", false).await?;
        wait_for("the goal", Duration::from_secs(90), || async {
            screen(&app, &id).contains("GOAL SET")
        })
        .await?;
        let shown = screen(&app, &id);
        let answer = shown
            .lines()
            .find(|line| line.starts_with("TRUST ANSWERED:"))
            .map(str::trim_end);
        assert_eq!(answer, Some("TRUST ANSWERED: 2"), "{shown}");
        assert_eq!(shown.matches("GOAL SET").count(), 1, "{shown}");
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-106 OTR-002 (#425): a trust prompt nobody answers gets nothing typed into it; once the
/// time to answer it is over the task is flagged and says why.
#[test]
fn an_unanswered_trust_prompt_gets_nothing_typed_and_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            config.trust_answer_ms = 3_000;
            config.goal_ready_ms = 2_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make the header blue",
            TaskMode::Goal,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        wait_for("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(shown.contains("Yes, I trust this folder"), "{shown}");
        assert!(
            !shown.contains("TRUST ANSWERED"),
            "something answered it:\n{shown}"
        );
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("asks whether to trust this folder. Answer")),
            "{said:?}"
        );
        assert!(
            said.iter()
                .any(|n| n.contains("still asks whether to trust this folder")
                    && n.ends_with("the task starts by itself once it is answered.")),
            "{said:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-106 OTR-002: the trust dialog's words in a session that has started (here a person's
/// draft) are no trust prompt: the draft is left alone and the task flagged, as in #252.
#[test]
fn trust_words_after_the_session_started_are_no_trust_prompt() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, opening_turn(), 0, probe).await?;
        let (id, ctx, attach) = goal_in_a_continued_turn(&app, &data).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"Yes, I trust this folder", false)
            .await?;
        wait_for("the flag", Duration::from_secs(60), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(said.iter().any(|n| n.contains("no goal yet")), "{said:?}");
        assert!(
            !said.iter().any(|n| n.contains("asks whether to trust")),
            "{said:?}"
        );
        assert!(!screen(&app, &id).contains("/goal"));
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-117 ONE-001, TSK-010: a `/goal` launch prompt that Claude Code never takes (no goal
/// recorded within `launch_goal_ms` once the session reads input) flags the task and says so, as
/// a typed goal that was never taken does.
#[test]
fn a_launch_goal_never_taken_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            ignores_goal_prompt: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            // 75 s in production (the typed goal's submit waits): shortened (SPEC-082 TST-001).
            config.launch_goal_ms = 3_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Tidy the parser", TaskMode::Goal).await?;
        let mut watch = app.runtime(&id).hub.watch();
        wait_for("the flag", Duration::from_secs(45), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Claude Code did not take the task's goal")),
            "{said:?}"
        );
        assert!(
            !screen(&app, &id).contains("/goal"),
            "{}",
            screen(&app, &id)
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-117 ONE-006, AC-ONE-3: keys a person types while Yulius types a prompt task's first
/// message with an image are not written into it, and the person is told to type once it is sent;
/// after it, typing reaches the session again. Without a SessionStart hook the session reads input
/// 10 s after its start (half of `goal_ready_ms`), so the keys surely come before the message.
#[test]
fn keys_typed_while_yulius_types_the_first_message_wait() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            no_session_start: true,
            ..hooked()
        };
        let (app, _) = start_with(&data, &fake, agent, 0, probe, |config| {
            config.goal_ready_ms = 20_000;
        })
        .await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the session", Duration::from_secs(60), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.terminal_input(&ctx, &attach, b"my draft", false)
            .await?;
        app.terminal_input(&ctx, &attach, b" more", false).await?;
        wait_for("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text (SPEC-124).
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        let shown = screen(&app, &id);
        assert!(!shown.contains("my draft"), "{shown}");
        assert!(!shown.contains("more"), "{shown}");
        let said = notices(&mut watch);
        let told = said
            .iter()
            .filter(|n| n.contains("Yulius is typing this task's first message"))
            .count();
        assert_eq!(told, 1, "{said:?}");
        // Sent: the person types into the session again.
        app.terminal_input(&ctx, &attach, b"after\r", false).await?;
        wait_for("the person's message", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: after")
        })
        .await?;
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-117 ONE-006: the folder-trust dialog in front of a prompt task's typed first message is
/// answered by a person: those keys pass (only a person answers it), and the message with its
/// image is typed once the dialog is gone.
#[test]
fn a_trust_prompt_before_a_typed_first_message_takes_the_answer() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            trust_prompt: true,
            ..hooked()
        };
        let (app, _) = start(&data, &fake, agent, 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        let mut watch = app.runtime(&id).hub.watch();
        wait_for("the trust prompt", Duration::from_secs(60), || async {
            screen(&app, &id).contains("Yes, I trust this folder")
        })
        .await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let attach = app
            .terminal_open(&ctx, id.as_str(), Some((120, 40)))
            .await?;
        app.terminal_input(&ctx, &attach, b"2\r", false).await?;
        wait_for("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text (SPEC-124).
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        let shown = screen(&app, &id);
        let answer = shown
            .lines()
            .find(|line| line.starts_with("TRUST ANSWERED:"))
            .map(str::trim_end);
        assert_eq!(answer, Some("TRUST ANSWERED: 2"), "{shown}");
        let said = notices(&mut watch);
        assert!(
            !said.iter().any(|n| n.contains("Yulius is typing")),
            "{said:?}"
        );
        app.terminal_leave(&attach).await;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-068 OPN-002, AC-OPN-1 (#266): the server restarts while the first message of a task with
/// an image is being typed (the image pasted, no Enter yet). The new server reattaches the run
/// and flags the task with a notice; nothing is submitted for it.
#[test]
fn a_first_message_cut_by_a_restart_flags_the_task() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, hooked(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        // The image's paste reached the terminal (its echo); the text and Enter come after the
        // paste settled, at least 500 ms later (`PASTE_QUIET`). A test that missed that window
        // would fail (the message echoed), never pass falsely.
        wait_for("the image's paste", Duration::from_secs(60), || async {
            screen(&app, &id).contains("/srv/shot.png")
        })
        .await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        wait_for("the flag", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("first message was never sent")),
            "{said:?}"
        );
        let shown = screen(&app, &id);
        assert!(
            !shown.contains("ECHO:"),
            "something was submitted:\n{shown}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// Phase one of a restart during an instance update: a prompt task with an image whose first
/// message's submit reached Yulius, which held it for the update. Returns the data directory's
/// guard and path, the internal port and the task; the first server is gone.
fn first_message_held_by_an_update(
    fake: &Path,
) -> Result<(tempfile::TempDir, PathBuf, u16, TaskId), BoxError> {
    let (dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, fake, hooked(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(
            &app,
            &sandbox,
            &user,
            "Make it blue\n\n@/srv/shot.png",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the run", Duration::from_secs(30), || async {
            matches!(app.deps.store.live_attempt(&id).await, Ok(Some(_)))
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        // An update drains the instance before the first message is typed: every submit waits.
        let now = Timestamp(1);
        app.deps
            .store
            .insert_update_run(&UpdateRunRecord {
                id: "update-1".into(),
                target_commit: "b".repeat(40),
                from_commit: None,
                summary: "next".into(),
                state: RunState::Draining,
                outcome: None,
                error: None,
                requested_by: "admin".into(),
                created_at: now,
                state_since: now,
                finished_at: None,
            })
            .await?;
        app.restore_update_hold().await;
        let marker = Path::new(&attempt.run_dir).join("opening-pending");
        wait_for("the held submit", Duration::from_secs(60), || async {
            std::fs::read(&marker).is_ok_and(|m| !m.is_empty())
        })
        .await?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));
    Ok((dir, data, port, id))
}

/// SPEC-068 OPN-002: the first message's submit reached Yulius but an instance update held it
/// when the server restarted. The new server does not flag the task: the held submit goes on and
/// the agent gets the message once.
#[test]
fn a_first_message_held_by_an_update_is_not_flagged_after_the_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data, port, id) = first_message_held_by_an_update(&fake)?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        wait_for("the first message", Duration::from_secs(60), || async {
            {
                let shown = screen(&app, &id);
                // The image is pasted where it sits: after the text (SPEC-124).
                shown
                    .find("ECHO: Make it blue")
                    .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
            }
        })
        .await?;
        assert_ne!(activity(&app, &id).await, Some(Activity::NeedsAttention));
        let said = notices(&mut watch);
        assert!(!said.iter().any(|n| n.contains("never sent")), "{said:?}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A hooked agent whose goal gets no verdict: the task stays on its run (not Done).
fn unjudged() -> FakeAgentOptions {
    FakeAgentOptions {
        goal_verdict: Some("none"),
        ..hooked()
    }
}

/// ADR-0095: a goal run started by an older server typed a first message before its goal; that
/// message was submitted and held when the server restarted into this version, which types no
/// goal after it. The task is flagged and says why, instead of going on without a goal.
#[test]
fn a_goal_run_of_an_older_server_held_across_the_update_is_flagged() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let first = runtime()?;
    let (port, id) = first.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, port) = start(&data, &fake, unjudged(), 0, probe).await?;
        let (sandbox, user) = seed(&app, &data, &["a-first"]).await?;
        let id = new_task(&app, &sandbox, &user, "Make it blue", TaskMode::Goal).await?;
        // The goal turn ended (its Stop clears a pending first message, so it comes first).
        wait_for("the goal turn's end", Duration::from_secs(60), || async {
            screen(&app, &id).contains("GOAL SET")
                && !matches!(activity(&app, &id).await, Some(Activity::Working) | None)
        })
        .await?;
        // What the older server left in the run directory: a first message submitted, held.
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        std::fs::write(
            Path::new(&attempt.run_dir).join("opening-pending"),
            b"entered",
        )?;
        Ok::<_, BoxError>((port, id))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, unjudged(), port, probe).await?;
        let mut watch = app.runtime(&id).hub.watch();
        app.reconcile().await?;
        wait_for("the flag", Duration::from_secs(20), || async {
            activity(&app, &id).await == Some(Activity::NeedsAttention)
        })
        .await?;
        let said = notices(&mut watch);
        assert!(
            said.iter()
                .any(|n| n.contains("Yulius restarted before it set this task's goal")),
            "{said:?}"
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// A task with an image whose first message is held for capacity: a-first runs out once the run
/// started; b-second, the only other subscription, is full too. Returns the task and its creator.
async fn first_message_held_for_capacity(
    app: &Arc<App>,
    data: &Path,
    probe: &ScriptedUsage,
) -> Result<(TaskId, UserId), BoxError> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    probe.set("a-first", usage(10.0, 4 * HOUR, 30.0, 100));
    probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "a-first").await?;
    refresh(app, &user, "b-second").await?;
    let id = new_task(
        app,
        &sandbox,
        &user,
        "Make it blue\n\n@/srv/shot.png",
        TaskMode::Prompt,
    )
    .await?;
    wait_for("the run on a-first", Duration::from_secs(30), || async {
        matches!(app.deps.store.live_attempt(&id).await, Ok(Some(_)))
    })
    .await?;
    // a-first runs out before the first message is typed: it is held at its submit.
    probe.set("a-first", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "a-first").await?;
    wait_for(
        "the held first message",
        Duration::from_secs(60),
        || async {
            matches!(
                app.deps.store.open_pause(&id).await,
                Ok(Some(pause)) if pause.kind == PauseKind::Prompt
            )
        },
    )
    .await?;
    // b-second's window resets.
    probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    Ok((id, user))
}

/// The task moved to b-second as a fresh session that typed the task's own opening there, the
/// image pasted, not the held prompt's text.
async fn typed_again_on_b_second(app: &Arc<App>, id: &TaskId) -> TestResult {
    wait_for("the run on b-second", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(id).await.unwrap_or_default();
        attempts.len() == 2 && attempts[1].state == AttemptState::Live
    })
    .await?;
    wait_for("the first message", Duration::from_secs(60), || async {
        {
            let shown = screen(app, id);
            // The image is pasted where it sits: after the text (SPEC-124).
            shown
                .find("ECHO: Make it blue")
                .is_some_and(|text| shown.find("[Image #1]").is_some_and(|image| image > text))
        }
    })
    .await?;
    let attempts = app.deps.store.attempts(id).await?;
    let second = &attempts[1];
    assert_eq!(
        second.account_id.as_ref().map(AccountId::as_str),
        Some("b-second")
    );
    assert_eq!(second.launch_kind, LaunchKind::New, "a fresh session");
    // The new session got the image pasted (its echo), not the held prompt's text.
    assert!(
        screen(app, id).contains("/srv/shot.png"),
        "{}",
        screen(app, id)
    );
    let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
    assert!(launch.contains("prompt: None"), "{launch}");
    Ok(())
}

/// SPEC-068 OPN-003, AC-OPN-2 (#266): the first message of a task with an image is held for
/// capacity and resumed on another subscription: a fresh session there gets the task's own
/// opening, the image pasted, not the held prompt's text.
#[test]
fn a_held_first_message_resumes_elsewhere_with_its_image() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), 0, probe.clone()).await?;
        let (id, _) = first_message_held_for_capacity(&app, &data, &probe).await?;
        let _ = app.resume_round().await;
        typed_again_on_b_second(&app, &id).await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-068 OPN-003: a person switches the account of a task whose first message is held: the
/// same fresh session on the chosen account, not "continue where you left off" in an empty one.
#[test]
fn a_manual_switch_of_a_held_first_message_keeps_its_image() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let (app, _) = start(&data, &fake, hooked(), 0, probe.clone()).await?;
        let (id, user) = first_message_held_for_capacity(&app, &data, &probe).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        app.switch_task(
            &ctx,
            id.as_str(),
            &ContinueTask {
                account_id: Some("b-second".into()),
            },
        )
        .await?;
        typed_again_on_b_second(&app, &id).await?;
        assert!(
            !screen(&app, &id).contains("Continue the task"),
            "{}",
            screen(&app, &id)
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
