//! The subscription pool with real processes: supervisor → detached `yulius ptyhost` → fake
//! agent, a scripted usage probe and real git worktrees (SPEC-001 ROT-004/010, AC-ROT-9..11,
//! AC-ROT-14; SPEC-002 ADR-17). The fake agent has no hooks, so its turn boundary is the end of
//! a turn in its transcript; hook holds are covered by `subscription_pool.rs`. MCP servers reach
//! every run on every subscription (MCP-004, AC-MCP-3).

use crate::support;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{FakeGitHub, TestResult};
use yulius::wiring::clock::SystemClock;
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, LaunchKind, PauseKind, SandboxRecord, Secret, TaskOrigin,
    TaskRecord, UserRecord,
};
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_contracts::api::mcp::McpServerInput;
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

const HOUR: i64 = 3_600_000;

/// A usage probe whose answers the test sets per credential (account id = credential here).
#[derive(Default)]
struct ScriptedUsage(Mutex<HashMap<String, AccountUsage>>);

impl ScriptedUsage {
    fn set(&self, account: &str, usage: AccountUsage) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(account.to_owned(), usage);
        }
    }
}

#[async_trait::async_trait]
impl UsageProbe for ScriptedUsage {
    async fn fetch(
        &self,
        _provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure> {
        self.0
            .lock()
            .ok()
            .and_then(|m| m.get(credential.expose()).copied())
            .ok_or_else(|| UsageFailure::new("no scripted usage"))
    }
}

fn now() -> Timestamp {
    Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
    )
}

/// The 5-hour window at `five` % (resetting in `five_reset_ms`) and the weekly window at `week` %
/// resetting in `week_reset_h` hours.
fn usage(five: f64, five_reset_ms: i64, week: f64, week_reset_h: i64) -> AccountUsage {
    AccountUsage {
        primary: Some(UsageWindow {
            used_percent: Some(five),
            window_minutes: Some(300),
            resets_at: Some(now().plus_millis(five_reset_ms)),
        }),
        secondary: Some(UsageWindow {
            used_percent: Some(week),
            window_minutes: Some(10_080),
            resets_at: Some(now().plus_millis(week_reset_h * HOUR)),
        }),
        ..AccountUsage::default()
    }
}

fn build(
    data: &Path,
    fake: &Path,
    usage: Arc<ScriptedUsage>,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_with(
        data,
        fake,
        usage,
        FakeAgentOptions {
            transcript: true,
            ..FakeAgentOptions::default()
        },
    )
}

fn build_with(
    data: &Path,
    fake: &Path,
    usage: Arc<ScriptedUsage>,
    agent: FakeAgentOptions,
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
    config.resume_stagger_ms = 1_500;
    let deps = Deps {
        store: Arc::new(store),
        clock: Arc::new(SystemClock),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[7u8; 32])?),
        github: Arc::new(FakeGitHub::default()),
        session_host: Arc::new(PtySessionHost::new(exe.clone())),
        harness: Arc::new(FakeAgentHarness::new(exe, fake.to_path_buf(), agent)),
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
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

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

/// User, sandbox (with its repository) and the given subscriptions (credential = id).
async fn seed(
    app: &App,
    data: &Path,
    accounts: &[&str],
) -> Result<(SandboxId, UserId), Box<dyn std::error::Error>> {
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
        id: SandboxId::parse("s-pool")?,
        slug: "pool".into(),
        repo_owner: "acme".into(),
        repo_name: "pool".into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base: 41200,
        port_count: 10,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: user.id.clone(),
        created_at: Timestamp(1),
        removed_at: None,
    };
    store.insert_sandbox(&sandbox).await?;
    for id in accounts {
        add_account(app, id).await?;
    }
    seed_repo(data, &sandbox.id)?;
    Ok((sandbox.id, user.id))
}

async fn add_account(app: &App, id: &str) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Anthropic,
            label: format!("Claude {id}"),
            credential: Some(app.deps.crypto.seal(id.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(account)
}

async fn refresh(app: &App, user: &UserId, account: &str) -> TestResult {
    let ctx = AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
    };
    app.refresh_account_usage(&ctx, account).await?;
    Ok(())
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

async fn task(app: &App, id: &TaskId) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

async fn has_status(app: &App, id: &TaskId, status: TaskStatus) -> bool {
    task(app, id).await.is_ok_and(|t| t.status == status)
}

fn screen(app: &App, id: &TaskId) -> String {
    app.runtime(id).hub.screen_text()
}

/// Every line the fake agent echoed: what the agent received as input.
fn echoed(app: &App, id: &TaskId) -> Vec<String> {
    screen(app, id)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ECHO: ").map(str::to_owned))
        .collect()
}

async fn stop(app: &Arc<App>, id: &TaskId) -> TestResult {
    if task(app, id).await?.status.is_resting() {
        return Ok(());
    }
    app.stop_task_now(id).await?;
    wait_for("the stop", Duration::from_secs(30), || async {
        task(app, id).await.is_ok_and(|t| t.status.is_resting())
    })
    .await
}

fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::Builder::new().prefix("yp").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// AC-ROT-9: a new task starts on the subscription whose capacity expires first.
#[test]
fn a_new_task_starts_on_the_capacity_that_expires_first() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-far", "b-soon"]).await?;
        // a-far: 90 % of its week left over 6 days; b-soon: 40 % left, resetting in 8 hours.
        probe.set("a-far", usage(5.0, 2 * HOUR, 8.0, 150));
        probe.set("b-soon", usage(5.0, 2 * HOUR, 58.0, 8));
        refresh(&app, &user, "a-far").await?;
        refresh(&app, &user, "b-soon").await?;
        let id = new_task(&app, &sandbox, &user, "Placement").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-soon"),
            "use it or lose it"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10, AC-ROT-14 (one subscription): its 5-hour window reaches 98 %; at the next turn
/// boundary the task pauses with its process alive, and the agent receives nothing. When the
/// window resets the task continues in place — same run, still nothing typed.
#[test]
fn one_exhausted_subscription_pauses_at_the_boundary_and_resumes_in_place() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Pause").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        probe.set("a-only", usage(98.5, HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::Running,
            "never mid-turn"
        );
        app.send_input(&id, b"first turn\r").await?;
        wait_for("the pause at the boundary", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Turn);
        let live = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("process kept")?;
        // Nothing reached the agent beyond what the user typed.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(echoed(&app, &id), ["first turn"]);

        // The window resets: the loop releases the task in place.
        probe.set("a-only", usage(0.0, 5 * HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        let _ = app.resume_round().await;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let after = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(after.id, live.id, "the same run continues");
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(echoed(&app, &id), ["first turn"], "still nothing typed");
        app.send_input(&id, b"second turn\r").await?;
        wait_for("the next turn", Duration::from_secs(10), || {
            let app = app.clone();
            let id = id.clone();
            async move { echoed(&app, &id).len() == 2 }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-11: every subscription exhausted → new tasks wait before starting (no process); a
/// subscription added later resumes them oldest first, one stagger apart, on it.
#[test]
fn a_new_subscription_resumes_waiting_tasks_oldest_first_and_staggered() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-full"]).await?;
        probe.set("a-full", usage(40.0, 3 * HOUR, 99.0, 50));
        refresh(&app, &user, "a-full").await?;
        let first = new_task(&app, &sandbox, &user, "First").await?;
        wait_for("the first pause", Duration::from_secs(30), || {
            has_status(&app, &first, TaskStatus::WaitingCapacity)
        })
        .await?;
        let second = new_task(&app, &sandbox, &user, "Second").await?;
        wait_for("the second pause", Duration::from_secs(30), || {
            has_status(&app, &second, TaskStatus::WaitingCapacity)
        })
        .await?;
        for id in [&first, &second] {
            assert!(app.deps.store.attempts(id).await?.is_empty(), "no process");
            let pause = app.deps.store.open_pause(id).await?.ok_or("pause")?;
            assert_eq!(pause.kind, PauseKind::Start);
        }
        let waiting = app.task_summary(&task(&app, &first).await?).await?;
        assert!(waiting.resumes_at.is_some(), "the weekly reset is known");

        // A subscription arrives.
        add_account(&app, "b-new").await?;
        probe.set("b-new", usage(0.0, 5 * HOUR, 5.0, 160));
        refresh(&app, &user, "b-new").await?;
        let _ = app.resume_round().await;
        for id in [&first, &second] {
            wait_for("the resumed run", Duration::from_secs(30), || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("FAKE-AGENT READY") }
            })
            .await?;
        }
        let one = app.deps.store.attempts(&first).await?;
        let two = app.deps.store.attempts(&second).await?;
        assert_eq!(
            one[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-new")
        );
        assert_eq!(
            two[0].account_id.as_ref().map(AccountId::as_str),
            Some("b-new")
        );
        let gap = two[0].started_at.millis_since(one[0].started_at);
        assert!(gap >= 1_400, "staggered, oldest first: {gap} ms");
        for id in [&first, &second] {
            stop(&app, id).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-11: a task paused on an exhausted subscription resumes on another one whose window
/// reset: a native resume of the same session there, with no prompt.
#[test]
fn a_paused_task_resumes_on_the_subscription_with_capacity_by_native_resume() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let id = new_task(&app, &sandbox, &user, "Move").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(
            first.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        probe.set("a-first", usage(98.0, 4 * HOUR, 30.0, 100));
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        app.send_input(&id, b"work\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;

        // b-second's window resets; a-first is still at 98 %.
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        wait_for("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        let second = &attempts[1];
        assert_eq!(
            second.account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        assert_eq!(
            second.provider_session_id, first.provider_session_id,
            "same session"
        );
        let launch = std::fs::read_to_string(Path::new(&second.run_dir).join("fake-launch.txt"))?;
        assert!(launch.contains("prompt: None"), "no prompt: {launch}");
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        assert!(app.deps.store.open_pause(&id).await?.is_none());
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

fn mcp_input(name: &str, transport: &str) -> McpServerInput {
    McpServerInput {
        name: name.into(),
        transport: transport.into(),
        command: (transport == "stdio").then(|| "npx".into()),
        args: vec!["-y".into(), format!("@example/{name}-mcp")],
        url: (transport == "http").then(|| format!("https://mcp.{name}.test/mcp")),
        env: None,
        headers: None,
        enabled: None,
        scope: None,
    }
}

/// The run's own MCP configuration (`--mcp-config`) and the names of its secret variables.
fn run_mcp(
    run_dir: &str,
) -> Result<(serde_json::Value, String, Vec<String>), Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(Path::new(run_dir).join("mcp.json"))?;
    let launch: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        Path::new(run_dir).join("launch.json"),
    )?)?;
    let secrets = launch["secret_env_names"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((serde_json::from_str(&text)?, text, secrets))
}

/// AC-MCP-3 (MCP-004): MCP servers configured in Yulius reach every run, whichever subscription
/// it runs on. Each run gets its own `mcp.json` (`--mcp-config`, strict) with the configured
/// servers and their secrets only as `${VAR}` references, and the values in the run's
/// environment. After the task moves to another subscription by native resume, the new run has
/// the same servers, plus one added in between.
#[test]
fn mcp_servers_reach_every_run_on_every_subscription() -> TestResult {
    const QUARTIO_TOKEN: &str = "synthetic-quartio-token";
    const APIFY_TOKEN: &str = "synthetic-apify-token";
    const LINEAR_KEY: &str = "synthetic-linear-key-1";
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            report_env: vec![
                "YULIUS_MCP_QUARTIO_TOKEN".into(),
                "APIFY_TOKEN".into(),
                "LINEAR_KEY".into(),
            ],
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        let ctx = AuthContext {
            user: app.deps.store.user(&user).await?.ok_or("user")?,
            csrf_token: None,
            session_hash: None,
        };
        let mut quartio = mcp_input("quartio", "http");
        quartio.scope = Some("instance".into());
        quartio.headers = Some(BTreeMap::from([(
            "Authorization".into(),
            format!("Bearer {QUARTIO_TOKEN}"),
        )]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &quartio)
            .await?;
        let mut apify = mcp_input("apify", "stdio");
        apify.env = Some(BTreeMap::from([("APIFY_TOKEN".into(), APIFY_TOKEN.into())]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &apify)
            .await?;

        let id = new_task(&app, &sandbox, &user, "Tools").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("ENV LINEAR_KEY=") }
        })
        .await?;
        let first = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(
            first.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );
        let (mcp, text, secrets) = run_mcp(&first.run_dir)?;
        let servers = mcp["mcpServers"].as_object().ok_or("mcpServers")?;
        let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["apify", "quartio", "yulius"]);
        assert_eq!(servers["quartio"]["url"], "https://mcp.quartio.test/mcp");
        assert_eq!(
            servers["quartio"]["headers"]["Authorization"],
            "Bearer ${YULIUS_MCP_QUARTIO_TOKEN}"
        );
        assert_eq!(servers["apify"]["command"], "npx");
        assert_eq!(servers["apify"]["env"]["APIFY_TOKEN"], "${APIFY_TOKEN}");
        for secret in [QUARTIO_TOKEN, APIFY_TOKEN] {
            assert!(!text.contains(secret), "the MCP file holds no secret value");
        }
        for name in ["YULIUS_MCP_QUARTIO_TOKEN", "APIFY_TOKEN"] {
            assert!(secrets.iter().any(|n| n == name), "{name} in {secrets:?}");
        }
        let shown = screen(&app, &id);
        assert!(
            shown.contains(&format!(
                "ENV YULIUS_MCP_QUARTIO_TOKEN=set len={}",
                QUARTIO_TOKEN.len()
            )),
            "{shown}"
        );
        assert!(
            shown.contains(&format!("ENV APIFY_TOKEN=set len={}", APIFY_TOKEN.len())),
            "{shown}"
        );
        assert!(shown.contains("ENV LINEAR_KEY=unset"), "{shown}");

        // A server added while the task runs; then the task moves to b-second.
        let mut linear = mcp_input("linear", "stdio");
        linear.env = Some(BTreeMap::from([("LINEAR_KEY".into(), LINEAR_KEY.into())]));
        app.create_mcp_server(&ctx, sandbox.as_str(), &linear)
            .await?;
        probe.set("a-first", usage(98.0, 4 * HOUR, 30.0, 100));
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "a-first").await?;
        refresh(&app, &user, "b-second").await?;
        app.send_input(&id, b"work\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        wait_for("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let second = app.deps.store.attempts(&id).await?.remove(1);
        assert_eq!(
            second.account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        assert_eq!(second.launch_kind, LaunchKind::Resume);
        let (mcp, text, secrets) = run_mcp(&second.run_dir)?;
        let servers = mcp["mcpServers"].as_object().ok_or("mcpServers")?;
        let mut names: Vec<&str> = servers.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["apify", "linear", "quartio", "yulius"]);
        assert_eq!(servers["linear"]["env"]["LINEAR_KEY"], "${LINEAR_KEY}");
        assert_eq!(
            servers["quartio"]["headers"]["Authorization"],
            "Bearer ${YULIUS_MCP_QUARTIO_TOKEN}"
        );
        assert!(!text.contains(LINEAR_KEY) && !text.contains(APIFY_TOKEN));
        for name in ["YULIUS_MCP_QUARTIO_TOKEN", "APIFY_TOKEN", "LINEAR_KEY"] {
            assert!(secrets.iter().any(|n| n == name), "{name} in {secrets:?}");
        }
        wait_for("the new run's environment", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move {
                screen(&app, &id).contains(&format!("ENV LINEAR_KEY=set len={}", LINEAR_KEY.len()))
            }
        })
        .await?;
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10: a pause survives a Yulius restart — the new server reattaches the paused run,
/// keeps it paused, and resumes it in place once capacity returns.
#[test]
fn a_pause_survives_a_server_restart() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let probe = Arc::new(ScriptedUsage::default());
    let first = runtime()?;
    let (user, id, child) = first.block_on(async {
        let app = build(&data, &fake, probe.clone())?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        let id = new_task(&app, &sandbox, &user, "Restart").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        probe.set("a-only", usage(99.0, HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        app.send_input(&id, b"before\r").await?;
        wait_for("the pause", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let live = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        Ok::<_, Box<dyn std::error::Error>>((user, id, live.child_pid))
    })?;
    first.shutdown_timeout(Duration::from_secs(5));

    let second = runtime()?;
    second.block_on(async {
        let app = build(&data, &fake, probe.clone())?;
        app.reconcile().await?;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::WaitingCapacity,
            "still paused"
        );
        assert!(app.deps.store.open_pause(&id).await?.is_some());
        let _ = app.resume_round().await;
        assert_eq!(
            task(&app, &id).await?.status,
            TaskStatus::WaitingCapacity,
            "no capacity yet"
        );
        probe.set("a-only", usage(1.0, 5 * HOUR, 40.0, 100));
        refresh(&app, &user, "a-only").await?;
        let _ = app.resume_round().await;
        assert_eq!(task(&app, &id).await?.status, TaskStatus::Running);
        let live = app.deps.store.live_attempt(&id).await?.ok_or("run")?;
        assert_eq!(live.child_pid, child, "the same process, never respawned");
        assert_eq!(app.deps.store.attempts(&id).await?.len(), 1);
        app.send_input(&id, b"after\r").await?;
        wait_for(
            "the echo after the restart",
            Duration::from_secs(10),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { screen(&app, &id).contains("ECHO: after") }
            },
        )
        .await?;
        assert!(
            !screen(&app, &id).contains("ECHO: Continue"),
            "nothing typed"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    second.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

/// AC-ROT-10: a turn the CLI itself interrupted with a usage limit, with every subscription
/// exhausted, pauses at the limit; it resumes on the subscription that recovers with the only
/// text the harness ever sends after a pause, a neutral "Continue." (no word about accounts,
/// limits or pauses).
#[test]
fn a_turn_interrupted_by_a_limit_resumes_with_a_neutral_continue() -> TestResult {
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let rt = runtime()?;
    rt.block_on(async {
        let probe = Arc::new(ScriptedUsage::default());
        let agent = FakeAgentOptions {
            transcript: true,
            limit_after: Some(1),
            ..FakeAgentOptions::default()
        };
        let app = build_with(&data, &fake, probe.clone(), agent)?;
        let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
        probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let id = new_task(&app, &sandbox, &user, "Limit").await?;
        wait_for("the run", Duration::from_secs(30), || {
            let app = app.clone();
            let id = id.clone();
            async move { screen(&app, &id).contains("FAKE-AGENT READY") }
        })
        .await?;
        app.send_input(&id, b"hello\r").await?;
        wait_for("the pause at the limit", Duration::from_secs(30), || {
            has_status(&app, &id, TaskStatus::WaitingCapacity)
        })
        .await?;
        let pause = app.deps.store.open_pause(&id).await?.ok_or("pause")?;
        assert_eq!(pause.kind, PauseKind::Limit);
        assert_eq!(
            pause.account_id.as_ref().map(AccountId::as_str),
            Some("a-first")
        );

        probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
        refresh(&app, &user, "b-second").await?;
        let _ = app.resume_round().await;
        wait_for("the resumed run", Duration::from_secs(60), || async {
            let attempts = app.deps.store.attempts(&id).await.unwrap_or_default();
            attempts.len() == 2 && attempts[1].state == yulius_domain::task::AttemptState::Live
        })
        .await?;
        let attempts = app.deps.store.attempts(&id).await?;
        assert_eq!(
            attempts[1].account_id.as_ref().map(AccountId::as_str),
            Some("b-second")
        );
        let launch =
            std::fs::read_to_string(Path::new(&attempts[1].run_dir).join("fake-launch.txt"))?;
        assert!(
            launch.contains(r#"prompt: Some("Continue.")"#),
            "only a neutral continuation: {launch}"
        );
        stop(&app, &id).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
