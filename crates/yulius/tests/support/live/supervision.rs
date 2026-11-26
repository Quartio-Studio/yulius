//! Shared fixtures moved from `supervision`.

use crate::support;
use crate::support::git;
use crate::support::live::{Live, UNSERVED};
use std::path::Path;
use std::sync::Arc;
use support::TestResult;
use support::agents::FakeAgentOptions;
use yulius_app::context::App;
use yulius_app::model::{AccountRecord, AccountStatus, SandboxRecord, TaskRecord, UserRecord};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

pub(crate) fn build(data: &Path, fake: &Path) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_with(data, Some(fake))
}

/// `fake = None` launches the real agent CLIs (live checks only).
pub(crate) fn build_with(
    data: &Path,
    fake: Option<&Path>,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    build_full(data, fake, FakeAgentOptions::default(), 8)
}

pub(crate) fn build_full(
    data: &Path,
    fake: Option<&Path>,
    agent: FakeAgentOptions,
    max_live_sessions: u32,
) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    Live::new(fake, agent).app(data, UNSERVED, |c| c.max_live_sessions = max_live_sessions)
}

/// Bare origin with one commit on `main`, cloned as the sandbox repository.
pub(crate) fn seed_repo(data: &Path, sandbox: &SandboxId) -> TestResult {
    seed_repo_with(data, sandbox, &[])
}

/// [`seed_repo`] with `files` (path, contents) committed next to the README.
pub(crate) fn seed_repo_with(
    data: &Path,
    sandbox: &SandboxId,
    files: &[(&str, &str)],
) -> TestResult {
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    for (path, contents) in files {
        std::fs::write(seed.join(path), contents)?;
    }
    git(&seed, &["add", "-A"])?;
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

pub(crate) async fn seed_records(
    app: &App,
) -> Result<(SandboxId, UserId), Box<dyn std::error::Error>> {
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
    Ok((sandbox.id, user.id))
}

pub(crate) async fn task(app: &App, id: &TaskId) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    Ok(app.deps.store.task(id).await?.ok_or("task vanished")?)
}

pub(crate) async fn screen_contains(app: &App, id: &TaskId, needle: &str) -> bool {
    app.runtime(id).hub.screen_text().contains(needle)
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}
