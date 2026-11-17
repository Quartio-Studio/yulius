//! Antigravity as a third harness with real processes (SPEC-505): supervisor → detached
//! `yulius ptyhost` → fake agent writing Antigravity CLI 1.5.10 shaped transcripts and running its
//! hooks through `yulius hook antigravity`. An Antigravity task runs on a Google account in its own
//! home, its conversation is learnt from its SessionStart and watched, it moves to another Google
//! account at a limit by resuming the conversation, a goal task gets its request as its first
//! message, and a refreshed sign-in is kept. No test calls Google.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::supervision::{
    build_full, runtime, screen_contains, seed_records, seed_repo, task, wait_for,
};
use crate::support;
use crate::support::TestResult;
use crate::support::agents::FakeAgentOptions;
use yulius::wiring::clock::SystemClock;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::App;
use yulius_app::context::{AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, LaunchKind, TaskMode, TaskOrigin};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_harness::login::{antigravity_api_key_credential, antigravity_sign_in_credential};
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

const API_KEY: &str = "SYNTHETIC-gemini-api-key-0123456789";

async fn add_google_account(
    app: &App,
    id: &str,
    credential: &[u8],
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Google,
            label: format!("Antigravity {id}"),
            credential: Some(app.deps.crypto.seal(credential)?),
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
    Ok(account)
}

fn api_key() -> Result<Vec<u8>, String> {
    antigravity_api_key_credential(API_KEY).ok_or_else(|| "an API key envelope".to_owned())
}

async fn new_agy_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    mode: TaskMode,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::Antigravity,
            title: Some(title.into()),
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
        .await?;
    Ok(record.id)
}

/// An app whose agents' hooks reach it: its internal listener serves them, as `yulius serve`'s
/// does, so the fake agent's hooks go through `yulius hook antigravity` to it.
async fn build_hooked(
    data: &Path,
    fake: &Path,
    agent: FakeAgentOptions,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
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
        harness: Arc::new(support::agents::FakeAgentHarness {
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
    let internal = yulius_server::internal_router(app.clone())
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
    Ok(app)
}

fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    // A short base: run sockets live at <data>/runs/<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yl").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// SPEC-505 AGY-002, AGY-003, AGY-004, AGY-005, AGY-006, AC-1: an Antigravity task runs on a
/// Google account (never the Claude one) in `<sandbox>/antigravity/<account>`, whose settings,
/// hooks and MCP configuration are private and whose conversations are the sandbox's; its
/// conversation is learnt from its SessionStart (sent through `yulius hook antigravity`); its
/// turns and tokens are read from the transcript; it runs Gemini 4 Carbon by default.
#[test]
fn an_antigravity_task_runs_in_its_home() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            report_env: vec!["HOME".into()],
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        let account = add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity echo", TaskMode::Prompt).await?;
        wait_for("the fake agent banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        assert_eq!(attempt.harness, Harness::Antigravity);
        assert_eq!(
            attempt.account_id.as_ref(),
            Some(&account),
            "a Google account"
        );
        assert_eq!(attempt.model.as_deref(), Some("gemini-4-carbon"));
        let launch = std::fs::read_to_string(Path::new(&attempt.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("model=None"),
            "the task names none: {launch}"
        );

        let sandbox_dir = data.join("sandboxes").join(sandbox.as_str());
        // The run's own home, in its run directory: no agent of the sandbox could have made it
        // a link before a contained run binds its `.gemini` (AGY-003).
        let agy_home = Path::new(&attempt.run_dir).join("antigravity-home");
        let gemini = agy_home.join(".gemini");
        assert!(!gemini.starts_with(&sandbox_dir), "{gemini:?}");
        // This run is uncontained: its HOME is that home.
        let home_len = agy_home.as_os_str().len();
        assert!(
            screen_contains(&app, &id, &format!("ENV HOME=set len={home_len}")).await,
            "{}",
            app.runtime(&id).hub.screen_text()
        );
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        for file in [
            "antigravity-cli/settings.json",
            "antigravity-cli/cache/onboarding.json",
            "config/hooks.json",
            "config/mcp_config.json",
        ] {
            let mode = std::fs::metadata(gemini.join(file))?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file}");
        }
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            gemini.join("antigravity-cli/settings.json"),
        )?)?;
        assert_eq!(settings["modelProvider"], "gemini");
        assert_eq!(settings["trustedWorkspaces"][0], worktree.as_str());
        let hooks = std::fs::read_to_string(gemini.join("config/hooks.json"))?;
        assert!(hooks.contains("hook antigravity Stop"), "{hooks}");
        assert!(!hooks.contains(API_KEY) && !settings.to_string().contains(API_KEY));
        for shared in ["brain", "conversations"] {
            assert_eq!(
                std::fs::read_link(gemini.join("antigravity-cli").join(shared))?,
                sandbox_dir.join("antigravity/shared").join(shared)
            );
        }

        // The conversation is the one its SessionStart named.
        let session = attempt.id.as_str().to_owned();
        wait_for(
            "the conversation learnt from SessionStart",
            Duration::from_secs(30),
            || async {
                app.deps
                    .store
                    .attempt(&attempt.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|a| a.provider_session_id)
                    .as_deref()
                    == Some(session.as_str())
            },
        )
        .await?;
        app.send_input(&id, b"hello antigravity\r").await?;
        wait_for("the echo", Duration::from_secs(10), || {
            screen_contains(&app, &id, "ECHO: hello antigravity")
        })
        .await?;
        let transcript = sandbox_dir
            .join("antigravity/shared/brain")
            .join(&session)
            .join(".system_generated/logs/transcript.jsonl");
        wait_for("the transcript's turn", Duration::from_secs(10), || async {
            std::fs::read_to_string(&transcript).is_ok_and(|t| t.contains("PLANNER_RESPONSE"))
        })
        .await?;
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-505 AGY-004, AGY-009, AC-2: a plan's quota in the transcript is a limit of the
/// subscription: the task resumes the same conversation on another Google account (asking agy
/// to go on), the first account is limited, and the Claude account is untouched.
#[test]
fn an_antigravity_task_at_its_limit_moves_to_another_google_account() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        let one = add_google_account(&app, "a-google-1", &api_key()?).await?;
        let two = add_google_account(&app, "a-google-2", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity rotation",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        app.send_input(&id, b"again\r").await?;
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
        let accounts = [Some(one.clone()), Some(two.clone())];
        assert!(accounts.contains(&first.account_id), "{first:?}");
        assert!(accounts.contains(&second.account_id), "{second:?}");
        assert_ne!(
            first.account_id, second.account_id,
            "another Google account"
        );
        assert_eq!(
            second.harness,
            Harness::Antigravity,
            "the task keeps its harness"
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert!(first.provider_session_id.is_some());
        assert_eq!(
            first.provider_session_id, second.provider_session_id,
            "same conversation"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains("Resume {") && launch.contains("continue_interrupted: true"),
            "{launch}"
        );
        let limited = app
            .deps
            .store
            .account(first.account_id.as_ref().ok_or("first account")?)
            .await?
            .ok_or("the first account")?;
        assert_eq!(limited.status, AccountStatus::Limited);
        let claude = app
            .deps
            .store
            .account(&AccountId::parse("a-claude")?)
            .await?
            .ok_or("claude")?;
        assert_eq!(
            claude.status,
            AccountStatus::Ok,
            "the Claude account is untouched"
        );
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-505: an Antigravity goal task gets its whole request as its first message, never a
/// `/goal` Yulius types (the CLI's own `/goal` is its own).
#[test]
fn an_antigravity_goal_task_gets_its_request_as_its_first_message() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(&app, &sandbox, &user, "Antigravity goal", TaskMode::Goal).await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let run_dir = Path::new(&attempt.run_dir);
        let launch = std::fs::read_to_string(run_dir.join("fake-launch.txt"))?;
        assert!(
            launch.contains("Antigravity goal: answer with an echo"),
            "{launch}"
        );
        assert!(!launch.contains("/goal"), "{launch}");
        // No `/goal` is typed later either, and its Stop is not held as a goal's.
        for marker in ["launch-goal-pending", "typed-goal-pending"] {
            assert!(!run_dir.join(marker).exists(), "{marker}");
        }
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let detail = app.task_detail(&ctx, id.as_str()).await?;
        assert_eq!(
            detail.model, "gemini-4-carbon",
            "Gemini, not the Claude default"
        );
        assert!(detail.model_default);
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-505 AGY-008, AC-5: a Google sign-in's token file is written into its home for the run;
/// when the run ends, a token file the CLI refreshed replaces the stored sign-in, and the file
/// leaves the sandbox once no run of the account is left.
#[test]
fn a_refreshed_antigravity_sign_in_is_kept_and_leaves_the_sandbox() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_full(&data, Some(&fake), FakeAgentOptions::default(), 8)?;
        let (sandbox, user) = seed_records(&app).await?;
        let stored = antigravity_sign_in_credential(b"SYNTHETIC-token-old").ok_or("envelope")?;
        let account = add_google_account(&app, "a-google", &stored).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity sign-in",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        let attempt = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("no live attempt")?;
        let token = Path::new(&attempt.run_dir)
            .join("antigravity-home/.gemini/antigravity-cli/antigravity-oauth-token");
        assert_eq!(std::fs::read(&token)?, b"SYNTHETIC-token-old");
        assert_eq!(
            std::fs::metadata(&token)?.permissions().mode() & 0o777,
            0o600
        );
        // The CLI refreshes its sign-in during the run.
        std::fs::write(&token, b"SYNTHETIC-token-new")?;
        app.send_input(&id, b"/exit\r").await?;
        wait_for("the run's end", Duration::from_secs(30), || async {
            task(&app, &id).await.is_ok_and(|t| !t.status.is_live())
        })
        .await?;
        let expected = antigravity_sign_in_credential(b"SYNTHETIC-token-new").ok_or("envelope")?;
        wait_for("the refresh kept", Duration::from_secs(10), || async {
            let Ok(Some(record)) = app.deps.store.account(&account).await else {
                return false;
            };
            record
                .credential
                .and_then(|sealed| app.deps.crypto.open(&sealed).ok())
                .is_some_and(|plain| plain == expected)
        })
        .await?;
        wait_for("the file gone", Duration::from_secs(10), || async {
            !token.exists()
        })
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// SPEC-505 AGY-004, AGY-006: a turn whose model API fails for good writes an error step per
/// attempt (no end of the turn: agy goes on) and then its Stop with the error, which Yulius settles
/// without a gate: the task leaves `working` and waits for its person.
#[test]
fn a_failed_antigravity_turn_leaves_its_task_waiting() -> TestResult {
    use yulius_domain::task::Activity;
    let fake = locate_fake_agent().ok_or("fake-agent is not built")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let agent = FakeAgentOptions {
            transcript: true,
            hooks: true,
            ..FakeAgentOptions::default()
        };
        let app = build_hooked(&data, &fake, agent).await?;
        let (sandbox, user) = seed_records(&app).await?;
        add_google_account(&app, "a-google", &api_key()?).await?;
        seed_repo(&data, &sandbox)?;
        let id = new_agy_task(
            &app,
            &sandbox,
            &user,
            "Antigravity failure",
            TaskMode::Prompt,
        )
        .await?;
        wait_for("the banner", Duration::from_secs(30), || {
            screen_contains(&app, &id, "FAKE-AGENT READY")
        })
        .await?;
        // A code change the gate would block a clean Stop for: a failed turn's is never held.
        let worktree = task(&app, &id).await?.worktree_path.ok_or("worktree")?;
        std::fs::write(Path::new(&worktree).join("api.rs"), "fn main() {}\n")?;
        app.send_input(&id, b"/apierror\r").await?;
        wait_for("the failed turn", Duration::from_secs(20), || {
            screen_contains(&app, &id, "API ERROR")
        })
        .await?;
        wait_for("the task waiting", Duration::from_secs(20), || async {
            task(&app, &id).await.is_ok_and(|t| {
                t.status == TaskStatus::Running && t.activity == Some(Activity::Waiting)
            })
        })
        .await?;
        assert_eq!(task(&app, &id).await?.gate_blocks, 0, "no gate block");
        support::stop_tasks(&app, &[&id]).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
