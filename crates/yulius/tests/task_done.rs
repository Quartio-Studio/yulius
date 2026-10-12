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
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, TaskMode, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::harness::HarnessPort;
use yulius_app::ports::store::TaskStore;
use yulius_contracts::events::UiEvent;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, TaskStatus};
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
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    let listener = socket.listen(64)?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
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
    app.stop_task_now(id).await?;
    wait_for("the stop", Duration::from_secs(30), || async {
        task(app, id).await.is_ok_and(|t| !t.status.is_live())
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
        // SPEC-025 ATT-003: without attached files the session starts with no first message.
        let launch = first_launch(&app, &id).await?;
        assert!(launch.contains("prompt: None"), "{launch}");
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

/// TSK-010: a goal typed into a TUI that drops the first submit is typed again until Claude
/// Code takes it, so the task starts instead of showing the goal unsubmitted.
#[test]
fn a_goal_the_tui_dropped_is_submitted_again() -> TestResult {
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
        let id = goal_task(&app, &sandbox, &user, "Tidy the parser").await?;
        wait_for("the goal to start", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        wait_for("Done", Duration::from_secs(30), || async {
            activity(&app, &id).await == Some(Activity::Done)
        })
        .await?;
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-011: a goal task starts from its request file (the request, its attached files and the
/// definition of done) and a `/goal` that names it, typed once the session started.
#[test]
fn a_goal_task_starts_from_its_request_file() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            session_start: true,
            // The turn on the typed first message (the attached image) is silent for longer
            // than the quiet time after which a goal is otherwise typed (SPEC-030 PIMG-003).
            echo_turn_ms: Some(4_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let record = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                prompt: "Make the header blue\n\n@/srv/shots/header.png".into(),
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
        // SPEC-030 PIMG-002: an attached image is pasted into the session's first message, so
        // Claude Code attaches it natively (`[Image #1]`), not mentioned in the command line;
        // the goal is still typed after that message's turn.
        let launch = first_launch(&app, &id).await?;
        assert!(launch.contains("prompt: None"), "{launch}");
        let screen = app.runtime(&id).hub.screen_text();
        let pasted = screen
            .find("ECHO: [Image #1] The files attached to this task.")
            .ok_or_else(|| screen.clone())?;
        let typed = screen.find("/goal ").ok_or_else(|| screen.clone())?;
        assert!(
            pasted < typed,
            "the goal was typed during the turn on the first message:\n{screen}"
        );
        assert!(!screen.contains("STOP BLOCKED"), "{screen}");
        assert_eq!(screen.matches("GOAL SET").count(), 1, "{screen}");
        assert!(request.contains("## Definition of done"), "{request}");
        // The goal names the file (a request with attached files is never inlined).
        assert!(
            screen_contains(&app, &id, "Everything requested in").await,
            "{}",
            app.runtime(&id).hub.screen_text()
        );
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-025 ATT-003: a goal task with attached files opens with a turn on them (its first message);
/// the goal is typed once that turn ended (its Stop hook), not into the middle of it, and once.
#[test]
fn the_goal_is_typed_after_the_turn_on_the_attached_files() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            session_start: true,
            // Silent for longer than the quiet time after which a goal is otherwise typed.
            opening_turn_ms: Some(6_000),
            ..FakeAgentOptions::default()
        };
        let (app, _) = start(&data, &fake, agent, 0).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let id = app
            .create_task_internal(NewTask {
                sandbox: sandbox.clone(),
                created_by: user.clone(),
                // Not an image: mentioned in the command-line prompt (SPEC-025, SPEC-030).
                prompt: "Make the header blue\n\n@/srv/notes.txt".into(),
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
            })
            .await?
            .id;
        wait_for("the goal to start", Duration::from_secs(90), || {
            screen_contains(&app, &id, "GOAL SET")
        })
        .await?;
        let screen = app.runtime(&id).hub.screen_text();
        let done = screen.find("DONE opening").ok_or_else(|| screen.clone())?;
        let typed = screen.find("/goal ").ok_or_else(|| screen.clone())?;
        assert!(
            done < typed,
            "the goal was typed during the opening turn:\n{screen}"
        );
        assert_eq!(screen.matches("GOAL SET").count(), 1, "{screen}");
        stop(&app, &id).await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-030 PIMG-002: a prompt-mode task with an attached image starts with no command-line
/// prompt; its first message is typed into the TUI with the image pasted (`[Image #1]`, as
/// Claude Code attaches a pasted image) and the other files mentioned.
#[test]
fn an_attached_image_is_pasted_into_the_first_message() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            session_start: true,
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
            })
            .await?
            .id;
        wait_for("the first message", Duration::from_secs(90), || {
            screen_contains(&app, &id, "ECHO: [Image #1] Make it blue")
        })
        .await?;
        let launch = first_launch(&app, &id).await?;
        assert!(launch.contains("prompt: None"), "{launch}");
        let screen = app.runtime(&id).hub.screen_text();
        assert!(screen.contains("@\"/srv/notes.txt\""), "{screen}");
        assert!(!screen.contains("@\"/srv/shot.png\""), "{screen}");
        assert_eq!(screen.matches("ECHO: [Image #1]").count(), 1, "{screen}");
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
