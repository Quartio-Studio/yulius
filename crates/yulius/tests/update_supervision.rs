//! An instance update with real processes (SPEC-001 OPS-008, AC-OPS-8; SPEC-002 ADR-18):
//! supervisor → detached `yulius ptyhost` → fake agent shaped like Claude Code running its hooks
//! through the real `yulius hook` client against the internal listener. A prompt typed while the
//! update pauses the tasks waits in its `UserPromptSubmit` hook, survives the server going away
//! and a new server starting on the same port, and reaches the agent exactly as typed once the
//! new server releases it; the session is the same process, never respawned.

use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use support::TestResult;
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::updates::{FakeHelper, FakeSource, NEW, OLD, services};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, TaskMode, TaskOrigin, UserRecord,
};
use yulius_contracts::api::updates::StartUpdate;
use yulius_contracts::update::{HelperAction, HelperState};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::Activity;
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

type BoxError = Box<dyn std::error::Error>;

/// A server built from `build` with its internal listener on `port` (0 = any).
async fn start(
    data: &Path,
    fake: &Path,
    port: u16,
    build: &str,
    source: &Arc<FakeSource>,
    helper: &Arc<FakeHelper>,
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
            options: FakeAgentOptions {
                transcript: true,
                hooks: true,
                ..FakeAgentOptions::default()
            },
            yulius_exe: exe,
        }),
        git: Arc::new(GitAdapter::new("git")),
        system: Arc::new(support::FixedSystem),
        browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
        usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
    };
    let app = App::new(config, deps);
    app.set_updates(services(build, source, helper));
    app.restore_update_hold().await;
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

/// The superadmin, a sandbox cloned from a bare origin and a Claude account.
async fn seed(app: &App, data: &Path) -> Result<(SandboxId, UserRecord), BoxError> {
    let store = &app.deps.store;
    let user = UserRecord {
        id: UserId::parse("u-admin")?,
        github_login: None,
        github_id: None,
        display_name: "Superadmin".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    store.insert_user(&user).await?;
    let sandbox = SandboxRecord {
        id: SandboxId::parse("s-update")?,
        slug: "update".into(),
        repo_owner: "acme".into(),
        repo_name: "update".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 43100,
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

fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

fn screen(app: &App, id: &TaskId) -> String {
    app.runtime(id).hub.screen_text()
}

/// AC-OPS-8: the prompt typed while tasks pause for an update waits, through the restart, and
/// then reaches the same agent process exactly as typed; nothing else is typed into it.
#[test]
fn a_prompt_typed_during_an_update_reaches_the_same_agent_after_the_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let dir = tempfile::Builder::new().prefix("yu").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    let source = FakeSource::behind();
    let helper = FakeHelper::installed();

    // The old server: a task finishes its first turn; an update is built; tasks pause.
    let first = runtime()?;
    let (port, id, pid) = first.block_on(async {
        let (app, port) = start(&data, &fake, 0, OLD, &source, &helper).await?;
        let (sandbox, user) = seed(&app, &data).await?;
        let task = app
            .create_task_internal(NewTask {
                sandbox,
                created_by: user.id.clone(),
                prompt: "write the parser".into(),
                harness: Harness::ClaudeCode,
                title: Some("Parser".into()),
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
        let id = task.id;
        wait_for("the session", Duration::from_secs(60), || async {
            screen(&app, &id).contains("FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"write the parser\r").await?;
        wait_for("the first turn", Duration::from_secs(60), || async {
            screen(&app, &id).contains("ECHO: write the parser")
        })
        .await?;
        wait_for("the turn end", Duration::from_secs(30), || async {
            app.deps
                .store
                .task(&id)
                .await
                .ok()
                .flatten()
                .and_then(|t| t.activity)
                == Some(Activity::Waiting)
        })
        .await?;
        let attempt = app.deps.store.live_attempt(&id).await?.ok_or("no run")?;
        let ctx = AuthContext {
            user,
            csrf_token: None,
            session_hash: None,
        };
        app.check_updates_now(&ctx).await?;
        app.start_update(
            &ctx,
            &StartUpdate {
                commit: NEW.into(),
                confirm: true,
            },
        )
        .await?;
        let build = helper.last().ok_or("no build request")?;
        helper.report(&build, HelperState::Done, "built", None);
        let _ = app.update_round().await;

        // The person types a prompt: the agent's UserPromptSubmit hook is held.
        app.send_input(&id, b"now add tests\r").await?;
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !screen(&app, &id).contains("ECHO: now add tests"),
            "the prompt waits while tasks pause"
        );
        // Every task is at its pause point: the install is requested.
        let _ = app.update_round().await;
        let _ = app.update_round().await;
        let install = helper.last().ok_or("no install request")?;
        assert_eq!(install.action, HelperAction::Install);
        helper.report(&install, HelperState::Running, "stopping", None);
        Ok::<_, BoxError>((port, id, attempt.child_pid))
    })?;
    // The helper stops the old server: its listener is gone; the held hook keeps polling.
    first.shutdown_timeout(Duration::from_secs(5));
    std::thread::sleep(Duration::from_secs(3));

    // The new build starts on the same port and database and reattaches the session.
    let rt = runtime()?;
    rt.block_on(async {
        let (app, _) = start(&data, &fake, port, NEW, &source, &helper).await?;
        app.reconcile().await?;
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !screen(&app, &id).contains("ECHO: now add tests"),
            "still held until the helper's check passed"
        );
        let install = helper.last().ok_or("install")?;
        helper.report(&install, HelperState::Done, "succeeded", None);
        let _ = app.update_round().await;
        let _ = app.update_round().await;
        wait_for("the held prompt", Duration::from_secs(30), || async {
            screen(&app, &id).contains("ECHO: now add tests")
        })
        .await?;
        let text = screen(&app, &id);
        for word in ["update", "Update", "pause", "Pause", "restart", "Continue."] {
            assert!(!text.contains(word), "the agent saw {word:?}:\n{text}");
        }
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(attempts.len(), 1, "the same session, never respawned");
        assert_eq!(attempts[0].child_pid, pid);
        app.stop_task_now(&id).await?;
        wait_for("the stop", Duration::from_secs(30), || async {
            app.deps
                .store
                .task(&id)
                .await
                .ok()
                .flatten()
                .is_some_and(|t| !t.status.is_live())
        })
        .await
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
