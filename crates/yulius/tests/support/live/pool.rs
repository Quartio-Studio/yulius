//! A pool of subscriptions whose usage a test scripts, and its tasks.

use crate::support;
use crate::support::live::{Live, UNSERVED};
pub(crate) use crate::support::live::{runtime, screen, task};
use crate::support::{eventually, git};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::TestResult;
use support::agents::FakeAgentOptions;
use yulius_app::auth::AuthContext;
use yulius_app::context::{App, AppConfig};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{
    AccountRecord, AccountStatus, SandboxRecord, Secret, TaskOrigin, UserRecord,
};
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, ModelAnswer, UsageWindow};

pub(crate) const HOUR: i64 = 3_600_000;

/// A usage probe whose answers the test sets per credential (account id = credential here):
/// the usage it reads, and what Claude answers when asked about a model on it (SPEC-473).
#[derive(Default)]
pub(crate) struct ScriptedUsage(
    Mutex<HashMap<String, AccountUsage>>,
    Mutex<HashMap<String, ModelAnswer>>,
);

impl ScriptedUsage {
    pub(crate) fn set(&self, account: &str, usage: AccountUsage) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(account.to_owned(), usage);
        }
    }

    /// What Claude answers a question about a model on `account`.
    pub(crate) fn answer(&self, account: &str, answer: ModelAnswer) {
        if let Ok(mut map) = self.1.lock() {
            map.insert(account.to_owned(), answer);
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

    async fn model_answer(
        &self,
        _provider: Provider,
        credential: &Secret,
        _model: &str,
    ) -> Result<ModelAnswer, UsageFailure> {
        self.1
            .lock()
            .ok()
            .and_then(|m| m.get(credential.expose()).copied())
            .ok_or_else(|| UsageFailure::new("no scripted answer"))
    }
}

pub(crate) fn now() -> Timestamp {
    Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
    )
}

/// The 5-hour window at `five` % (resetting in `five_reset_ms`) and the weekly window at `week` %
/// resetting in `week_reset_h` hours.
pub(crate) fn usage(five: f64, five_reset_ms: i64, week: f64, week_reset_h: i64) -> AccountUsage {
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

pub(crate) fn build(
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

/// [`build`] with `agent`'s options.
pub(crate) fn build_with(
    data: &Path,
    fake: &Path,
    usage: Arc<ScriptedUsage>,
    agent: FakeAgentOptions,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    pool(
        Live {
            usage,
            ..Live::new(Some(fake), agent)
        },
        data,
        UNSERVED,
        |_| {},
    )
}

/// The fake agent with `agent`'s options reading usage from an unscripted probe.
pub(crate) fn pool_live(fake: &Path, agent: FakeAgentOptions) -> Live {
    Live {
        usage: Arc::new(ScriptedUsage::default()),
        ..Live::new(Some(fake), agent)
    }
}

/// A pool's app built from `live`: its runs reach Yulius at `url` (as the instance's own adapter
/// tells them), sessions resume 1.5 s apart, then `tune` changes the configuration.
pub(crate) fn pool(
    live: Live,
    data: &Path,
    url: &str,
    tune: impl FnOnce(&mut AppConfig),
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    live.app(data, url, |config| {
        config.resume_stagger_ms = 1_500;
        tune(config);
    })
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
pub(crate) async fn seed(
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

pub(crate) async fn add_account(
    app: &App,
    id: &str,
) -> Result<AccountId, Box<dyn std::error::Error>> {
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
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(account)
}

/// A provider's reading of `account` now, as the background round makes it: what a reading
/// does to the stored usage, with no question about the limits stored for it.
pub(crate) async fn reading(app: &App, account: &str) -> TestResult {
    app.read_account_usage(&AccountId::parse(account)?)
        .await
        .map_err(|failure| failure.message)?;
    Ok(())
}

/// The Subscriptions page's refresh of `account` by an administrator: a reading, then Claude is
/// asked about each limit stored for it (SPEC-473).
pub(crate) async fn refresh(app: &App, user: &UserId, account: &str) -> TestResult {
    let ctx = AuthContext {
        user: app.deps.store.user(user).await?.ok_or("user")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    app.refresh_account_usage(&ctx, account).await?;
    Ok(())
}

pub(crate) async fn new_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    new_task_of(app, sandbox, user, title, None).await
}

/// A task a person set to Fable 5.2, started now: the model whose weekly allowance of usage
/// credits binds it and which falls back to Opus 5.5 (a default task runs Opus 5.5, SPEC-493).
pub(crate) async fn new_fable_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    new_task_of(app, sandbox, user, title, Some("claude-fable-5-2")).await
}

async fn new_task_of(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    model: Option<&str>,
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
            model: model.map(str::to_owned),
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

pub(crate) async fn has_status(app: &App, id: &TaskId, status: TaskStatus) -> bool {
    task(app, id).await.is_ok_and(|t| t.status == status)
}

/// Every line the fake agent echoed: what the agent received as input.
pub(crate) fn echoed(app: &App, id: &TaskId) -> Vec<String> {
    screen(app, id)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ECHO: ").map(str::to_owned))
        .collect()
}

pub(crate) async fn stop(app: &Arc<App>, id: &TaskId) -> TestResult {
    if task(app, id).await?.status.is_resting() {
        return Ok(());
    }
    app.stop_task_now(id).await?;
    eventually("the stop", Duration::from_secs(30), || async {
        task(app, id).await.is_ok_and(|t| t.status.is_resting())
    })
    .await
}

pub(crate) fn temp() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::Builder::new().prefix("yp").tempdir_in("/tmp")?;
    let data = dir.path().canonicalize()?;
    Ok((dir, data))
}

/// The delayed task gives its worker back to Tokio so the competing move can finish.
pub(crate) fn hold_pool_move(
    point: yulius_app::race::Point,
    id: &TaskId,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (ready, reached) = std::sync::mpsc::channel();
    let (go, release) = std::sync::mpsc::channel();
    yulius_app::race::on(point, Path::new(id.as_str()), move || {
        tokio::task::block_in_place(|| {
            let _ = ready.send(());
            let _ = release.recv_timeout(Duration::from_secs(60));
        });
    });
    (reached, go)
}
