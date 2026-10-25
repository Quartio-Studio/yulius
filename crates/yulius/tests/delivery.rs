//! Git page and delivery flows end to end (AC-DLV-1..3, AC-GIT-1/2, AC-DSC-2): the HTTP API
//! over the application with a fake GitHub whose pull requests and refs are backed by a real
//! bare repository, and a test-local `GitPort` over the real `git` command line.

use crate::support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, FakeRemote, Resp, TestResult, call};
use yulius_app::agent_wait::WaitState;
use yulius_app::delivery::profile_to_contract;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{
    AttemptRecord, GitHubTokenRecord, LaunchKind, SandboxRecord, Sealed, Secret, StepRecord,
    TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort, MergeIntegration};
use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState, GhPull};
use yulius_app::ports::session::{
    AttachedRun, Containment, LaunchPlan, RunInspection, SessionHost, SpawnedRun,
};
use yulius_app::ports::store::{
    CoordinationStore, InstanceStore, SandboxStore, TaskStore, UserStore,
};
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::{DeliveryProfile, DeliveryStage};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------- real git helpers

const GIT_ENV: [(&str, &str); 3] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
];
/// Identity for commits; no fsync (throwaway repositories: keeps tests fast on busy disks).
const IDENTITY: [&str; 8] = [
    "-c",
    "user.name=Yulius Test",
    "-c",
    "user.email=test@yulius.test",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.fsync=none",
];

fn git_sync(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(IDENTITY)
        .args(args)
        .current_dir(dir)
        .envs(GIT_ENV)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git(dir: &Path, args: &[&str]) -> BoxResult<String> {
    Ok(git_sync(dir, args)?)
}

/// Test-local Git port over the real `git` command line (no network: "origin" is a local bare
/// repository; URLs are ignored, and pushes record the token they carried).
struct RealGit {
    origin: PathBuf,
    /// Pushes, in order: `<local>-><remote> token=<token>`.
    pushes: std::sync::Mutex<Vec<String>>,
    /// How the next pushes fail, until cleared (SPEC-139).
    push_failure: std::sync::Mutex<Option<PushFailure>>,
    /// When set, `ahead_behind` fails with this message (SPEC-139 PSH-003).
    ahead_behind_error: std::sync::Mutex<Option<String>>,
    /// While set, the remote's branches cannot be read (`ls-remote` fails, SPEC-139).
    remote_unreadable: std::sync::atomic::AtomicBool,
}

/// An injected push failure, as GitHub's `! [remote rejected] … (failed)` (#366).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PushFailure {
    /// The remote is left as it was.
    Refused,
    /// The remote branch moved, and the push still reports the failure.
    AfterLanding,
}

impl RealGit {
    fn fail_pushes(&self, failure: Option<PushFailure>) {
        if let Ok(mut slot) = self.push_failure.lock() {
            *slot = failure;
        }
    }

    fn remote_unreadable(&self, unreadable: bool) {
        self.remote_unreadable
            .store(unreadable, std::sync::atomic::Ordering::SeqCst);
    }

    fn fail_ahead_behind(&self, message: Option<&str>) {
        if let Ok(mut slot) = self.ahead_behind_error.lock() {
            *slot = message.map(str::to_owned);
        }
    }
}

impl RealGit {
    async fn run(&self, dir: &Path, args: &[&str]) -> AppResult<String> {
        let output = tokio::process::Command::new("git")
            .args(IDENTITY)
            .args(args)
            .current_dir(dir)
            .envs(GIT_ENV)
            .output()
            .await
            .map_err(AppError::internal)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(AppError::Upstream(format!(
                "git {}: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

#[async_trait]
impl GitPort for RealGit {
    fn valid_branch(&self, name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('-')
            && !name.contains("..")
            && !name.chars().any(|c| c.is_whitespace() || c.is_control())
    }
    async fn sync_clone(&self, _url: &str, dest: &Path, _token: &Secret) -> AppResult<()> {
        if dest.join(".git").exists() {
            self.run(dest, &["fetch", "--prune", "--quiet", "origin"])
                .await?;
        } else {
            let origin = self.origin.to_string_lossy().into_owned();
            let target = dest.to_string_lossy().into_owned();
            self.run(Path::new("/"), &["clone", "--quiet", &origin, &target])
                .await?;
        }
        Ok(())
    }
    /// The production Git against the bare origin (the URL is ignored, as everywhere here).
    async fn mirror_fetch_head(
        &self,
        mirror: &Path,
        _source: &str,
        branch: &str,
        _token: Option<&Secret>,
    ) -> AppResult<String> {
        let origin = self.origin.to_string_lossy().into_owned();
        yulius::wiring::git::GitAdapter::new("git")
            .mirror_fetch_head(mirror, &origin, branch, None)
            .await
    }
    async fn export_tree(&self, mirror: &Path, sha: &str, dest: &Path) -> AppResult<()> {
        yulius::wiring::git::GitAdapter::new("git")
            .export_tree(mirror, sha, dest)
            .await
    }
    async fn default_branch(&self, repo: &Path) -> AppResult<String> {
        let head = self
            .run(
                repo,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )
            .await?;
        Ok(head.trim_start_matches("origin/").to_owned())
    }
    async fn branch_exists(&self, repo: &Path, branch: &str) -> AppResult<bool> {
        for candidate in [
            format!("refs/heads/{branch}"),
            format!("refs/remotes/origin/{branch}"),
        ] {
            if self
                .run(repo, &["rev-parse", "--verify", "--quiet", &candidate])
                .await
                .is_ok()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    async fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> AppResult<()> {
        let path = path.to_string_lossy().into_owned();
        self.run(
            repo,
            &["worktree", "add", "--quiet", "-b", branch, &path, base],
        )
        .await
        .map(drop)
    }
    async fn remove_worktree(&self, _: &Path, _: &Path, _: &Path) -> AppResult<bool> {
        Ok(false)
    }
    async fn merge_integration(
        &self,
        worktree: &Path,
        integration: &str,
        _token: &Secret,
    ) -> AppResult<MergeIntegration> {
        self.run(worktree, &["fetch", "--quiet", "origin"]).await?;
        // The product's merge (identity from the worktree's own configuration).
        let git = yulius_git::Git::new("git")
            .with_env("GIT_CONFIG_GLOBAL", "/dev/null")
            .with_env("GIT_CONFIG_NOSYSTEM", "1");
        let (worktree, rev) = (worktree.to_path_buf(), format!("origin/{integration}"));
        let merged = tokio::task::spawn_blocking(move || git.merge_into(&worktree, &rev))
            .await
            .map_err(AppError::internal)?
            .map_err(|e| AppError::Upstream(e.to_string()))?;
        Ok(match merged {
            yulius_git::MergeInto::UpToDate => MergeIntegration::UpToDate,
            yulius_git::MergeInto::Merged(head) => MergeIntegration::Merged(head),
            yulius_git::MergeInto::Conflict(files) => MergeIntegration::Conflict(files),
        })
    }
    async fn has_changes(&self, worktree: &Path) -> AppResult<bool> {
        Ok(!self
            .run(worktree, &["status", "--porcelain"])
            .await?
            .is_empty())
    }
    async fn install_push_guard(&self, _: &Path, _: &[String]) -> AppResult<()> {
        Ok(())
    }
    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()> {
        self.run(worktree, &["config", "user.name", name]).await?;
        self.run(worktree, &["config", "user.email", email])
            .await
            .map(drop)
    }
    /// The production adapter: the review reads real Git output.
    async fn review_changes(
        &self,
        worktree: &Path,
        base: &str,
    ) -> AppResult<yulius_app::ports::git::ReviewChanges> {
        yulius::wiring::git::GitAdapter::new("git")
            .review_changes(worktree, base)
            .await
    }

    async fn review_commits(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
        limit: u32,
    ) -> AppResult<Vec<yulius_contracts::api::git::TaskCommit>> {
        yulius::wiring::git::GitAdapter::new("git")
            .review_commits(repo, base, head, limit)
            .await
    }

    /// The production adapter (SPEC-083): record numbers, review carry-over, merge attributes.
    async fn list_dir(&self, repo: &Path, rev: &str, dir: &str) -> AppResult<Vec<String>> {
        yulius::wiring::git::GitAdapter::new("git")
            .list_dir(repo, rev, dir)
            .await
    }

    async fn only_clean_merges_since(
        &self,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: &str,
        limit: usize,
    ) -> AppResult<bool> {
        yulius::wiring::git::GitAdapter::new("git")
            .only_clean_merges_since(worktree, reviewed, head, integration, limit)
            .await
    }

    async fn set_merge_attributes(&self, repo: &Path, lines: &[String]) -> AppResult<bool> {
        yulius::wiring::git::GitAdapter::new("git")
            .set_merge_attributes(repo, lines)
            .await
    }

    async fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> AppResult<DiffStat> {
        let text = self
            .run(repo, &["diff", "--shortstat", &format!("{base}...{head}")])
            .await?;
        let mut stat = DiffStat::default();
        for part in text.split(',') {
            let mut words = part.split_whitespace();
            let number = words
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            match words.next() {
                Some(w) if w.starts_with("file") => stat.files = number,
                Some(w) if w.starts_with("insertion") => stat.insertions = number,
                Some(w) if w.starts_with("deletion") => stat.deletions = number,
                _ => {}
            }
        }
        Ok(stat)
    }
    async fn ahead_behind(&self, repo: &Path, base: &str, head: &str) -> AppResult<(u32, u32)> {
        if let Some(message) = self.ahead_behind_error.lock().ok().and_then(|e| e.clone()) {
            return Err(AppError::Upstream(message));
        }
        let text = self
            .run(
                repo,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{base}...{head}"),
                ],
            )
            .await?;
        let mut counts = text
            .split_whitespace()
            .map(|n| n.parse::<u32>().unwrap_or(0));
        let behind = counts.next().unwrap_or(0);
        let ahead = counts.next().unwrap_or(0);
        Ok((ahead, behind))
    }
    async fn log(&self, repo: &Path, rev: &str, limit: u32) -> AppResult<Vec<CommitInfo>> {
        let text = self
            .run(
                repo,
                &[
                    "log",
                    &format!("-n{limit}"),
                    "--format=%H%x1f%s%x1f%an%x1f%ct",
                    rev,
                    "--",
                ],
            )
            .await?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\u{1f}');
                Some(CommitInfo {
                    sha: fields.next()?.to_owned(),
                    subject: fields.next()?.to_owned(),
                    author: fields.next()?.to_owned(),
                    at_ms: fields.next()?.parse::<i64>().ok()? * 1000,
                })
            })
            .collect())
    }
    async fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        _: &str,
        _: &str,
    ) -> AppResult<bool> {
        self.run(worktree, &["add", "-A"]).await?;
        if self
            .run(worktree, &["diff", "--cached", "--quiet"])
            .await
            .is_ok()
        {
            return Ok(false);
        }
        self.run(worktree, &["commit", "--quiet", "-m", message])
            .await
            .map(|_| true)
    }
    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()> {
        if let Ok(mut pushes) = self.pushes.lock() {
            pushes.push(format!(
                "{local_branch}->{remote_branch} token={}",
                token.expose()
            ));
        }
        let failure = self.push_failure.lock().ok().and_then(|f| *f);
        let rejected = || {
            AppError::Upstream(format!(
                "git push: ! [remote rejected] {local_branch} -> {remote_branch} (failed)"
            ))
        };
        if failure == Some(PushFailure::Refused) {
            return Err(rejected());
        }
        self.run(
            repo,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("refs/heads/{local_branch}:refs/heads/{remote_branch}"),
            ],
        )
        .await?;
        match failure {
            Some(_) => Err(rejected()),
            None => Ok(()),
        }
    }
    async fn remote_branch_head(
        &self,
        repo: &Path,
        branch: &str,
        _token: &Secret,
    ) -> AppResult<Option<String>> {
        if self
            .remote_unreadable
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AppError::Upstream(
                "git ls-remote: could not read from remote".into(),
            ));
        }
        let listed = self
            .run(
                repo,
                &["ls-remote", "origin", &format!("refs/heads/{branch}")],
            )
            .await?;
        Ok(listed.split_whitespace().next().map(str::to_owned))
    }
    async fn import_bundle(&self, _: &Path, _: &Path, _: &str) -> AppResult<Vec<String>> {
        Err(AppError::Upstream("not used".into()))
    }
    /// The branch policy is read from `yulius.toml` at the integration branch (SPEC-069).
    async fn show_file(&self, repo: &Path, rev: &str, path: &str) -> AppResult<Option<String>> {
        let object = format!("{rev}:{path}");
        if self.run(repo, &["cat-file", "-e", &object]).await.is_err() {
            return Ok(None);
        }
        self.run(repo, &["show", &object]).await.map(Some)
    }
}

/// The fake GitHub's repository: merges and fast-forwards happen directly in the bare origin
/// (`merge-tree` + `commit-tree` + `update-ref`, no working tree).
struct BareRemote {
    origin: PathBuf,
}

impl FakeRemote for BareRemote {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        // Pushed refs are loose files in the bare repository; fall back to git for packed ones.
        if let Ok(text) = std::fs::read_to_string(self.origin.join("refs/heads").join(branch)) {
            return Some(text.trim().to_owned());
        }
        git_sync(
            &self.origin,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
    }
    fn merge(
        &self,
        base: &str,
        head_sha: &str,
        method: &str,
        message: &str,
    ) -> Result<String, String> {
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = git_sync(
            &self.origin,
            &["merge-tree", "--write-tree", &base_sha, head_sha],
        )?;
        let commit = match method {
            "merge" => git_sync(
                &self.origin,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &base_sha,
                    "-p",
                    head_sha,
                    "-m",
                    message,
                ],
            )?,
            "squash" => git_sync(
                &self.origin,
                &["commit-tree", &tree, "-p", &base_sha, "-m", message],
            )?,
            other => return Err(format!("unsupported merge method {other}")),
        };
        git_sync(
            &self.origin,
            &[
                "update-ref",
                &format!("refs/heads/{base}"),
                &commit,
                &base_sha,
            ],
        )?;
        Ok(commit)
    }
    fn fast_forward(&self, branch: &str, sha: &str) -> Result<(), String> {
        let reference = format!("refs/heads/{branch}");
        let current = self.branch_sha(branch);
        if let Some(current) = &current {
            git_sync(&self.origin, &["merge-base", "--is-ancestor", current, sha])
                .map_err(|_| "not a fast-forward".to_owned())?;
        }
        let mut args = vec!["update-ref", reference.as_str(), sha];
        if let Some(current) = &current {
            args.push(current);
        }
        git_sync(&self.origin, &args).map(drop)
    }
    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        if self.branch_sha(branch).is_none() {
            return Ok(false);
        }
        git_sync(
            &self.origin,
            &["update-ref", "-d", &format!("refs/heads/{branch}")],
        )?;
        Ok(true)
    }
}

// ---------------------------------------------------------------- fixture

const SPEC_PATH: &str = "docs/specs/2026-10-07-checkout-api.md";

fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\nCustomers can pay for their cart.\n\n\
         ## Context\nThe shop has a cart but no checkout.\n\n\
         ## Requirements\n- POST /checkout creates an order.\n\n\
         ## Plan\n- [x] Write the spec\n- [ ] Implement the endpoint\n\n\
         ## Acceptance criteria\n- An order is created.\n\n\
         ## Test plan\n- Integration test for POST /checkout.\n\n\
         ## Decisions\n- None yet.\n"
    )
}

struct Fixture {
    h: support::Harness,
    git: Arc<RealGit>,
    root: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    /// Admin on the repository.
    alice: Creds,
    /// Write access.
    bob: Creds,
    /// Read access.
    rita: Creds,
}

async fn add_user(
    h: &support::Harness,
    id: &str,
    login: &str,
    perms: GitHubPermissions,
) -> BoxResult<Creds> {
    let user = UserRecord {
        id: UserId::parse(id)?,
        github_login: Some(login.into()),
        github_id: None,
        display_name: login.to_uppercase(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&user).await?;
    let access = h
        .app
        .deps
        .crypto
        .seal(format!("token-{login}").as_bytes())?;
    h.store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: user.id.clone(),
                access,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            Timestamp(1),
        )
        .await?;
    h.github.set_perm(login, "Quartio-Studio/shop", perms);
    let token = h
        .app
        .issue_api_token(&user, "test", yulius_domain::permission::TokenScope::Cli)
        .await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

async fn fixture(profile: &DeliveryProfile) -> BoxResult<Fixture> {
    fixture_with(profile, Box::new(|_| {}), None).await
}

/// [`fixture`] with the configuration adjusted, and another process host than the real one.
async fn fixture_with(
    profile: &DeliveryProfile,
    configure: support::ConfigureFn,
    host: Option<Arc<dyn SessionHost>>,
) -> BoxResult<Fixture> {
    let root = tempfile::tempdir()?;
    let origin = root.path().join("origin.git");
    git(
        root.path(),
        &[
            "init",
            "--quiet",
            "--bare",
            "-b",
            "main",
            &origin.to_string_lossy(),
        ],
    )?;
    let real_git = Arc::new(RealGit {
        origin: origin.clone(),
        pushes: std::sync::Mutex::default(),
        push_failure: std::sync::Mutex::default(),
        ahead_behind_error: std::sync::Mutex::default(),
        remote_unreadable: std::sync::atomic::AtomicBool::default(),
    });
    // Environment runs go through the real process host, as in production (SPEC-219); its
    // sockets live at <data>/runs/env-<uuid>/pty.sock, so the data directory's path is short.
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = support::harness_with(support::HarnessOptions {
        git: Some(real_git.clone()),
        session_host: Some(
            host.unwrap_or_else(|| Arc::new(yulius::wiring::session::PtySessionHost::new(exe))),
        ),
        dir: Some(tempfile::Builder::new().prefix("ydl").tempdir_in("/tmp")?),
        configure: Some(configure),
        ..support::HarnessOptions::default()
    })?;
    h.github.set_remote(Arc::new(BareRemote {
        origin: origin.clone(),
    }));
    h.store.init_instance("https://yulius.test", "x").await?;
    let app_cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c", "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    let sealed = h.app.deps.crypto.seal(app_cfg.to_string().as_bytes())?;
    h.store
        .set_github_app(&sealed, "Quartio-Studio", "org")
        .await?;
    h.store
        .insert_sandbox(&SandboxRecord {
            id: SandboxId::parse("s1")?,
            slug: "shop".into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: "shop".into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: serde_json::to_string(&profile_to_contract(profile))?,
            port_base: 20000,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u-alice")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    git(
        &repo,
        &["commit", "--quiet", "--allow-empty", "-m", "Initial commit"],
    )?;
    git(
        &repo,
        &[
            "push",
            "--quiet",
            "origin",
            "HEAD:refs/heads/main",
            &format!("HEAD:refs/heads/{}", profile.integration_branch),
        ],
    )?;
    let alice = add_user(
        &h,
        "u-alice",
        "alice",
        GitHubPermissions {
            admin: true,
            push: true,
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    let bob = add_user(
        &h,
        "u-bob",
        "bob",
        GitHubPermissions {
            push: true,
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    let rita = add_user(
        &h,
        "u-rita",
        "rita",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    Ok(Fixture {
        h,
        git: real_git,
        root,
        origin,
        repo,
        alice,
        bob,
        rita,
    })
}

impl Fixture {
    /// A task of Bob's with a worktree on its own branch from the integration branch.
    async fn add_task(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        base: &str,
    ) -> BoxResult<PathBuf> {
        self.add_task_by(id, title, branch, base, "u-bob").await
    }

    /// [`Fixture::add_task`] created by the user `creator`.
    async fn add_task_by(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        base: &str,
        creator: &str,
    ) -> BoxResult<PathBuf> {
        let worktree = self
            .h
            .app
            .config
            .data_dir
            .join(format!("sandboxes/s1/worktrees/{id}"));
        git(&self.repo, &["fetch", "--quiet", "origin"])?;
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &worktree.to_string_lossy(),
                &format!("origin/{base}"),
            ],
        )?;
        self.h
            .store
            .insert_task(&TaskRecord {
                id: TaskId::parse(id)?,
                sandbox_id: SandboxId::parse("s1")?,
                number: 0,
                title: title.into(),
                prompt: format!("Build: {title}"),
                harness: Harness::ClaudeCode,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Closed,
                activity: None,
                branch: Some(branch.into()),
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: UserId::parse(creator)?,
                owner: UserId::parse(creator)?,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
            })
            .await?;
        Ok(worktree)
    }

    async fn req(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        who: &Creds,
    ) -> BoxResult<Resp> {
        call(&self.h.router, method, path, body, who).await
    }

    async fn delivery(&self, task: &str) -> BoxResult<Value> {
        let r = self
            .req(
                Method::GET,
                &format!("/api/tasks/{task}/delivery"),
                None,
                &self.bob,
            )
            .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    /// Pushes so far (`<local>-><remote> token=<token>`).
    fn pushes(&self) -> Vec<String> {
        self.git
            .pushes
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    fn origin_sha(&self, branch: &str) -> BoxResult<String> {
        git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    /// Polls an environment run until it leaves `running` (bounded).
    async fn wait_run(&self, run_id: &str) -> BoxResult<Value> {
        for _ in 0..400 {
            let r = self
                .req(
                    Method::GET,
                    &format!("/api/sandboxes/s1/delivery/runs/{run_id}"),
                    None,
                    &self.rita,
                )
                .await?;
            assert_eq!(r.status, StatusCode::OK, "{}", r.body);
            if r.body["state"] != "running" {
                return Ok(r.body);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("environment run did not finish".into())
    }
}

fn commit_file(worktree: &Path, path: &str, contents: &str, message: &str) -> BoxResult<()> {
    let full = worktree.join(path);
    std::fs::create_dir_all(full.parent().ok_or("parent")?)?;
    std::fs::write(full, contents)?;
    git(worktree, &["add", "-A"])?;
    git(worktree, &["commit", "--quiet", "-m", message])?;
    Ok(())
}

fn actions(delivery: &Value) -> Vec<String> {
    delivery["next_actions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn error_message(r: &Resp) -> String {
    r.body["error"]["message"].as_str().unwrap_or("").to_owned()
}

fn product_with_deploy() -> Value {
    json!({"profile": {
        "preset": "product", "integration_branch": "quartio", "production_branch": "main",
        "merge": "merge", "promote": "fast_forward", "auto_promote": false,
        "environments": [
            {"name": "dev-vm", "after": "integration", "verify": "command",
             "command": "printf 'deployed %s on %s\\n' \"$(cat feature.txt)\" \"$YULIUS_ENVIRONMENT\""},
            {"name": "rollout", "after": "production", "verify": "manual", "command": null}
        ]
    }})
}

/// Replaces a task's steps with one step per status.
async fn set_steps(f: &Fixture, task: &str, statuses: &[&str]) -> BoxResult<()> {
    let id = TaskId::parse(task)?;
    let steps = statuses
        .iter()
        .enumerate()
        .map(|(i, status)| {
            Ok(StepRecord {
                id: StepId::parse(&format!("{task}-step-{i}"))?,
                task_id: id.clone(),
                ordinal: u32::try_from(i)?,
                title: format!("Step {i}"),
                status: (*status).to_owned(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            })
        })
        .collect::<BoxResult<Vec<_>>>()?;
    f.h.store
        .replace_steps(&id, &steps, "test", Timestamp(2))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- tests

/// AC-DSC-2, AC-GIT-1: the PR is refused until the task has a valid committed spec, which is
/// discovered and registered; the Git page shows the branch facts, the PR and its checks.
#[tokio::test]
async fn pull_request_requires_a_valid_committed_spec() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;

    // No spec: delivery explains it and refuses the pull request.
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "none");
    assert_eq!(d["spec_valid"], false);
    assert!(actions(&d).is_empty());
    assert_eq!(d["blocked"][0]["action"], "open_pr");
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(error_message(&refused).contains("spec"), "{}", refused.body);
    assert!(f.h.github.writes().is_empty(), "nothing reached GitHub");
    assert!(
        f.origin_sha("feat/1-checkout-api").is_err(),
        "nothing was pushed"
    );

    // An invalid spec is discovered, registered and its problems are reported.
    commit_file(
        &wt,
        SPEC_PATH,
        "---\nyulius-task: t1\nstatus: draft\n---\n## Goal\nPay.\n",
        "Draft spec",
    )?;
    let refused = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({})),
            &f.bob,
        )
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("Missing section '## Context'"),
        "{}",
        refused.body
    );

    // A valid but uncommitted spec is refused too: the PR must contain what was validated.
    std::fs::write(wt.join(SPEC_PATH), valid_spec("t1"))?;
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("uncommitted"),
        "{}",
        refused.body
    );
    git(&wt, &["commit", "--quiet", "-am", "Complete spec"])?;

    let d = f.delivery("t1").await?;
    assert_eq!(d["spec_valid"], true, "{d}");
    assert_eq!(d["spec_path"], SPEC_PATH);
    assert_eq!(actions(&d), ["open_pr"]);
    let detail = f.req(Method::GET, "/api/tasks/t1", None, &f.rita).await?;
    assert_eq!(
        detail.body["spec_path"], SPEC_PATH,
        "discovered spec is registered"
    );
    assert_eq!(detail.body["delivery"]["spec_valid"], true);

    // Read access cannot open the PR; write access can.
    let forbidden = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.rita)
        .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["stage"], "pr_open");
    assert_eq!(opened.body["pr"]["number"], 1);
    assert_eq!(opened.body["pr"]["base"], "dev");
    assert_eq!(opened.body["pr"]["head"], "feat/1-checkout-api");
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(
        f.origin_sha("feat/1-checkout-api")?,
        head,
        "branch pushed first"
    );
    let pull = f.h.github.pull(1).ok_or("pull")?;
    // The branch policy's conventional title from the branch type (SPEC-069 BPL-006).
    assert_eq!(pull.title, "feat: checkout API");
    assert_eq!(
        f.h.github.writes(),
        ["create_pull #1 dev<-feat/1-checkout-api"]
    );

    // Git page (read access): branch facts, PR with checks, integration commits.
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["integration_branch"], "dev");
    assert_eq!(page.body["production_branch"], "main");
    let task = &page.body["tasks"][0];
    assert_eq!(task["branch"], "feat/1-checkout-api");
    assert_eq!(task["ahead"], 3);
    assert_eq!(task["behind"], 0);
    assert_eq!(task["files_changed"], 2);
    assert_eq!(task["pr"]["number"], 1);
    // The standard profile requires checks: none reported yet reads as pending (#68).
    assert_eq!(task["pr"]["checks"], "pending");
    assert_eq!(page.body["pulls"].as_array().map(Vec::len), Some(1));
    assert_eq!(page.body["recent_commits"][0]["subject"], "Initial commit");
    assert_eq!(page.body["notes"], json!([]));

    Ok(())
}

/// AC-DLV-1: standard profile — merging waits for green checks and nothing waits for a person
/// after it; promotion opens a promotion PR dev → main and merges it on a later request.
#[tokio::test]
async fn standard_flow_merges_on_green_checks_and_promotes_by_pr() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;

    // Checks gate the merge (standard requires them).
    f.h.github.set_checks(&head, ChecksState::Pending);
    let pending = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(pending.status, StatusCode::CONFLICT);
    assert!(error_message(&pending).contains("still running"));
    f.h.github.set_checks(&head, ChecksState::Failure);
    let failing = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert!(error_message(&failing).contains("checks failed"));
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "failure");
    assert_eq!(d["blocked"][0]["action"], "merge");
    let forbidden = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.rita)
        .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    f.h.github.set_checks(&head, ChecksState::Success);
    // F 2026-10-11: behind its base under a strict ruleset, GitHub would refuse with "required
    // status checks are expected"; Yulius says what that means.
    let set_behind = |behind: bool| {
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            pull.behind = behind;
        }
    };
    set_behind(true);
    let behind = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(behind.status, StatusCode::CONFLICT, "{}", behind.body);
    assert!(
        error_message(&behind).contains("behind `dev`"),
        "{}",
        behind.body
    );
    f.h.app.reconcile_deliveries().await;
    f.h.app.reconcile_deliveries().await;
    let t1 = TaskId::parse("t1")?;
    assert!(
        f.h.store.task_messages(&t1, 50).await?.is_empty(),
        "MSR-002 (SPEC-028): no message"
    );
    set_behind(false);
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    assert_eq!(merged.body["pr"]["state"], "merged");
    assert_eq!(actions(&merged.body), ["promote"]);
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #1 squash".to_owned())
    );
    assert_eq!(git(&f.origin, &["show", "dev:feature.txt"])?, "hello");
    // Hygiene (DLV-007): the merged head branch is deleted on GitHub.
    assert!(
        f.h.github
            .writes()
            .contains(&"delete_branch feat/1-checkout-api".to_owned()),
        "{:?}",
        f.h.github.writes()
    );
    assert!(
        git(
            &f.origin,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/feat/1-checkout-api"
            ]
        )
        .is_err()
    );

    // DLV-009: nothing between the merge and promotion waits for a person.
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "merged_integration");
    assert_eq!(actions(&d), ["promote"]);

    // Promotion: first a promotion PR, merged by a later request once its checks allow.
    let main_before = f.origin_sha("main")?;
    let opened = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["result"], "pr_opened");
    assert!(
        opened.body["sha"]
            .as_str()
            .unwrap_or("")
            .ends_with("/pull/2")
    );
    assert!(
        f.h.github
            .writes()
            .contains(&"create_pull #2 main<-dev".to_owned())
    );
    assert_eq!(
        f.origin_sha("main")?,
        main_before,
        "opening the PR changes nothing"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    let promotion_head = f.h.github.pull(2).ok_or("pull 2")?.head_sha;
    f.h.github.set_checks(&promotion_head, ChecksState::Pending);
    let waiting = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(waiting.status, StatusCode::CONFLICT);
    // Checks that can't be read never count as passed for the release either (#111).
    f.h.github.set_checks_report(
        &promotion_head,
        ChecksReport {
            state: ChecksState::Unknown,
            unreadable: vec![ChecksSource::CheckRuns, ChecksSource::CommitStatuses],
        },
    );
    let unknown = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(unknown.status, StatusCode::CONFLICT);
    assert!(
        error_message(&unknown).contains("can't be read"),
        "{}",
        unknown.body
    );
    f.h.github.set_checks(&promotion_head, ChecksState::Success);
    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(promoted.body["result"], "promoted");
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #2 merge".to_owned())
    );
    assert_eq!(git(&f.origin, &["show", "main:feature.txt"])?, "hello");
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "production_done");
    assert!(actions(&d).is_empty());
    Ok(())
}

/// AC-DLV-2: product profile — PR into quartio, the dev-VM deployment runs only on an explicit
/// confirmed request and records its output, then main fast-forwards and the rollout step is
/// confirmed. Only repository admins change the profile.
#[tokio::test]
async fn product_flow_deploys_on_confirmation_then_fast_forwards_and_rolls_out() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    let denied = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.bob,
        )
        .await?;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "write access cannot change the profile"
    );
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(
        patched.body["profile"]["preset"], "custom",
        "a deploy command differs from the preset"
    );
    assert_eq!(patched.body["profile"]["require_checks"], true);
    let invalid = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {"preset": "product", "integration_branch": "main", "production_branch": "main", "merge": "merge", "promote": "fast_forward", "auto_promote": false, "environments": []}})),
            &f.alice,
        )
        .await?;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);

    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({"title": "feat: checkout"})),
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["pr"]["base"], "quartio");
    assert_eq!(f.h.github.pull(1).ok_or("pull")?.title, "feat: checkout");
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #1 merge".to_owned())
    );
    assert_eq!(actions(&merged.body), ["run_env:dev-vm"]);

    // No deployment without the explicit confirmation; nothing is recorded.
    let unconfirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(unconfirmed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error_message(&unconfirmed).contains("explicit confirmation"));
    let runs = f
        .req(
            Method::GET,
            "/api/sandboxes/s1/delivery/runs",
            None,
            &f.rita,
        )
        .await?;
    assert_eq!(runs.body, json!([]));
    let read_only = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.rita,
        )
        .await?;
    assert_eq!(read_only.status, StatusCode::FORBIDDEN);
    let out_of_order = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/rollout/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(
        out_of_order.status,
        StatusCode::OK,
        "rollout is standalone: no task is in production yet"
    );
    let early = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert!(
        error_message(&early).contains("environment 'dev-vm' is not done"),
        "{}",
        early.body
    );

    // The confirmed run checks out the integration head and records the command output. A hook
    // an agent planted in the shared clone's `.git` is not executed by the server's checkout.
    let marker = f.repo.join("hook-ran");
    let hook = f.repo.join(".git/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().ok_or("hooks dir")?)?;
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display()))?;
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let started = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["kind"], "command");
    assert!(
        started.body["command"]
            .as_str()
            .unwrap_or("")
            .starts_with("printf")
    );
    let run_id = started.body["run_id"].as_str().ok_or("run id")?.to_owned();
    let run = f.wait_run(&run_id).await?;
    assert_eq!(run["state"], "succeeded", "{run}");
    assert_eq!(run["exit_code"], 0);
    assert!(
        run["output"]
            .as_str()
            .unwrap_or("")
            .contains("deployed hello on dev-vm"),
        "{run}"
    );
    assert_eq!(run["commit"], f.origin_sha("quartio")?.as_str());
    assert!(!marker.exists(), "planted post-checkout hook must not run");
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "env_done");
    assert_eq!(d["environment"], "dev-vm");
    assert_eq!(actions(&d), ["promote"]);

    // A redeploy is allowed (still confirmed) and changes no stage.
    let again = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let rerun = f
        .wait_run(again.body["run_id"].as_str().ok_or("run id")?)
        .await?;
    assert_eq!(rerun["state"], "succeeded", "{rerun}");
    assert_eq!(f.delivery("t1").await?["environment"], "dev-vm");

    // Fast-forward main to the integration head, then confirm the rollout step.
    let quartio = f.origin_sha("quartio")?;
    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(promoted.body["result"], "promoted");
    assert_eq!(promoted.body["sha"], quartio.as_str());
    assert!(
        f.h.github
            .writes()
            .contains(&format!("fast_forward main {quartio}"))
    );
    assert_eq!(f.origin_sha("main")?, quartio);
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "promoted");
    assert_eq!(actions(&d), ["confirm_env:rollout"]);
    let again = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "nothing left to promote"
    );
    let rollout = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/rollout/confirm",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(rollout.status, StatusCode::OK, "{}", rollout.body);
    assert_eq!(f.delivery("t1").await?["stage"], "production_done");
    let runs = f
        .req(
            Method::GET,
            "/api/sandboxes/s1/delivery/runs",
            None,
            &f.rita,
        )
        .await?;
    assert_eq!(runs.body.as_array().map(Vec::len), Some(4));
    assert_eq!(runs.body[0]["kind"], "manual", "newest first");
    Ok(())
}

/// AC-DLV-3: studio — after a merge into quartio, main fast-forwards automatically even with
/// failing checks (none are required); switching the profile to gated disables it.
#[tokio::test]
async fn studio_auto_promotes_until_the_profile_is_gated() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    let wt = f
        .add_task("t1", "Landing page", "feat/1-landing", "quartio")
        .await?;
    commit_file(&wt, "landing.txt", "v1", "Landing")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    f.h.github
        .set_checks(&git(&wt, &["rev-parse", "HEAD"])?, ChecksState::Failure);
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "production_done", "{}", merged.body);
    let quartio = f.origin_sha("quartio")?;
    assert_eq!(
        f.origin_sha("main")?,
        quartio,
        "main fast-forwarded immediately"
    );
    assert!(
        f.h.github
            .writes()
            .contains(&format!("fast_forward main {quartio}"))
    );
    assert!(
        merged.body["notes"]
            .to_string()
            .contains("Automatic promotion"),
        "{}",
        merged.body
    );

    // Gate the profile: one change (auto_promote off) relabels it and stops auto-promotion.
    let mut gated = serde_json::to_value(profile_to_contract(&DeliveryProfile::studio()))?;
    gated["auto_promote"] = json!(false);
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": gated})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(patched.body["profile"]["preset"], "custom");
    assert_eq!(patched.body["profile"]["auto_promote"], false);

    let wt2 = f
        .add_task("t2", "Pricing page", "feat/2-pricing", "quartio")
        .await?;
    commit_file(&wt2, "pricing.txt", "v1", "Pricing")?;
    commit_file(
        &wt2,
        "docs/specs/2026-10-07-pricing.md",
        &valid_spec("t2"),
        "Spec",
    )?;
    let opened = f
        .req(Method::POST, "/api/tasks/t2/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let writes_before = f.h.github.writes().len();
    let merged = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    assert_eq!(actions(&merged.body), ["promote"]);
    // Only the merge and the merged branch's deletion (DLV-007), no promotion.
    let after: Vec<String> = f.h.github.writes()[writes_before..].to_vec();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after[0].starts_with("merge_pull #"), "{after:?}");
    assert_eq!(after[1], "delete_branch feat/2-pricing");
    assert_eq!(f.origin_sha("main")?, quartio, "main did not move");

    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(f.origin_sha("main")?, f.origin_sha("quartio")?);
    assert_eq!(f.delivery("t2").await?["stage"], "production_done");
    Ok(())
}

/// GIT-002: a pull request head that differs from the head that was checked is never merged.
#[tokio::test]
async fn stale_heads_are_refused() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);

    // The agent committed more after the PR was opened: the PR head is stale.
    commit_file(&wt, "feature.txt", "hello again", "Change after review")?;
    let stale = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert!(
        error_message(&stale).contains("head changed"),
        "{}",
        stale.body
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["blocked"][0]["action"], "merge");

    // Someone pushed to the PR branch outside Yulius: still not the checked head.
    git(
        &wt,
        &[
            "push",
            "--quiet",
            "origin",
            "HEAD:refs/heads/feat/1-checkout-api",
        ],
    )?;
    git(&wt, &["reset", "--quiet", "--hard", "HEAD~1"])?;
    let stale = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull"))
    );

    // Heads agree again: the merge goes through at exactly that head.
    git(
        &wt,
        &[
            "pull",
            "--quiet",
            "--ff-only",
            "origin",
            "feat/1-checkout-api",
        ],
    )?;
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(git(&f.origin, &["show", "dev:feature.txt"])?, "hello again");
    Ok(())
}

/// AC-GIT-2, AC-DLV-8 (#139): agents deliver their own work only through the gated delivery
/// tools — a merge needs passed checks and an independent review of the exact head — while reads,
/// task tokens on the REST API and read-only users never merge, promote or deploy.
#[tokio::test]
async fn agents_merge_only_through_gated_tools() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    let wt = f
        .add_task("t1", "Landing page", "feat/1-landing", "quartio")
        .await?;
    commit_file(&wt, "landing.txt", "v1", "Landing")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let main_before = f.origin_sha("main")?;

    // An agent run of the task.
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: TaskId::parse("t1")?,
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: f.h.app.deps.crypto.sha256_hex(b"tok-agent"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    let internal = yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let agent = Creds {
        bearer: Some("tok-agent".into()),
        ..Creds::default()
    };
    let rpc = |method: &str, params: Value| {
        call(
            &internal,
            Method::POST,
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
            &agent,
        )
    };
    let tools = rpc("tools/list", json!({})).await?;
    let names: Vec<String> = tools.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    for tool in [
        "delivery_status",
        "delivery_merge",
        "delivery_deploy",
        "cd_deploy",
    ] {
        assert!(names.contains(&tool.to_owned()), "{tool}");
    }
    // Agents never promote: production is a person's (ADR-0110).
    assert!(!names.iter().any(|n| n.contains("promote")), "{names:?}");
    let delivery_tools = names
        .iter()
        .filter(|n| {
            ["merge", "promote", "deploy", "environment"]
                .iter()
                .any(|w| n.contains(w))
        })
        .count();
    assert_eq!(
        delivery_tools, 3,
        "no other way to merge or deploy: {names:?}"
    );
    let status = rpc(
        "tools/call",
        json!({"name": "delivery_status", "arguments": {}}),
    )
    .await?;
    // The agent reads a few lines (SPEC-086 TOL-004): the stage, the pull request, what people
    // do next and the profile.
    let text = status.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?;
    assert!(
        text.starts_with("Stage: pr_open.\nPull request #1 open"),
        "{text}"
    );
    assert!(text.contains("\nNext for people: merge.\n"), "{text}");
    assert!(text.contains("\nProfile studio: "), "{text}");
    assert!(
        status.body["result"].get("structuredContent").is_none(),
        "Claude Code would show the record instead of the text: {}",
        status.body
    );
    let tool_text = |r: &Resp| {
        r.body["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    // No review, a review of another head, or checks still running: refused.
    let attempt = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {}}),
    )
    .await?;
    assert_eq!(attempt.body["result"]["isError"], true);
    assert!(
        tool_text(&attempt).contains("reviewed_head"),
        "{}",
        attempt.body
    );
    let stale = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": "0000000"}}),
    )
    .await?;
    assert_eq!(stale.body["result"]["isError"], true);
    assert!(
        tool_text(&stale).contains("review the new head"),
        "{}",
        stale.body
    );
    // While the checks still run, the reviewed head's merge is armed, not refused: Yulius
    // merges it once they pass (SPEC-011); nothing is merged yet.
    let pending = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": head}}),
    )
    .await?;
    assert_eq!(pending.body["result"]["isError"], false, "{}", pending.body);
    assert!(
        tool_text(&pending).contains("armed: true"),
        "{}",
        pending.body
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    // A person-verified, production or unknown environment is never run by an agent.
    let deploy = rpc(
        "tools/call",
        json!({"name": "delivery_deploy", "arguments": {"environment": "rollout"}}),
    )
    .await?;
    assert_eq!(deploy.body["result"]["isError"], true);
    let direct = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/delivery/merge",
        None,
        &agent,
    )
    .await?;
    assert_eq!(
        direct.status,
        StatusCode::UNAUTHORIZED,
        "task tokens are not API credentials"
    );

    // Every read path, repeatedly, by every reader.
    for who in [&f.rita, &f.bob, &f.alice] {
        for path in [
            "/api/tasks/t1/delivery",
            "/api/tasks/t1",
            "/api/sandboxes/s1/git",
            "/api/sandboxes/s1/delivery/runs",
            "/api/sandboxes/s1",
        ] {
            let r = f.req(Method::GET, path, None, who).await?;
            assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        }
    }
    // Read-only users cannot act.
    for path in [
        "/api/tasks/t1/delivery/merge",
        "/api/sandboxes/s1/delivery/promote",
        "/api/sandboxes/s1/delivery/environments/x/runs",
    ] {
        let r = f
            .req(Method::POST, path, Some(json!({"confirm": true})), &f.rita)
            .await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }

    assert_eq!(
        f.h.github.writes(),
        ["create_pull #1 quartio<-feat/1-landing"]
    );
    assert_eq!(f.origin_sha("main")?, main_before);
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // On green, with the reviewed head, the agent merges with its creator's authority (and then
    // the explicit profile policy promotes).
    f.h.github.set_checks(&head, ChecksState::Success);
    let merged = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": &head[..12], "review_summary": "approve: code and tests checked"}}),
    )
    .await?;
    assert_eq!(merged.body["result"]["isError"], false, "{}", merged.body);
    assert_eq!(f.delivery("t1").await?["pr"]["state"], "merged");
    assert_eq!(f.origin_sha("main")?, f.origin_sha("quartio")?);
    // F 2026-10-11 (AC-GIT-3): on GitHub the creator merged and promoted, not the App.
    assert_eq!(f.h.github.actors_of("merge_pull"), ["bob"]);
    assert_eq!(f.h.github.actors_of("fast_forward"), ["bob"]);

    // Follow-up work becomes a ready issue (Definition of Ready), never a vague one.
    let vague = rpc(
        "tools/call",
        json!({"name": "issue_create", "arguments": {"title": "Fix things", "body": "Stuff.", "labels": ["type: bug"]}}),
    )
    .await?;
    assert_eq!(vague.body["result"]["isError"], true);
    assert!(tool_text(&vague).contains("not ready"), "{}", vague.body);
    let ready = rpc(
        "tools/call",
        json!({"name": "issue_create", "arguments": {
            "title": "Flaky landing test",
            "body": "Fails 1 in 20 runs.\n\n## Acceptance criteria\n- [ ] Passes 100 runs under load",
            "labels": ["type: bug", "priority: medium", "area: frontend"]
        }}),
    )
    .await?;
    assert_eq!(ready.body["result"]["isError"], false, "{}", ready.body);
    assert!(
        f.h.github
            .writes()
            .iter()
            .any(|w| w == "create_issue #101 Flaky landing test"),
        "{:?}",
        f.h.github.writes()
    );
    assert_eq!(
        f.h.github.actors_of("create_issue"),
        ["bob"],
        "the issue and its labels are the creator's"
    );
    Ok(())
}

/// Reconciliation: GitHub outages never change the recorded stage; a pull request merged
/// outside Yulius moves the task forward on read; a promotion merged on GitHub is recorded by
/// the next promotion request; a broken clone degrades the Git page to notes.
#[tokio::test]
async fn reads_reconcile_with_github_and_degrade_gracefully() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);

    // Merged on GitHub directly, outside Yulius.
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        1,
        "squash",
        &head,
    )
    .await?;

    // GitHub unreachable (expired user token, broken App credentials): the page answers with
    // a note and the stage stays as recorded.
    let crypto = &f.h.app.deps.crypto;
    let expired = |access: &str| -> BoxResult<GitHubTokenRecord> {
        Ok(GitHubTokenRecord {
            user_id: UserId::parse("u-bob")?,
            access: crypto.seal(access.as_bytes())?,
            refresh: None,
            expires_at: Some(Timestamp(1)),
            refresh_expires_at: None,
        })
    };
    f.h.store
        .save_github_token(&expired("token-bob")?, Timestamp(2))
        .await?;
    f.h.store
        .set_github_app(&Sealed(vec![1, 2, 3]), "Quartio-Studio", "org")
        .await?;
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "pr_open", "{d}");
    assert!(
        d["notes"].to_string().contains("GitHub is not available"),
        "{d}"
    );
    let app_cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c", "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    f.h.store
        .set_github_app(
            &crypto.seal(app_cfg.to_string().as_bytes())?,
            "Quartio-Studio",
            "org",
        )
        .await?;
    f.h.store
        .save_github_token(
            &GitHubTokenRecord {
                expires_at: None,
                ..expired("token-bob")?
            },
            Timestamp(3),
        )
        .await?;

    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "merged_integration", "{d}");
    assert_eq!(actions(&d), ["promote"]);

    // The promotion pull request is merged on GitHub; the next promote records it.
    let opened = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(opened.body["result"], "pr_opened", "{}", opened.body);
    let head = f.h.github.pull(2).ok_or("pull 2")?.head_sha;
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        2,
        "merge",
        &head,
    )
    .await?;
    let recorded = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(recorded.status, StatusCode::OK, "{}", recorded.body);
    assert_eq!(recorded.body["result"], "promoted");
    assert_eq!(recorded.body["sha"], f.origin_sha("main")?.as_str());
    assert_eq!(f.delivery("t1").await?["stage"], "production_done");
    let nothing = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(nothing.status, StatusCode::CONFLICT);
    assert!(error_message(&nothing).contains("nothing to promote"));

    // The sandbox clone is missing: the Git page still answers, with notes.
    std::fs::remove_dir_all(&f.repo)?;
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["recent_commits"], json!([]));
    assert!(
        page.body["notes"].to_string().contains("not cloned"),
        "{}",
        page.body
    );
    Ok(())
}

/// #35: a task ready for delivery opens its pull request by itself — with its creator's GitHub
/// account, after the branch is pushed — while open steps or uncommitted work keep it waiting;
/// a merge on GitHub then moves the stage in the background without anyone reading the task.
#[tokio::test]
async fn a_ready_task_opens_its_pull_request_by_itself() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let t1 = TaskId::parse("t1")?;
    let pulls_opened = || {
        f.h.github
            .writes()
            .iter()
            .filter(|w| w.starts_with("create_pull"))
            .count()
    };

    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "a step is still open");

    set_steps(&f, "t1", &["done", "done"]).await?;
    std::fs::write(wt.join("scratch.txt"), "work in progress")?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "uncommitted work");
    std::fs::remove_file(wt.join("scratch.txt"))?;

    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "{:?}", f.h.github.writes());
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "pr_open");
    assert_eq!(d["pr"]["base"], "dev");
    assert_eq!(d["pr"]["head"], "feat/1-checkout-api");
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head, "pushed first");
    // F 2026-10-11 (AC-GIT-3): the task's creator pushes and opens it, never the App.
    assert_eq!(
        f.pushes(),
        ["feat/1-checkout-api->feat/1-checkout-api token=token-bob"]
    );
    assert_eq!(f.h.github.actors_of("create_pull"), ["bob"]);
    // MSR-002 (SPEC-028): the opened pull request is said in the terminal, not by message.
    assert!(f.h.store.task_messages(&t1, 10).await?.is_empty());
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "one pull request per task");

    // #68: commits made after the pull request opened are pushed to it at the next clean Stop.
    commit_file(&wt, "follow-up.txt", "more", "Follow-up")?;
    let follow_up = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/1-checkout-api")?,
        follow_up,
        "the open pull request gets the new commits"
    );
    assert_eq!(pulls_opened(), 1, "no second pull request");
    assert_eq!(
        f.pushes().last().map(String::as_str),
        Some("feat/1-checkout-api->feat/1-checkout-api token=token-bob"),
        "the creator pushes the follow-up too"
    );

    // MSR-002 (SPEC-028): a conflict, red checks and green ones message nobody; `yulius wait`
    // and `delivery_status` say them.
    let set_mergeable = |mergeable: Option<bool>| {
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            pull.mergeable = mergeable;
        }
    };
    set_mergeable(Some(false));
    f.h.app.reconcile_deliveries().await;
    set_mergeable(Some(true));
    f.h.github.set_checks(&follow_up, ChecksState::Failure);
    f.h.app.reconcile_deliveries().await;
    f.h.github.set_checks(&follow_up, ChecksState::Success);
    f.h.app.reconcile_deliveries().await;
    assert!(f.h.store.task_messages(&t1, 50).await?.is_empty());

    // Merged on GitHub: the background round follows it; nobody read the task meanwhile.
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        1,
        "squash",
        &follow_up,
    )
    .await?;
    f.h.app.reconcile_deliveries().await;
    let record = f.h.store.delivery(&t1).await?.ok_or("delivery row")?;
    assert_eq!(record.stage, DeliveryStage::MergedIntegration);
    // #68: the merged pull request stays on its task, so the task stays "Done · merged".
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    let row = &page.body["tasks"][0];
    assert_eq!(row["pr"]["number"], 1, "{}", page.body);
    assert_eq!(row["pr"]["state"], "merged");
    Ok(())
}

/// DSC-013 (#139): features and fixes need a spec, documentation does not. A task without a
/// workflow that only changed documents opens its pull request with no spec and no plan; once it
/// changes code, it needs the spec again.
#[tokio::test]
async fn a_docs_only_task_needs_no_spec_to_deliver() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Fix the README", "docs/1-readme", "dev")
        .await?;
    let t1 = TaskId::parse("t1")?;
    let pulls_opened = || {
        f.h.github
            .writes()
            .iter()
            .filter(|w| w.starts_with("create_pull"))
            .count()
    };
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "nothing changed yet");

    commit_file(
        &wt,
        "docs/setup.md",
        "# Setup\n",
        "docs: describe the setup",
    )?;
    let task = f.h.store.task(&t1).await?.ok_or("task")?;
    assert!(!f.h.app.spec_required(&task).await?, "documentation only");
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "{:?}", f.h.github.writes());

    let t2_wt = f
        .add_task("t2", "Checkout API", "feat/2-checkout-api", "dev")
        .await?;
    commit_file(&t2_wt, "docs/checkout.md", "# Checkout\n", "docs")?;
    commit_file(
        &t2_wt,
        "src/checkout.rs",
        "fn main() {}\n",
        "feat: checkout",
    )?;
    let t2 = TaskId::parse("t2")?;
    let task = f.h.store.task(&t2).await?.ok_or("task")?;
    assert!(f.h.app.spec_required(&task).await?, "code changed");
    f.h.app.auto_open_pr(&t2).await;
    assert_eq!(pulls_opened(), 1, "a code change without a spec waits");
    Ok(())
}

// ---------------------------------------------------------------- merge conflicts (SPEC-083)

/// A live run of `task` whose agent calls the internal endpoints with the returned credentials.
async fn live_agent(f: &Fixture, task: &str, token: &str) -> BoxResult<Creds> {
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse(&format!("a-{task}"))?,
            task_id: TaskId::parse(task)?,
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: f.h.app.deps.crypto.sha256_hex(token.as_bytes()),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(Creds {
        bearer: Some(token.into()),
        ..Creds::default()
    })
}

/// The internal router the agents call (MCP, `yulius wait`).
fn internal(f: &Fixture) -> axum::Router {
    yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))))
}

/// One MCP tool call of an agent: its result (`isError`, `content`, and the tool's record as
/// `structuredContent`).
async fn agent_tool(f: &Fixture, agent: &Creds, name: &str, arguments: Value) -> BoxResult<Value> {
    let r = call(
        &internal(f),
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments,
                "_meta": {"yulius/structured": true}}})),
        agent,
    )
    .await?;
    Ok(r.body["result"].clone())
}

/// The record a successful tool call answered.
fn answer(result: &Value) -> BoxResult<Value> {
    assert_eq!(result["isError"], false, "{result}");
    if let Some(record) = result.get("structuredContent") {
        return Ok(record.clone());
    }
    Ok(serde_json::from_str(
        result["content"][0]["text"].as_str().unwrap_or("null"),
    )?)
}

/// `delivery_merge` of `head`, reviewed, by `agent`: its answer.
async fn arm_merge(f: &Fixture, agent: &Creds, head: &str) -> BoxResult<Value> {
    answer(
        &agent_tool(
            f,
            agent,
            "delivery_merge",
            json!({"reviewed_head": head, "review_summary": "approve"}),
        )
        .await?,
    )
}

/// `yulius wait` of `agent` at `head`: the state now.
async fn wait_state(f: &Fixture, agent: &Creds, head: &str) -> BoxResult<Value> {
    let r = call(
        &internal(f),
        Method::POST,
        "/internal/agent/wait",
        Some(json!({"head": head, "since": Value::Null, "timeout_secs": 3})),
        agent,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// `record_number` of `agent`: the number it gave out.
async fn record_number(f: &Fixture, agent: &Creds, args: Value) -> BoxResult<Value> {
    answer(&agent_tool(f, agent, "record_number", args).await?)
}

/// SPEC-083 NUM-001 to NUM-003 (81 decision records renamed in 36 of 74 sessions, F 2026-10-13):
/// `record_number` gives each task a number past the files on the integration branch, in every
/// live task's checkout (uncommitted ones too) and every number given out before; asked again
/// before the file exists, it gives the same number back.
#[tokio::test]
async fn parallel_tasks_get_their_own_record_numbers() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    advance_base(
        &f,
        "quartio",
        "docs/decisions/0001-first.md",
        "# 0001. First\n",
    )?;
    advance_base(
        &f,
        "quartio",
        "docs/decisions/0002-second.md",
        "# 0002. Second\n",
    )?;
    let wt1 = f
        .add_task("t1", "Ownership", "feat/1-ownership", "quartio")
        .await?;
    let wt2 = f
        .add_task("t2", "Counts", "feat/2-counts", "quartio")
        .await?;
    let one = live_agent(&f, "t1", "tok-1").await?;
    let two = live_agent(&f, "t2", "tok-2").await?;
    // Task 2 counted the files itself and has not committed its record yet.
    std::fs::write(
        wt2.join("docs/decisions/0003-counts.md"),
        "# 0003. Counts\n",
    )?;
    // A record merged after both tasks started.
    advance_base(
        &f,
        "quartio",
        "docs/decisions/0004-merged.md",
        "# 0004. Merged\n",
    )?;

    let first = record_number(&f, &one, json!({"dir": "docs/decisions/"})).await?;
    assert_eq!(first["number"], "0005", "{first}");
    assert_eq!(first["file"], "docs/decisions/0005-<slug>.md");
    assert_eq!(first["again"], false);
    let again = record_number(&f, &one, json!({"dir": "docs/decisions"})).await?;
    assert_eq!(
        (again["number"].as_str(), again["again"].as_bool()),
        (Some("0005"), Some(true)),
        "asked again before the file exists: {again}"
    );
    let second = record_number(&f, &two, json!({"dir": "docs/decisions"})).await?;
    assert_eq!(
        second["number"], "0006",
        "past task 1's reservation: {second}"
    );
    // Task 1 wrote its record; its next one gets the next free number.
    std::fs::write(
        wt1.join("docs/decisions/0005-ownership.md"),
        "# 0005. Ownership\n",
    )?;
    let next = record_number(&f, &one, json!({"dir": "docs/decisions"})).await?;
    assert_eq!(next["number"], "0007", "{next}");

    // Another series in a directory, and directories with none or outside the repository.
    advance_base(&f, "quartio", "docs/notes/SPEC-009-search.md", "spec\n")?;
    advance_base(&f, "quartio", "docs/notes/0100-misc.md", "note\n")?;
    let spec = record_number(&f, &two, json!({"dir": "docs/notes", "prefix": "SPEC-"})).await?;
    assert_eq!(spec["number"], "SPEC-010", "{spec}");
    for (args, why) in [
        (json!({"dir": "src"}), "no numbered files"),
        (json!({"dir": "../x"}), "outside the repository"),
        (json!({}), "no directory"),
        (
            json!({"dir": "docs/notes", "prefix": "ADR-"}),
            "no such series",
        ),
    ] {
        let refused = agent_tool(&f, &two, "record_number", args).await?;
        assert_eq!(refused["isError"], true, "{why}: {refused}");
    }
    Ok(())
}

/// SPEC-083 NUM-004: a task's spec number is given out like any record number, so it never takes
/// a number another task reserved (SPEC-020 SPC-001 counted only its own checkout and the spec
/// paths of other tasks).
#[tokio::test]
async fn a_spec_number_is_never_one_another_task_reserved() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    advance_base(&f, "quartio", "docs/specs/SPEC-001-shop.md", "spec\n")?;
    f.add_task("t1", "Ownership", "feat/1-ownership", "quartio")
        .await?;
    let wt2 = f
        .add_task("t2", "Counts", "feat/2-counts", "quartio")
        .await?;
    let one = live_agent(&f, "t1", "tok-1").await?;
    let reserved = record_number(&f, &one, json!({"dir": "docs/specs", "prefix": "SPEC-"})).await?;
    assert_eq!(reserved["number"], "SPEC-002");
    let t2 = f.h.store.task(&TaskId::parse("t2")?).await?.ok_or("t2")?;
    f.h.app.allocate_spec_path(&t2, &wt2).await;
    let t2 = f.h.store.task(&t2.id).await?.ok_or("t2")?;
    assert_eq!(
        t2.spec_path.as_deref(),
        Some("docs/specs/SPEC-003-counts.md")
    );
    Ok(())
}

/// SPEC-083 REV-001 (33 of 57 merges spent a review round on dev merges or renumbering,
/// F 2026-10-13): the review of a head carries over Git's own merges of the integration branch
/// into it; new work after it needs a new review.
#[tokio::test]
async fn a_review_carries_over_clean_merges_of_the_integration_branch() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    advance_base(f, "quartio", "other.txt", "merged meanwhile")?;
    git(&t.wt, &["fetch", "--quiet", "origin"])?;
    git(&t.wt, &["merge", "--quiet", "--no-edit", "origin/quartio"])?;
    let merged = git(&t.wt, &["rev-parse", "HEAD"])?;
    assert_ne!(merged, t.head);
    // `delivery_merge` pushes the merge (DMG-001); its checks still run, so the merge arms.
    f.h.github.set_checks(&merged, ChecksState::Pending);
    let armed = t.merge_at(&t.head).await?;
    assert_eq!(
        armed["isError"], false,
        "the review of {} carries over: {armed}",
        t.head
    );
    let state = t.wait_at(&merged).await?;
    assert_eq!(state["merge_armed"], true, "{state}");

    let refused_text = |r: &Value| {
        r["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("review the new head"))
    };
    commit_file(&t.wt, "landing.txt", "v2", "Landing v2")?;
    let refused = t.merge_at(&t.head).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused_text(&refused), "new work needs a review: {refused}");

    // Unreviewed work merged as if it were the integration branch (a moved remote-tracking ref)
    // does not count: what the integration branch holds is fetched.
    git(&t.wt, &["reset", "--quiet", "--hard", &merged])?;
    git(
        &t.wt,
        &["checkout", "--quiet", "-b", "side", "origin/quartio"],
    )?;
    commit_file(&t.wt, "side.txt", "unreviewed", "Side work")?;
    let side = git(&t.wt, &["rev-parse", "HEAD"])?;
    git(&t.wt, &["checkout", "--quiet", "feat/1-landing"])?;
    git(&t.wt, &["update-ref", "refs/remotes/origin/quartio", &side])?;
    git(&t.wt, &["merge", "--quiet", "--no-edit", "origin/quartio"])?;
    let sneaked = t.merge_at(&t.head).await?;
    assert_eq!(sneaked["isError"], true, "{sneaked}");
    assert!(refused_text(&sneaked), "{sneaked}");
    Ok(())
}

/// SPEC-083 MRG-001: in a repository that commits no `.gitattributes`, the changes log every task
/// adds an entry to conflicts when Yulius brings an armed branch up to date (DLV-203), and its
/// merge is put on hold for the agent. Once Yulius keeps the union merge for the shared logs (at
/// every task start), the same update merges both entries.
#[tokio::test]
async fn the_changes_log_merges_by_union_in_any_repository() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    let log = "# Changes\n\n## 2026-10-13\n\n- First change.\n";
    advance_base(&f, "quartio", "docs/changes.md", log)?;
    let wt = f
        .add_task("t1", "Landing page", "feat/1-landing", "quartio")
        .await?;
    git(&wt, &["config", "user.name", "Bob"])?;
    git(&wt, &["config", "user.email", "bob@yulius.test"])?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    commit_file(
        &wt,
        "docs/changes.md",
        &log.replace(
            "## 2026-10-13\n\n",
            "## 2026-10-13\n\n- The landing page.\n",
        ),
        "Landing",
    )?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let agent = live_agent(&f, "t1", "tok-1").await?;
    f.h.store
        .transition_task(
            &TaskId::parse("t1")?,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    // Another task's entry merged at the top of the same section.
    advance_base(
        &f,
        "quartio",
        "docs/changes.md",
        &log.replace("## 2026-10-13\n\n", "## 2026-10-13\n\n- Search.\n"),
    )?;
    let sandbox = SandboxId::parse("s1")?;

    // Without the attribute: on hold, the worktree left as it was.
    assert_eq!(arm_merge(&f, &agent, &head).await?["armed"], true);
    f.h.app.merge_armed(&sandbox).await;
    assert_eq!(git(&wt, &["rev-parse", "HEAD"])?, head, "aborted");
    let held = wait_state(&f, &agent, &head).await?;
    assert!(
        held["merge_outcome"]
            .as_str()
            .is_some_and(|o| o.contains("on hold") && o.contains("`docs/changes.md`")),
        "{held}"
    );

    // Kept by Yulius: the same update merges both entries and the merge stays armed.
    f.h.app.keep_union_merges(&wt).await;
    assert_eq!(arm_merge(&f, &agent, &head).await?["armed"], true);
    f.h.app.merge_armed(&sandbox).await;
    let moved = git(&wt, &["rev-parse", "HEAD"])?;
    assert_ne!(moved, head, "Yulius merged the base in");
    let merged_log = std::fs::read_to_string(wt.join("docs/changes.md"))?;
    assert!(
        merged_log.contains("- The landing page.") && merged_log.contains("- Search."),
        "{merged_log}"
    );
    assert_eq!(wait_state(&f, &agent, &moved).await?["merge_armed"], true);
    assert!(
        !wt.join(".gitattributes").exists(),
        "nothing committed to the repository"
    );
    Ok(())
}

/// #28 (UI-031): the task review reads the worktree's committed, uncommitted and untracked
/// changes against the merge base with `origin/dev`, and the branch's commits, with read access;
/// a symbolic link is listed but never followed.
#[tokio::test]
async fn task_review_shows_changes_and_commits() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello\nworld\n", "Add feature")?;
    std::fs::write(wt.join("feature.txt"), "hello\nthere\nworld\n")?;
    std::fs::write(wt.join("notes.md"), "draft\n")?;
    std::os::unix::fs::symlink("/etc/passwd", wt.join("outside"))?;

    let changes = f
        .req(Method::GET, "/api/tasks/t1/changes", None, &f.rita)
        .await?;
    assert_eq!(changes.status, StatusCode::OK, "{}", changes.body);
    let c = &changes.body;
    assert_eq!(c["base"], "dev");
    assert_eq!(c["head"], "feat/1-checkout-api");
    assert_eq!(c["ahead"], 1);
    assert_eq!(c["behind"], 0);
    assert_eq!(c["uncommitted"], 3, "{c}");
    assert_eq!(c["truncated"], false);
    let files = c["files"].as_array().ok_or("files")?;
    let file = |path: &str| files.iter().find(|f| f["path"] == path).cloned();
    let feature = file("feature.txt").ok_or("feature.txt")?;
    assert_eq!(feature["status"], "added", "new since the merge base");
    let added: Vec<&str> = feature["hunks"][0]["lines"]
        .as_array()
        .ok_or("lines")?
        .iter()
        .filter(|l| l["kind"] == "add")
        .filter_map(|l| l["text"].as_str())
        .collect();
    assert_eq!(
        added,
        ["hello", "there", "world"],
        "uncommitted work included"
    );
    let notes = file("notes.md").ok_or("notes.md")?;
    assert_eq!(notes["status"], "added");
    assert_eq!(notes["hunks"][0]["lines"][0]["text"], "draft");
    let link = file("outside").ok_or("outside")?;
    assert!(
        link["hunks"].as_array().is_some_and(Vec::is_empty),
        "a symlink is not followed"
    );
    assert!(
        !changes.body.to_string().contains("root:"),
        "nothing outside the worktree"
    );

    let commits = f
        .req(Method::GET, "/api/tasks/t1/commits", None, &f.rita)
        .await?;
    assert_eq!(commits.status, StatusCode::OK, "{}", commits.body);
    let list = commits.body.as_array().ok_or("commits")?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["subject"], "Add feature");
    assert_eq!(list[0]["files"], 1);
    assert_eq!(list[0]["insertions"], 2);
    assert!(list[0]["at"].as_str().is_some_and(|t| t.ends_with('Z')));

    let anonymous = f
        .req(
            Method::GET,
            "/api/tasks/t1/changes",
            None,
            &Creds::default(),
        )
        .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// #111 (`F 2026-10-10`): the GitHub App may not read every check source. What it can read
/// counts; checks that can't be read never count as passed; the missing permission is said once,
/// in plain words, with the App's permission page for instance administrators only; GitHub
/// errors are not prefixed "GitHub:" twice.
#[tokio::test]
async fn unreadable_check_sources_are_said_once_and_never_count_as_passed() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner = superadmin(&f.h).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;

    // The installation of #111: Checks but no Commit statuses. The check runs decide.
    f.h.github.set_checks_report(
        &head,
        ChecksReport {
            state: ChecksState::Success,
            unreadable: vec![ChecksSource::CommitStatuses],
        },
    );
    let expected = "Yulius can't read commit statuses for Quartio-Studio/shop: the GitHub App \
                    needs the 'Commit statuses: read' permission. Results come from check runs only.";
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["pulls"][0]["checks"], "success");
    assert_eq!(page.body["notes"], json!([]), "no warning note");
    let access = &page.body["checks_access"];
    assert_eq!(access["repo"], "Quartio-Studio/shop");
    assert_eq!(access["missing"], json!(["Commit statuses"]));
    assert_eq!(access["read_from"], json!(["check runs"]));
    assert_eq!(access["message"], expected);
    assert_eq!(access["settings_url"], Value::Null, "not an administrator");
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(
        page.body["checks_access"]["settings_url"],
        "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-test/permissions"
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "success");
    assert_eq!(d["checks_access"]["message"], expected);
    assert_eq!(actions(&d), ["merge"]);

    // Every source read: nothing to say.
    f.h.github.set_checks(&head, ChecksState::Success);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(page.body["checks_access"], Value::Null);

    // Nothing readable: unknown, never green. The merge waits and says why.
    f.h.github.set_checks_report(
        &head,
        ChecksReport {
            state: ChecksState::Unknown,
            unreadable: vec![
                ChecksSource::CheckRuns,
                ChecksSource::CommitStatuses,
                ChecksSource::WorkflowRuns,
            ],
        },
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "unknown");
    assert!(actions(&d).is_empty(), "{d}");
    assert_eq!(d["blocked"][0]["action"], "merge");
    assert!(
        d["blocked"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("can't be read") && r.contains("don't count as passed")),
        "{d}"
    );
    assert_eq!(
        d["checks_access"]["missing"],
        json!(["Checks", "Commit statuses"])
    );
    assert_eq!(d["checks_access"]["read_from"], json!([]));
    assert_eq!(
        d["checks_access"]["message"],
        "Yulius can't read check results for Quartio-Studio/shop: the GitHub App needs the \
         'Checks: read' and 'Commit statuses: read' permissions. Checks show as unknown, and \
         merges that require them wait."
    );
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("can't be read"),
        "{}",
        refused.body
    );
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull")),
        "nothing merged"
    );

    // Any other GitHub failure: unknown too (not "running"), one note naming GitHub once.
    if let Ok(mut slot) = f.h.github.checks_error.lock() {
        *slot = Some(AppError::Upstream(
            "GitHub: GitHub refused the request (HTTP 403): Resource protected by organization \
             SAML enforcement."
                .into(),
        ));
    }
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "unknown");
    assert_eq!(
        d["notes"],
        json!([
            "Checks can't be read: GitHub refused the request (HTTP 403): Resource protected by \
             organization SAML enforcement."
        ])
    );
    assert_eq!(d["checks_access"], Value::Null);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.body["pulls"][0]["checks"], "unknown");
    let notes = page.body["notes"].to_string();
    assert!(
        notes.contains("Checks can't be read: GitHub refused"),
        "{notes}"
    );
    assert!(!notes.contains("GitHub: GitHub"), "{notes}");
    Ok(())
}

/// The password superadmin: no GitHub identity, admin on every sandbox (ACC-012).
async fn superadmin(h: &support::Harness) -> BoxResult<Creds> {
    let user = UserRecord {
        id: UserId::parse("u-root")?,
        github_login: None,
        github_id: None,
        display_name: "Superadmin".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$synthetic$synthetic".into()),
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&user).await?;
    let token = h
        .app
        .issue_api_token(&user, "test", yulius_domain::permission::TokenScope::Cli)
        .await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

/// ACC-012 (F 2026-10-09): the superadmin without a GitHub identity reads a task's delivery and
/// opens its pull request through the App, as Yulius does for a ready task; merging and
/// releasing act as a person on GitHub, so those two actions are refused with a message saying
/// so (403) — never as a signed-out session (401), and the rest of the page keeps working.
#[tokio::test]
async fn the_superadmin_delivers_through_the_app_and_is_told_what_needs_github() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner = superadmin(&f.h).await?;
    // The superadmin's own task: only a task's owner delivers it (SPEC-070).
    let wt = f
        .add_task_by("t1", "Checkout API", "feat/1-checkout-api", "dev", "u-root")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;

    let d = f
        .req(Method::GET, "/api/tasks/t1/delivery", None, &owner)
        .await?;
    assert_eq!(d.status, StatusCode::OK, "{}", d.body);
    assert_eq!(actions(&d.body), ["open_pr"]);
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &owner)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["stage"], "pr_open");
    assert_eq!(
        f.h.github.pull_author(1).as_deref(),
        Some("inst"),
        "the App installation opened the pull request"
    );
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head);
    f.h.github.set_checks(&head, ChecksState::Success);

    let merge = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &owner)
        .await?;
    assert_eq!(merge.status, StatusCode::FORBIDDEN, "{}", merge.body);
    assert!(
        error_message(&merge).contains("Merging a pull request acts as a person on GitHub"),
        "{}",
        merge.body
    );
    let release = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &owner,
        )
        .await?;
    assert_eq!(release.status, StatusCode::FORBIDDEN, "{}", release.body);
    assert!(
        error_message(&release).contains("Releasing to main acts as a person on GitHub"),
        "{}",
        release.body
    );
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull")),
        "nothing was merged"
    );

    // Still signed in, and the Git page answers.
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["tasks"][0]["pr"]["number"], 1);

    // Bob may not merge the superadmin's task; once it is handed to him (SPEC-070), he merges
    // the App's pull request as before.
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let (task, root, bob) = (
        TaskId::parse("t1")?,
        UserId::parse("u-root")?,
        UserId::parse("u-bob")?,
    );
    assert!(
        f.h.store
            .request_control(&task, &bob, f.h.app.now())
            .await?
    );
    assert!(
        f.h.store
            .pass_control(&task, &root, &bob, f.h.app.now())
            .await?
    );
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    Ok(())
}

/// F 2026-10-11 (AC-GIT-3): a task whose creator has no usable GitHub token — here the
/// superadmin, with no GitHub identity — still delivers: Yulius pushes and opens its pull request
/// as the App, and its agent merges as the App instead of being refused; the task's terminal
/// says once why the App appears.
#[tokio::test]
async fn a_task_without_a_github_creator_delivers_as_the_app() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    superadmin(&f.h).await?;
    let wt = f
        .add_task_by("t1", "Checkout API", "feat/1-checkout-api", "dev", "u-root")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let t1 = TaskId::parse("t1")?;
    let mut watch = f.h.app.runtime(&t1).hub.watch();

    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    assert_eq!(
        f.pushes(),
        ["feat/1-checkout-api->feat/1-checkout-api token=inst"]
    );
    assert_eq!(f.h.github.actors_of("create_pull"), ["inst"]);

    // Its agent merges on green, as the App.
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: t1.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: f.h.app.deps.crypto.sha256_hex(b"tok-agent"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    let internal = yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let merged = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "delivery_merge",
                "arguments": {"reviewed_head": head, "review_summary": "approve"}
            }}),
        ),
        &Creds {
            bearer: Some("tok-agent".into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(merged.body["result"]["isError"], false, "{}", merged.body);
    assert_eq!(f.h.github.actors_of("merge_pull"), ["inst"]);
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");

    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame
            && text.contains("GitHub App")
        {
            said.push(text);
        }
    }
    assert_eq!(said.len(), 1, "said once: {said:?}");
    assert!(said[0].contains("no linked GitHub account"), "{said:?}");
    Ok(())
}

/// #147: Home's cards and the sandbox tabs count "needs you" with the sidebar's rule (`taskInfo`
/// in the web UI): Your turn, held prompts, blocked chains and failing checks count; a paused or
/// ended task does not; a tested pull request answers for a finished task once its plan is
/// complete (#139). The pull request facts are what the Git overview last read.
#[tokio::test]
async fn needs_you_counts_follow_the_sidebar_rule() -> TestResult {
    use yulius_app::intake::{ChainRecord, HoldRecord, hold_state};
    use yulius_app::ports::store_ext::IntakeStore;
    use yulius_domain::intake::PromptClass;

    let f = fixture(&DeliveryProfile::standard()).await?;
    let now = Timestamp(5);
    let set = async |id: &str, status: TaskStatus, activity: Option<Activity>| -> BoxResult<()> {
        f.h.store
            .transition_task(&TaskId::parse(id)?, None, status, activity, now)
            .await?;
        Ok(())
    };
    let hold = |id: &str| -> BoxResult<HoldRecord> {
        Ok(HoldRecord {
            id: format!("hold-{id}"),
            task_id: TaskId::parse(id)?,
            class: PromptClass::NewScope,
            prompt: "Also add dark mode.".into(),
            prompt_sha256: "0".repeat(64),
            percent: 90,
            typed_by: None,
            state: hold_state::PENDING.into(),
            created_at: now,
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
    };
    let needs = async || -> BoxResult<u64> {
        let r = f.req(Method::GET, "/api/sandboxes", None, &f.bob).await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let sandbox = r
            .body
            .as_array()
            .and_then(|list| list.iter().find(|s| s["id"] == "s1"))
            .ok_or("s1")?;
        sandbox["counts"]["needs_attention"]
            .as_u64()
            .ok_or_else(|| format!("{sandbox}").into())
    };

    // t1 finished its turn with an open pull request and a complete plan.
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    set("t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    // Your turn; a held prompt; a chain whose predecessor ended otherwise.
    f.add_task("t2", "Your turn", "feat/2-your-turn", "dev")
        .await?;
    set("t2", TaskStatus::Running, Some(Activity::Waiting)).await?;
    f.add_task("t3", "Held prompt", "feat/3-held", "dev")
        .await?;
    set("t3", TaskStatus::Running, Some(Activity::Working)).await?;
    f.h.store.insert_hold(&hold("t3")?).await?;
    f.add_task("t4", "Blocked chain", "feat/4-chain", "dev")
        .await?;
    set("t4", TaskStatus::Chained, None).await?;
    f.h.store
        .insert_chain(&ChainRecord {
            task_id: TaskId::parse("t4")?,
            after_task_id: Some(TaskId::parse("t2")?),
            after_number: 2,
            blocked_reason: Some("#2 stopped".into()),
            created_at: now,
            rule: None,
        })
        .await?;
    // Nobody needs to act: paused for capacity, or ended (even with a held prompt).
    f.add_task("t5", "Paused", "feat/5-paused", "dev").await?;
    set(
        "t5",
        TaskStatus::WaitingCapacity,
        Some(Activity::NeedsAttention),
    )
    .await?;
    f.add_task("t6", "Closed", "feat/6-closed", "dev").await?;
    f.h.store.insert_hold(&hold("t6")?).await?;

    // Before any Git overview read, t1's pull request is not known: Your turn.
    assert_eq!(needs().await?, 4);

    // Green checks, read by the Git overview: t1 is Done.
    f.h.github.set_checks(&head, ChecksState::Success);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(needs().await?, 3);

    // A step opened again keeps it out of Done: its own state (Your turn) shows.
    set_steps(&f, "t1", &["done", "pending"]).await?;
    assert_eq!(needs().await?, 4);

    // Failing checks need a person whatever the plan says.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.h.github.set_checks(&head, ChecksState::Failure);
    f.req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(needs().await?, 4);

    // Working again: the pull request no longer speaks for it.
    set("t1", TaskStatus::Running, Some(Activity::Working)).await?;
    assert_eq!(needs().await?, 3);
    Ok(())
}

/// #170: a sandbox's counters split the sidebar's groups with its rule (`taskInfo`): "working"
/// plus "queued" is the Working group, "queued" its queued and chained tasks, and "needs you" the
/// Needs you group. A Done task, a task being stopped and an ended one count nowhere.
#[tokio::test]
async fn working_and_queued_counts_split_the_sidebar_working_group() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let set = async |id: &str, status: TaskStatus, activity: Option<Activity>| -> BoxResult<()> {
        f.h.store
            .transition_task(&TaskId::parse(id)?, None, status, activity, Timestamp(5))
            .await?;
        Ok(())
    };
    // (needs you, working, queued)
    let counts = async || -> BoxResult<(u64, u64, u64)> {
        let r = f.req(Method::GET, "/api/sandboxes", None, &f.bob).await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let sandbox = r
            .body
            .as_array()
            .and_then(|list| list.iter().find(|s| s["id"] == "s1"))
            .ok_or("s1")?;
        let count = |name: &str| sandbox["counts"][name].as_u64().ok_or(format!("{sandbox}"));
        Ok((
            count("needs_attention")?,
            count("running")?,
            count("queued")?,
        ))
    };

    // t1 finished its turn with an open pull request whose checks still run, its plan complete.
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    set("t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    let tasks: [(&str, TaskStatus, Option<Activity>); 9] = [
        // Queued: in line, or after another task.
        ("t2", TaskStatus::Queued, None),
        ("t3", TaskStatus::Chained, None),
        // Working: on its way, paused for capacity, or working.
        ("t4", TaskStatus::Starting, None),
        ("t5", TaskStatus::WaitingCapacity, Some(Activity::Working)),
        ("t6", TaskStatus::Running, Some(Activity::Working)),
        ("t7", TaskStatus::Switching, None),
        // Your turn needs you; an achieved goal is Done; a stop asked for is with the ended.
        ("t8", TaskStatus::Running, Some(Activity::Waiting)),
        ("t9", TaskStatus::Running, Some(Activity::Done)),
        ("t10", TaskStatus::Stopping, Some(Activity::Working)),
    ];
    for (id, status, activity) in tasks {
        f.add_task(id, id, &format!("feat/{id}"), "dev").await?;
        set(id, status, activity).await?;
    }

    // Before any Git overview read, t1's pull request is not known: Your turn.
    assert_eq!(counts().await?, (2, 4, 2));

    // Checks running, read by the Git overview: t1 is In review, in the Working group.
    f.h.github.set_checks(&head, ChecksState::Pending);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(counts().await?, (1, 5, 2));

    // Green: Done, counted nowhere.
    f.h.github.set_checks(&head, ChecksState::Success);
    f.req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(counts().await?, (1, 4, 2));

    // Yulius CI could not run at the head (SPEC-050 STA-101): In review, Working, even with a
    // step open again, where its own state would be Your turn.
    use yulius_app::ci::CiRunRecord;
    use yulius_app::ports::store_ext::CiStore;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    let sandbox = SandboxId::parse("s1")?;
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut profile: Value = serde_json::from_str(&record.profile_json)?;
    profile["gate"] = json!("true");
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;
    f.h.store
        .insert_ci_run(
            &CiRunRecord {
                id: "run-1".to_owned(),
                sandbox_id: sandbox.clone(),
                reference: CiRef::PullRequest(1),
                sha: head.clone(),
                source: GateSource::Profile,
                state: RunState::Error,
                reason: Some("the job could not start".to_owned()),
                requested_by: "yulius".to_owned(),
                status_for: None,
                status_note: None,
                status_attempts: 0,
                queued_at: Timestamp(10),
                started_at: Some(Timestamp(10)),
                ended_at: Some(Timestamp(11)),
            },
            &[],
        )
        .await?;
    set_steps(&f, "t1", &["done", "pending"]).await?;
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(
        page.body.to_string().contains("\"checks\":\"error\""),
        "the overview reads the errored run: {}",
        page.body
    );
    assert_eq!(counts().await?, (1, 5, 2));
    Ok(())
}

/// A task with an open pull request at `head` (checks pending) and a live run whose agent calls
/// the internal endpoints with `agent`.
struct ArmedTask {
    f: Fixture,
    wt: PathBuf,
    head: String,
    internal: axum::Router,
    agent: Creds,
}

impl ArmedTask {
    async fn new() -> BoxResult<Self> {
        let f = fixture(&DeliveryProfile::studio()).await?;
        let wt = f
            .add_task("t1", "Landing page", "feat/1-landing", "quartio")
            .await?;
        commit_file(&wt, "landing.txt", "v1", "Landing")?;
        commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
        let opened = f
            .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
            .await?;
        assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
        let head = git(&wt, &["rev-parse", "HEAD"])?;
        f.h.github.set_checks(&head, ChecksState::Pending);
        // Its plan is finished, as `delivery_merge` needs to deliver a later head (DMG-001).
        set_steps(&f, "t1", &["done", "done"]).await?;
        f.h.store
            .insert_attempt(&AttemptRecord {
                id: AttemptId::parse("a1")?,
                task_id: TaskId::parse("t1")?,
                seq: 1,
                harness: Harness::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::New,
                state: AttemptState::Live,
                run_dir: "/tmp/none".into(),
                token_hash: f.h.app.deps.crypto.sha256_hex(b"tok-agent"),
                host_token: Sealed(vec![0]),
                ptyhost_pid: None,
                child_pid: None,
                started_at: Timestamp(1),
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        let internal = yulius_server::internal_router(f.h.app.clone())
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
        let agent = Creds {
            bearer: Some("tok-agent".into()),
            ..Creds::default()
        };
        Ok(Self {
            f,
            wt,
            head,
            internal,
            agent,
        })
    }

    /// `yulius wait` at `head`: the state now.
    async fn wait_at(&self, head: &str) -> BoxResult<Value> {
        let r = call(
            &self.internal,
            Method::POST,
            "/internal/agent/wait",
            Some(json!({"head": head, "since": Value::Null, "timeout_secs": 3})),
            &self.agent,
        )
        .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    /// `yulius wait` at the reviewed head: the state now (`since` null) or its next change.
    async fn wait(&self, since: Value) -> BoxResult<Value> {
        let r = call(
            &self.internal,
            Method::POST,
            "/internal/agent/wait",
            Some(json!({"head": self.head, "since": since, "timeout_secs": 3})),
            &self.agent,
        )
        .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    /// `delivery_merge` of the reviewed head; its tool result.
    async fn merge(&self) -> BoxResult<Value> {
        self.merge_at(&self.head).await
    }

    /// `delivery_merge` of `head`, reviewed; its tool result.
    async fn merge_at(&self, head: &str) -> BoxResult<Value> {
        let r = call(
            &self.internal,
            Method::POST,
            "/mcp",
            Some(
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "delivery_merge",
                "arguments": {"reviewed_head": head, "review_summary": "approve"}}}),
            ),
            &self.agent,
        )
        .await?;
        Ok(r.body["result"].clone())
    }

    async fn arm(&self) -> TestResult {
        let armed = self.merge().await?;
        assert_eq!(armed["isError"], false, "{armed}");
        let state = self.wait(Value::Null).await?;
        assert_eq!(state["merge_armed"], true, "{state}");
        assert_eq!(
            state["merge_outcome"],
            Value::Null,
            "a new arm forgets the old outcome"
        );
        Ok(())
    }

    fn set_behind(&self, behind: bool) {
        if let Ok(mut pulls) = self.f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            pull.behind = behind;
        }
    }

    async fn outcome(&self) -> BoxResult<String> {
        let state = self.wait(Value::Null).await?;
        assert_eq!(state["merge_armed"], false, "{state}");
        Ok(state["merge_outcome"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }
}

/// SPEC-011: `delivery_merge` while the checks of the reviewed head still run arms the merge;
/// Yulius merges exactly that head once they pass, and `yulius wait` (the internal wait endpoint)
/// sees each step and how the merge ended (MSR-003) without any polling by the agent.
#[tokio::test]
async fn an_armed_merge_happens_when_the_checks_pass() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let before = t.wait(Value::Null).await?;
    assert_eq!(before["stage"], "pr_open");
    assert_eq!(before["merge_armed"], false);
    t.arm().await?;

    // Checks still running: nothing happens.
    let sandbox = SandboxId::parse("s1")?;
    f.h.app.merge_armed(&sandbox).await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // MSR-003: while Yulius decides (GitHub is slow), the merge still reads as armed, so a wait
    // never ends on a decision that was not made.
    let hold = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let deciding = tokio::spawn(async move {
        app.merge_armed(&SandboxId::parse("s1")?).await;
        Ok::<_, yulius_domain::ids::IdError>(())
    });
    f.h.github.checks_entered.notified().await;
    let during = t.wait(Value::Null).await?;
    f.h.github.release_checks(&hold);
    deciding.await??;
    assert_eq!(during["merge_armed"], true, "{during}");
    assert_eq!(
        t.wait(Value::Null).await?["merge_armed"],
        true,
        "still armed: checks pending"
    );

    // The branch moves on, so the armed merge is dropped and the wait says why.
    commit_file(&t.wt, "landing.txt", "v2", "Landing v2")?;
    let moved = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("dropped") && outcome.contains(&moved[..9]),
        "{outcome}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // Back at the reviewed head and armed again.
    git(&t.wt, &["reset", "--hard", &t.head])?;
    t.arm().await?;

    // Green: Yulius merges the armed head, clears it and keeps how it ended for the wait.
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    // Merged (the studio profile then promotes by its own policy).
    let stage = f.delivery("t1").await?["stage"].clone();
    assert_ne!(stage, "pr_open");
    let after = t.wait(Value::Null).await?;
    assert_eq!(after["stage"], stage, "{after}");
    let outcome = t.outcome().await?;
    assert!(
        outcome.starts_with("Yulius merged pull request #1") && outcome.contains("`quartio`"),
        "{outcome}"
    );
    assert_eq!(after["merge_outcome_head"], t.head.as_str());
    assert!(
        f.h.store
            .task_messages(&TaskId::parse("t1")?, 10)
            .await?
            .is_empty(),
        "MSR-002: no message"
    );
    Ok(())
}

/// SPEC-206 RMV-001 (#624): nothing armed in a removed sandbox is merged. A CI run that ends
/// after the removal (green) calls `merge_armed`; the removed sandbox's pull request stays open.
#[tokio::test]
async fn a_removed_sandbox_merges_nothing_it_had_armed() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    t.arm().await?;

    let removed = f
        .req(Method::DELETE, "/api/sandboxes/s1", None, &f.alice)
        .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;

    let record =
        f.h.store
            .delivery(&TaskId::parse("t1")?)
            .await?
            .ok_or("delivery")?;
    assert_eq!(record.stage, DeliveryStage::PrOpen, "nothing merged");
    let states: Vec<String> =
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pulls")?
            .iter()
            .map(|p| p.state.clone())
            .collect();
    assert_eq!(states, ["open"], "the pull request stays open on GitHub");
    Ok(())
}

/// What `yulius wait` decides from a wait state the server answered.
fn settled(now: &Value, before: Option<&Value>) -> BoxResult<Option<String>> {
    let now: WaitState = serde_json::from_value(now.clone())?;
    let before: Option<WaitState> = before
        .map(|b| serde_json::from_value(b.clone()))
        .transpose()?;
    Ok(yulius::wait_cli::settled(&now, before.as_ref()))
}

/// #249 (SPEC-064 WAI-001): with a merge armed and its checks pending, `yulius wait` keeps
/// waiting, and ends with the merge's outcome once Yulius decides it.
#[tokio::test]
async fn a_wait_on_an_armed_merge_with_pending_checks_keeps_waiting() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    t.arm().await?;
    let first = t.wait(Value::Null).await?;
    assert_eq!(settled(&first, None)?, None, "{first}");
    // Yulius looks at it, the checks still run: nothing changes, so the wait goes on.
    f.h.app.merge_armed(&sandbox).await;
    let next = t.wait(first.clone()).await?;
    assert_eq!(next, first, "unchanged until the server's wait times out");
    assert_eq!(settled(&next, Some(&first))?, None);
    // Green: the merge happens and the wait says so.
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    let after = t.wait(first.clone()).await?;
    let line = settled(&after, Some(&first))?.unwrap_or_default();
    assert!(line.starts_with("Yulius merged pull request #1"), "{line}");
    Ok(())
}

/// #248 (SPEC-064 WAI-002): the agent arms a new head while Yulius still decides the merge it
/// armed before (GitHub is slow). That earlier decision neither puts the old merge back over the
/// new one nor ends the wait on the new one with the old one's outcome.
#[tokio::test]
async fn a_merge_armed_while_an_earlier_one_is_decided_supersedes_it() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    t.arm().await?;

    // Yulius starts deciding the merge of the first head; GitHub holds its checks read.
    let hold = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let deciding = tokio::spawn(async move {
        app.merge_armed(&SandboxId::parse("s1")?).await;
        Ok::<_, yulius_domain::ids::IdError>(())
    });
    f.h.github.checks_entered.notified().await;
    // Later reads go through; the decision stays held.
    if let Ok(mut held) = f.h.github.checks_hold.lock() {
        *held = None;
    }

    // Meanwhile the agent commits, gets the review and arms the new head.
    commit_file(&t.wt, "landing.txt", "v2", "Landing v2")?;
    let moved = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&moved, ChecksState::Pending);
    let armed = t.merge_at(&moved).await?;
    assert_eq!(armed["isError"], false, "{armed}");
    let before = t.wait_at(&moved).await?;
    assert_eq!(before["merge_armed"], true, "{before}");

    // The earlier decision ends (checks pending: it would have kept its merge).
    f.h.github.release_checks(&hold);
    deciding.await??;
    let now = t.wait_at(&moved).await?;
    assert_eq!(now["merge_armed"], true, "{now}");
    assert_eq!(now["merge_outcome"], Value::Null, "{now}");
    assert_eq!(settled(&now, Some(&before))?, None, "{now}");

    // Yulius decides again: the armed merge is the new head's, still pending.
    f.h.app.merge_armed(&sandbox).await;
    let again = t.wait_at(&moved).await?;
    assert_eq!(again["merge_armed"], true, "{again}");
    assert_eq!(settled(&again, Some(&before))?, None, "{again}");
    let saved = std::fs::read_to_string(
        f.h.app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE),
    )?;
    assert!(saved.contains(&moved), "the new head stays saved: {saved}");
    Ok(())
}

/// #251 (SPEC-064 WAI-004, SPEC-020): `reconcile_deliveries` queues an errored Yulius CI run at
/// an open pull request's head again once by itself (it could not run: not the code), never a
/// third time, and never for a pull request that conflicts with its base.
#[tokio::test]
async fn an_errored_run_at_an_open_pull_request_is_retried_once() -> TestResult {
    use yulius_app::ci::CiRunRecord;
    use yulius_app::ports::store_ext::CiStore;
    use yulius_domain::ci::{CiRef, GateSource, RunState};

    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    // The project has Yulius CI (the profile's gate), so its run at the head decides the checks.
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut profile: Value = serde_json::from_str(&record.profile_json)?;
    profile["gate"] = json!("true");
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;
    let errored = |id: &str, at: i64| CiRunRecord {
        id: id.to_owned(),
        sandbox_id: sandbox.clone(),
        reference: CiRef::PullRequest(1),
        sha: t.head.clone(),
        source: GateSource::Profile,
        state: RunState::Error,
        reason: Some("the job could not start".to_owned()),
        requested_by: "yulius".to_owned(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(at),
        started_at: Some(Timestamp(at)),
        ended_at: Some(Timestamp(at + 1)),
    };
    let runs = || f.h.store.ci_runs_for_sha(&sandbox, &t.head);

    // A conflicting pull request is not retried: the agent has to merge its base in first.
    f.h.store.insert_ci_run(&errored("run-1", 10), &[]).await?;
    let set_mergeable = |mergeable: Option<bool>| {
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            pull.mergeable = mergeable;
        }
    };
    set_mergeable(Some(false));
    f.h.app.reconcile_deliveries().await;
    assert_eq!(runs().await?.len(), 1, "conflicting: not retried");

    // Mergeable: the errored run is queued again, once.
    set_mergeable(None);
    f.h.app.reconcile_deliveries().await;
    let after = runs().await?;
    assert_eq!(after.len(), 2, "{after:?}");
    let retry = &after[0];
    assert_ne!(retry.id, "run-1");
    assert_eq!(retry.requested_by, "yulius");
    f.h.app.reconcile_deliveries().await;
    assert_eq!(runs().await?.len(), 2, "not while the retry is queued");

    // The retry errors too: two errored runs at the head wait for the agent.
    assert!(
        f.h.store
            .transition_ci_run(
                &retry.id,
                &[RunState::Queued, RunState::Running],
                RunState::Error,
                &yulius_app::ci::CiRunUpdate::default(),
            )
            .await?,
        "the retry was queued, not run: {retry:?}"
    );
    f.h.app.reconcile_deliveries().await;
    f.h.app.reconcile_deliveries().await;
    let last = runs().await?;
    assert_eq!(last.len(), 2, "no third run: {last:?}");
    assert!(last.iter().all(|r| r.state == RunState::Error), "{last:?}");
    Ok(())
}

/// Commits `file` on `base` of the fixture's origin, as another merged pull request would.
fn advance_base(f: &Fixture, base: &str, file: &str, content: &str) -> TestResult {
    let dir = tempfile::tempdir()?;
    let clone = dir.path().join("c");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            base,
            &f.origin.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    )?;
    commit_file(&clone, file, content, &format!("Base: {file}"))?;
    git(
        &clone,
        &["push", "--quiet", "origin", &format!("HEAD:{base}")],
    )?;
    Ok(())
}

/// DLV-203: an armed merge whose branch fell behind its base is brought up to date by Yulius (a
/// merge in the task's worktree, pushed as a fast-forward) and stays armed for the new head,
/// which is merged once its checks pass; a real conflict is aborted, leaves the worktree as it
/// was and tells the agent what to resolve.
#[tokio::test]
async fn an_armed_merge_behind_its_base_is_brought_up_to_date() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    let task = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    // A task's worktree has its creator's identity (set at its start).
    git(&t.wt, &["config", "user.name", "Bob"])?;
    git(&t.wt, &["config", "user.email", "bob@yulius.test"])?;

    // A real conflict first: the base adds the same file differently.
    t.arm().await?;
    advance_base(f, "quartio", "landing.txt", "base version")?;
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("conflicts with `quartio`") && outcome.contains("`landing.txt`"),
        "{outcome}"
    );
    assert_eq!(git(&t.wt, &["rev-parse", "HEAD"])?, t.head, "aborted");
    assert_eq!(git(&t.wt, &["status", "--porcelain"])?, "", "left clean");
    Ok(())
}

/// DLV-203, the clean case of `an_armed_merge_behind_its_base_is_brought_up_to_date`.
#[tokio::test]
async fn an_armed_merge_is_kept_current_and_merged_on_green() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    let task = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    // A task's worktree has its creator's identity (set at its start).
    git(&t.wt, &["config", "user.name", "Bob"])?;
    git(&t.wt, &["config", "user.email", "bob@yulius.test"])?;
    t.arm().await?;
    let seen = f.h.store.task(&task).await?.ok_or("t1")?;
    eprintln!(
        "DEBUG status={:?} activity={:?}",
        seen.status, seen.activity
    );
    advance_base(f, "quartio", "other.txt", "unrelated")?;
    f.h.app.merge_armed(&sandbox).await;
    let head = git(&t.wt, &["rev-parse", "HEAD"])?;
    assert_ne!(head, t.head, "Yulius merged the base in");
    git(&t.wt, &["merge-base", "--is-ancestor", &t.head, "HEAD"])?;
    git(
        &t.wt,
        &["merge-base", "--is-ancestor", "origin/quartio", "HEAD"],
    )?;
    assert_eq!(
        git(&f.origin, &["rev-parse", "feat/1-landing"])?,
        head,
        "pushed as a fast-forward"
    );
    assert_eq!(t.wait(Value::Null).await?["merge_armed"], true);
    // Green at the new head: merged.
    f.h.github.set_checks(&head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    assert_ne!(f.delivery("t1").await?["stage"], "pr_open");
    Ok(())
}

/// DLV-205: an armed merge survives a restart. It is saved when armed and put back for a running
/// task at startup (a deploy dropped them all, and agents waited for merges that no longer
/// existed).
#[tokio::test]
async fn an_armed_merge_survives_a_restart() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let task = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    t.arm().await?;
    let file =
        f.h.app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let saved = std::fs::read_to_string(&file)?;
    assert!(
        saved.contains("\"t1\"") && saved.contains(&t.head),
        "{saved}"
    );

    // A new server process reads it back (simulated: the saved entry, restored).
    f.h.app.restore_armed_merges().await;
    assert_eq!(t.wait(Value::Null).await?["merge_armed"], true);

    // Decided (merged): the entry goes.
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    assert_ne!(f.delivery("t1").await?["stage"], "pr_open");
    assert!(!std::fs::read_to_string(&file)?.contains("\"t1\""));
    Ok(())
}

/// DLV-205 with SPEC-064 WAI-002: a restore that read the saved merges before a merge was armed
/// and decided in this process does not bring that merge back (it would be decided again, as
/// "dropped", over the real outcome).
#[tokio::test]
async fn a_restore_never_brings_back_a_merge_decided_since() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let task = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    t.arm().await?;
    let file =
        f.h.app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let saved = std::fs::read_to_string(&file)?;
    // Decided in this process: the branch moved on.
    commit_file(&t.wt, "landing.txt", "v2", "Landing v2")?;
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    let decided = t.outcome().await?;
    assert!(decided.contains("dropped"), "{decided}");
    // The restore read the file before that decision.
    std::fs::write(&file, &saved)?;
    f.h.app.restore_armed_merges().await;
    let state = t.wait(Value::Null).await?;
    assert_eq!(state["merge_armed"], false, "{state}");
    assert_eq!(state["merge_outcome"], decided.as_str(), "{state}");
    Ok(())
}

/// SPEC-116 DMG-001 (task #101): a goal task whose evaluator keeps its turn going never reaches
/// the clean end of turn that opens its pull request, so `delivery_merge` delivers the head
/// itself: pushed, the pull request opened by the task's creator, then the merge armed. A head
/// that cannot be delivered yet is refused with why, and nothing opens or arms.
#[tokio::test]
async fn a_merge_call_opens_the_pull_request_a_goal_task_still_lacks() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let pulls_opened = || {
        f.h.github
            .writes()
            .iter()
            .filter(|w| w.starts_with("create_pull"))
            .count()
    };
    let merge = json!({"reviewed_head": head, "review_summary": "approve"});
    let refusal = |result: &Value| {
        assert_eq!(result["isError"], true, "{result}");
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    // A feature without a plan is not delivered either (review of SPEC-116, finding 2).
    let unplanned = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    assert!(refusal(&unplanned).contains("no plan"), "{unplanned}");
    assert_eq!(pulls_opened(), 0);

    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    let blocked = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    assert_eq!(blocked["isError"], true, "{blocked}");
    let why = blocked["content"][0]["text"].as_str().unwrap_or_default();
    assert!(why.contains("is in_progress"), "{why}");
    assert_eq!(pulls_opened(), 0, "nothing opened");
    assert_eq!(
        wait_state(&f, &agent, &head).await?["merge_armed"],
        false,
        "nothing armed"
    );

    set_steps(&f, "t1", &["done", "done"]).await?;
    let armed = arm_merge(&f, &agent, &head).await?;
    assert_eq!(armed["armed"], true, "{armed}");
    assert_eq!(pulls_opened(), 1, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head, "pushed");
    assert_eq!(
        f.h.github.actors_of("create_pull"),
        ["bob"],
        "as its creator"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // A later commit reaches the open pull request through the call too, before it arms.
    commit_file(&wt, "follow-up.txt", "more", "Follow-up")?;
    let follow_up = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&follow_up, ChecksState::Pending);
    let again = arm_merge(&f, &agent, &follow_up).await?;
    assert_eq!(again["armed"], true, "{again}");
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, follow_up, "pushed");
    assert_eq!(pulls_opened(), 1, "no second pull request");

    // A rewritten branch GitHub refuses: the call answers with the remedy, not git's words
    // (review of SPEC-116, finding 3), and nothing is force-pushed.
    git(&wt, &["reset", "--quiet", "--hard", &head])?;
    commit_file(&wt, "rewritten.txt", "other", "Rewritten")?;
    let rewritten = git(&wt, &["rev-parse", "HEAD"])?;
    let refused = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": rewritten, "review_summary": "approve"}),
    )
    .await?;
    let why = refusal(&refused);
    assert!(
        why.contains("Do not force-push")
            && why.contains("git merge --no-edit origin/feat/1-checkout-api"),
        "{why}"
    );
    assert_eq!(
        f.origin_sha("feat/1-checkout-api")?,
        follow_up,
        "not forced"
    );
    Ok(())
}

/// DLV-204: when the end of a turn delivers nothing because a step is still open, `yulius wait`
/// says so instead of waiting for a push that never comes (#34 waited 42 minutes).
#[tokio::test]
async fn an_undelivered_head_says_what_blocks_it() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    set_steps(f, "t1", &["done", "in_progress"]).await?;
    commit_file(&t.wt, "follow-up.txt", "more", "Follow-up")?;
    let head = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_ne!(f.origin_sha("feat/1-landing")?, head, "not pushed");
    let state = t.wait_at(&head).await?;
    let note = state["push_problem"].as_str().unwrap_or_default();
    assert!(
        note.starts_with("Yulius has not delivered") && note.contains("is in_progress"),
        "{state}"
    );
    // Resolved: delivered, and the note is gone.
    set_steps(f, "t1", &["done", "done"]).await?;
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_eq!(f.origin_sha("feat/1-landing")?, head, "pushed");
    assert_eq!(t.wait_at(&head).await?["push_problem"], Value::Null);
    Ok(())
}

/// SPEC-139 PSH-001 (#366): a push Git reports failed while GitHub's branch is at the head counts
/// as done, and clears the note an earlier failed push of that head left.
#[tokio::test]
async fn a_push_github_already_has_counts_as_done() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    commit_file(&t.wt, "follow-up.txt", "more", "Follow-up")?;
    let head = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);

    // Refused first: the note says so.
    f.git.fail_pushes(Some(PushFailure::Refused));
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_ne!(f.origin_sha("feat/1-landing")?, head, "not pushed");
    let state = t.wait_at(&head).await?;
    let note = state["push_problem"].as_str().unwrap_or_default();
    assert!(note.starts_with("Yulius could not push"), "{state}");

    // GitHub takes the next push and still reports `remote rejected … (failed)`: delivered.
    f.git.fail_pushes(Some(PushFailure::AfterLanding));
    let armed = t.merge_at(&head).await?;
    assert_eq!(armed["isError"], false, "{armed}");
    assert_eq!(f.origin_sha("feat/1-landing")?, head, "landed");
    assert_eq!(t.wait_at(&head).await?["push_problem"], Value::Null);
    Ok(())
}

/// SPEC-139 PSH-002 (#366): after Yulius's push failed and the agent pushed the head itself,
/// `yulius wait` at that head waits for its checks instead of ending on the old note.
#[tokio::test]
async fn a_stale_push_note_does_not_end_the_wait_once_the_branch_is_at_the_head() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    commit_file(&t.wt, "follow-up.txt", "more", "Follow-up")?;
    let head = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    let state = t.wait_at(&head).await?;
    assert!(settled(&state, None)?.is_some_and(|line| line.starts_with("Yulius could not push")));

    // The agent's own `git push` from the worktree; no turn ends since.
    git(&t.wt, &["push", "--quiet", "origin", "feat/1-landing"])?;
    // While the remote cannot be read, the note stands.
    f.git.remote_unreadable(true);
    let state = t.wait_at(&head).await?;
    assert!(state["push_problem"].is_string(), "{state}");
    f.git.remote_unreadable(false);
    let state = t.wait_at(&head).await?;
    assert_eq!(state["push_problem"], Value::Null, "{state}");
    assert_eq!(settled(&state, None)?, None, "it waits for the checks");
    // Gone for good: the next wait reads no note even with GitHub's branch moved on.
    git(
        &t.wt,
        &[
            "push",
            "--quiet",
            "origin",
            &format!("{}:refs/heads/feat/1-landing", t.head),
            "--force",
        ],
    )?;
    assert_eq!(t.wait_at(&head).await?["push_problem"], Value::Null);
    Ok(())
}

/// SPEC-139 PSH-001/002: at the end of a turn, a push that lands while Git reports a failure
/// opens the pull request. When the pull request then fails to open, the note says so and is no
/// push failure: GitHub's branch at the head does not make the wait skip it (#34).
#[tokio::test]
async fn a_pull_request_that_fails_to_open_after_its_push_keeps_its_note() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let task = TaskId::parse("t1")?;
    f.git.fail_pushes(Some(PushFailure::AfterLanding));
    if let Ok(mut error) = f.h.github.create_pull_error.lock() {
        *error = Some("Validation Failed".to_owned());
    }

    f.h.app.auto_open_pr(&task).await;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head, "landed");
    let state = wait_state(&f, &agent, &head).await?;
    let note = state["push_problem"].as_str().unwrap_or_default();
    assert!(
        note.contains("could not open its pull request") && note.contains("Validation Failed"),
        "{state}"
    );
    assert!(settled(&state, None)?.is_some(), "the wait ends on it");

    // GitHub opens it at the next end of turn; the push still "fails": delivered, no note.
    if let Ok(mut error) = f.h.github.create_pull_error.lock() {
        *error = None;
    }
    f.h.app.auto_open_pr(&task).await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    assert_eq!(
        wait_state(&f, &agent, &head).await?["push_problem"],
        Value::Null
    );
    Ok(())
}

/// SPEC-139 PSH-001: a push clears only the note about the head it pushed; a failed push of
/// another head keeps its note.
#[tokio::test]
async fn a_push_clears_only_the_note_about_its_own_head() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let task = TaskId::parse("t1")?;
    commit_file(&t.wt, "follow-up.txt", "more", "Follow-up")?;
    let first = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    f.h.app.auto_open_pr(&task).await;
    assert!(t.wait_at(&first).await?["push_problem"].is_string());

    commit_file(&t.wt, "second.txt", "more", "Second")?;
    let second = git(&t.wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(None);
    f.h.app.auto_open_pr(&task).await;
    assert_eq!(f.origin_sha("feat/1-landing")?, second, "pushed");
    let state = t.wait_at(&first).await?;
    assert!(state["push_problem"].is_string(), "{state}");
    Ok(())
}

/// SPEC-139 PSH-001: Deliver… opens the pull request when its push lands while Git reports a
/// failure.
#[tokio::test]
async fn deliver_opens_the_pull_request_after_a_push_that_landed() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(Some(PushFailure::AfterLanding));
    let opened = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({"title": "feat: checkout API"})),
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head);
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // A push that never lands is still refused.
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    let refused = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({"title": "feat: checkout API"})),
            &f.bob,
        )
        .await?;
    assert_ne!(refused.status, StatusCode::OK, "{}", refused.body);
    assert_eq!(f.delivery("t1").await?["stage"], "none");
    Ok(())
}

/// SPEC-139 PSH-002: when the agent pushed a head whose push Yulius could not make and no pull
/// request is open yet, the wait ends on how to open it instead of waiting for nothing.
#[tokio::test]
async fn a_pushed_head_without_a_pull_request_says_how_to_open_it() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    let refused = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");

    git(&wt, &["push", "--quiet", "origin", "feat/1-checkout-api"])?;
    let state = wait_state(&f, &agent, &head).await?;
    let note = state["push_problem"].as_str().unwrap_or_default();
    assert!(
        note.contains("pull request is not open yet") && note.contains("`delivery_merge`"),
        "{state}"
    );
    assert!(settled(&state, None)?.is_some(), "the wait ends on it");
    Ok(())
}

/// SPEC-139 PSH-003 (#448): a `delivery_merge` that fails for a reason unrelated to pushing,
/// after an earlier call was refused for an open step since resolved, answers with its own
/// failure, not the older "not delivered" note about the same head.
#[tokio::test]
async fn a_merge_call_answers_its_own_failure_not_an_older_note() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let merge = json!({"reviewed_head": head, "review_summary": "approve"});
    let refusal = |result: &Value| {
        assert_eq!(result["isError"], true, "{result}");
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    let blocked = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    assert!(refusal(&blocked).contains("is in_progress"), "{blocked}");

    // Resolved without a new commit; the next call fails reading the branch.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.git.fail_ahead_behind(Some("the object store is busy"));
    let failed = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    let why = refusal(&failed);
    assert!(
        why.contains("the object store is busy") && !why.contains("in_progress"),
        "{why}"
    );
    f.git.fail_ahead_behind(None);
    Ok(())
}

/// MSR-003: an armed merge whose checks fail or that GitHub refuses ends the wait with why; a
/// merge made at once afterwards leaves no stale outcome behind.
#[tokio::test]
async fn an_armed_merge_says_why_it_was_dropped_or_refused() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;

    // Red checks: dropped, with what to do.
    t.arm().await?;
    f.h.github.set_checks(&t.head, ChecksState::Failure);
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("dropped") && outcome.contains("checks") && outcome.contains("failed"),
        "{outcome}"
    );

    // Green but behind its base: GitHub's refusal is said.
    f.h.github.set_checks(&t.head, ChecksState::Pending);
    t.arm().await?;
    t.set_behind(true);
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.starts_with("The merge you armed was refused") && outcome.contains("behind"),
        "{outcome}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // Up to date again, merged at once by `delivery_merge`: the refusal is not news any more.
    t.set_behind(false);
    let merged = t.merge().await?;
    assert_eq!(merged["isError"], false, "{merged}");
    assert_ne!(f.delivery("t1").await?["stage"], "pr_open");
    let state = t.wait(Value::Null).await?;
    assert_eq!(state["merge_outcome"], Value::Null, "{state}");
    Ok(())
}

/// Edits pull request `number` on the fake GitHub, as a person would on github.com.
fn edit_pull(f: &Fixture, number: u64, edit: impl FnOnce(&mut GhPull)) {
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == number)
    {
        edit(pull);
    }
}

/// SPEC-069 BPL-003/004/005 (#223): the merge gate checks every pull request against the branch
/// policy of the integration branch's `yulius.toml` — the standard preset by default — refuses a
/// violation with the rule in words and merges once it is fixed; promotion pull requests are
/// titled and checked too, and a fork's is never taken.
#[tokio::test]
async fn the_branch_policy_gates_every_merge() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({"title": "Checkout API"})),
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);

    // A title that is not conventional: refused with the rule, nothing merged.
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    let message = error_message(&refused);
    for part in [
        "branch policy",
        "conventional",
        "`Checkout API`",
        "gh pr edit 1 --title",
    ] {
        assert!(message.contains(part), "{part}: {message}");
    }
    let merges = |f: &Fixture| {
        f.h.github
            .writes()
            .into_iter()
            .filter(|w| w.starts_with("merge_pull"))
            .count()
    };
    assert_eq!(merges(&f), 0);
    // Too long: refused with its length.
    edit_pull(&f, 1, |p| p.title = format!("feat: {}", "x".repeat(120)));
    let long = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert!(
        error_message(&long).contains("at most 100 characters; this one has 126"),
        "{}",
        long.body
    );
    // Fixed on GitHub: merges as before.
    edit_pull(&f, 1, |p| p.title = "feat(checkout): add the API".into());
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merges(&f), 1);

    // An untyped branch: refused, whatever its title.
    let wt2 = f.add_task("t2", "Search", "search-box", "dev").await?;
    commit_file(&wt2, "search.txt", "find", "feat: search")?;
    commit_file(
        &wt2,
        "docs/specs/2026-10-07-search.md",
        &valid_spec("t2"),
        "Spec",
    )?;
    let opened = f
        .req(Method::POST, "/api/tasks/t2/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head2 = git(&wt2, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head2, ChecksState::Success);
    let untyped = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(untyped.status, StatusCode::CONFLICT, "{}", untyped.body);
    let message = error_message(&untyped);
    assert!(
        message.contains("typed short-lived branch") && message.contains("`search-box`"),
        "{message}"
    );
    assert_eq!(merges(&f), 1);

    // Promotion: Yulius titles its pull request as the policy asks, refuses another title and
    // never takes a fork's pull request for its own.
    let fork = GhPull {
        number: 3,
        html_url: "https://github.test/Quartio-Studio/shop/pull/3".into(),
        title: "release: promote dev to main".into(),
        state: "open".into(),
        merged: false,
        base: "main".into(),
        head: "dev".into(),
        head_sha: head.clone(),
        mergeable: None,
        behind: false,
        from_fork: true,
    };
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.push(fork);
    }
    let promote = || {
        f.req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
    };
    let opened = promote().await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["result"], "pr_opened", "{}", opened.body);
    let promotion = f.h.github.pull(4).ok_or("promotion pull request")?;
    assert_eq!(promotion.title, "release: promote dev to main");
    f.h.github
        .set_checks(&promotion.head_sha, ChecksState::Success);
    edit_pull(&f, 4, |p| p.title = "Promote dev to main".into());
    let refused = promote().await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(
        error_message(&refused).contains("release: promote dev to main"),
        "{}",
        refused.body
    );
    assert_eq!(merges(&f), 1);
    edit_pull(&f, 4, |p| {
        p.title = "release: promote dev to main - checkout".into()
    });
    let promoted = promote().await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #4 merge".to_owned())
    );

    // An invalid policy at the integration branch: the standard preset applies, and says why.
    advance_base(
        &f,
        "dev",
        "yulius.toml",
        "[branch_policy]\npreset = \"strict\"\n",
    )?;
    git(&f.repo, &["fetch", "--quiet", "origin"])?;
    let invalid = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(invalid.status, StatusCode::CONFLICT, "{}", invalid.body);
    let message = error_message(&invalid);
    assert!(
        message.contains("typed short-lived branch")
            && message.contains("standard rules apply")
            && message.contains("yulius.toml line 2"),
        "{message}"
    );

    // The integration branch's yulius.toml turns the policy off: the same pull request merges.
    advance_base(
        &f,
        "dev",
        "yulius.toml",
        "[branch_policy]\npreset = \"none\"\n",
    )?;
    git(&f.repo, &["fetch", "--quiet", "origin"])?;
    let merged = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    Ok(())
}

/// SPEC-069 BPL-004: `delivery_merge` refuses a pull request the policy does not accept before
/// it arms anything, and an armed merge whose title is changed afterwards is refused, not merged.
#[tokio::test]
async fn agents_cannot_arm_or_land_a_merge_the_policy_refuses() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    // Yulius's own title passes: "Landing page" with non-conventional commits on `feat/1-landing`.
    assert_eq!(
        f.h.github.pull(1).ok_or("pull")?.title,
        "feat: landing page"
    );
    edit_pull(f, 1, |p| p.title = "Landing page".into());
    let refused = t.merge().await?;
    assert_eq!(refused["isError"], true, "{refused}");
    let text = refused["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("conventional"), "{text}");
    let state = t.wait(Value::Null).await?;
    assert_eq!(state["merge_armed"], false, "nothing armed: {state}");

    edit_pull(f, 1, |p| p.title = "feat: landing page".into());
    t.arm().await?;
    edit_pull(f, 1, |p| p.title = "Landing page".into());
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("refused") && outcome.contains("conventional"),
        "{outcome}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    Ok(())
}

/// Opens task `id`'s pull request after its spec commit alone, then commits `code` (oldest first)
/// and pushes it, as Yulius pushes a ready task's new commits to its open pull request. Returns
/// the pull request's number and the pushed head.
async fn open_after_the_spec(
    f: &Fixture,
    id: &str,
    title: &str,
    branch: &str,
    spec: &str,
    code: &[&str],
) -> BoxResult<(u64, String)> {
    let wt = f.add_task(id, title, branch, "dev").await?;
    commit_file(
        &wt,
        &format!("docs/specs/2026-10-14-{id}.md"),
        &valid_spec(id),
        spec,
    )?;
    let opened = f
        .req(
            Method::POST,
            &format!("/api/tasks/{id}/delivery/pr"),
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let number = opened.body["pr"]["number"]
        .as_u64()
        .ok_or_else(|| format!("no pull request: {}", opened.body))?;
    for (n, subject) in code.iter().enumerate() {
        commit_file(&wt, &format!("{id}-{n}.txt"), subject, subject)?;
    }
    git(
        &wt,
        &["push", "--quiet", "origin", &format!("HEAD:{branch}")],
    )?;
    Ok((number, git(&wt, &["rev-parse", "HEAD"])?))
}

/// The next retitles on the fake GitHub fail with `errors`, one each.
fn fail_retitles(f: &Fixture, errors: Vec<AppError>) {
    if let Ok(mut failures) = f.h.github.retitle_errors.lock() {
        failures.extend(errors);
    }
}

/// SPEC-069 BPL-008 (#381, #382): a pull request Yulius opened after the spec commit alone reads
/// `docs(spec): …`. Before Yulius merges it, by the armed merge and on the Git page alike, it
/// takes the type of its most significant commit on GitHub, so the squash commit names the
/// change; a title a person wrote stays. A title GitHub cannot take for now (a 5xx) keeps the
/// merge armed for the next pass; one GitHub refuses (a 422) stops it, and `yulius wait` says
/// why and what to do.
#[tokio::test]
async fn a_merge_names_the_change_by_its_most_significant_commit() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let sandbox = SandboxId::parse("s1")?;
    let squash_subject = || git(&f.origin, &["log", "-1", "--format=%s", "dev"]);
    let merges = |f: &Fixture| {
        f.h.github
            .writes()
            .into_iter()
            .filter(|w| w.starts_with("merge_pull"))
            .count()
    };
    let outage = || AppError::Upstream("GitHub: GitHub upstream error (HTTP 502)".into());

    // #382's commits, merged by the agent's armed merge once the checks pass; GitHub fails the
    // first retitle with a 502, so the merge stays armed and lands on the next pass.
    let (pr, head) = open_after_the_spec(
        &f,
        "t1",
        "yulius doctor warns about a GitHub CLI older than 2.82.1",
        "feat/1-doctor-gh",
        "docs(spec): SPEC-088 doctor warns about a GitHub CLI older than 2.82.1",
        &[
            "feat(doctor): warn about a GitHub CLI older than 2.82.1 (SPEC-088)",
            "test(doctor): the gh warning names both commands and the base rebuild",
            "fix(doctor): clearer gh warning wording",
            "docs(spec): SPEC-088 evidence and done",
        ],
    )
    .await?;
    assert_eq!(
        f.h.github.pull(pr).ok_or("pull")?.title,
        "docs(spec): yulius doctor warns about a GitHub CLI older than 2.82.1",
        "titled by the only commit there was"
    );
    let agent = live_agent(&f, "t1", "tok-t1").await?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    arm_merge(&f, &agent, &head).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    fail_retitles(&f, vec![outage()]);
    f.h.app.merge_armed(&sandbox).await;
    let waiting = wait_state(&f, &agent, &head).await?;
    assert_eq!(
        waiting["merge_armed"], true,
        "kept for the next pass: {waiting}"
    );
    assert_eq!(waiting["merge_outcome"], Value::Null, "{waiting}");
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    assert_eq!(merges(&f), 0);
    f.h.app.merge_armed(&sandbox).await;
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    let feat = "feat(doctor): yulius doctor warns about a GitHub CLI older than 2.82.1";
    assert_eq!(f.h.github.pull(pr).ok_or("pull")?.title, feat);
    assert_eq!(squash_subject()?, format!("{feat} (#{pr})"));
    let writes = f.h.github.writes();
    let retitled = writes
        .iter()
        .position(|w| *w == format!("update_pull #{pr} {feat}"))
        .ok_or("retitled")?;
    let merged = writes
        .iter()
        .position(|w| *w == format!("merge_pull #{pr} squash"))
        .ok_or("merged")?;
    assert!(retitled < merged, "{writes:?}");

    // #381's commits, green already. The agent's `delivery_merge` meets a 502 and arms the
    // merge; on the next pass GitHub refuses the title (422): nothing is merged, and the agent
    // hears why and how to set a title. A person then merges on the Git page.
    let (pr, head) = open_after_the_spec(
        &f,
        "t2",
        "Fix #364: the repository check catches committed conflict markers",
        "feat/2-conflict-markers",
        "docs(spec): SPEC-089 the repository check catches conflict markers (#364)",
        &[
            "fix(check): the repository check fails on committed conflict markers (#364)",
            "docs: the repository check's conflict markers in development and changes (#364)",
        ],
    )
    .await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    fail_retitles(
        &f,
        vec![
            outage(),
            AppError::Invalid(
                "GitHub: GitHub rejected the request as invalid (HTTP 422): Validation Failed"
                    .into(),
            ),
        ],
    );
    let agent = live_agent(&f, "t2", "tok-t2").await?;
    let armed = arm_merge(&f, &agent, &head).await?;
    assert_eq!(
        (&armed["merged"], &armed["armed"]),
        (&json!(false), &json!(true)),
        "{armed}"
    );
    f.h.app.merge_armed(&sandbox).await;
    let refused = wait_state(&f, &agent, &head).await?;
    assert_eq!(refused["merge_armed"], false, "{refused}");
    let outcome = refused["merge_outcome"].as_str().unwrap_or_default();
    let edit = format!("gh pr edit {pr} --title");
    for part in [
        "The merge you armed was refused",
        "GitHub refused the title `fix(check): fix #364",
        "HTTP 422",
        edit.as_str(),
        "Yulius keeps a title you write",
    ] {
        assert!(outcome.contains(part), "{part}: {outcome}");
    }
    assert_eq!(merges(&f), 1, "nothing merged under the wrong title");
    let landed = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(landed.status, StatusCode::OK, "{}", landed.body);
    let fix = "fix(check): fix #364: the repository check catches committed conflict markers";
    assert_eq!(f.h.github.pull(pr).ok_or("pull")?.title, fix);
    assert_eq!(squash_subject()?, format!("{fix} (#{pr})"));

    // A title a person wrote on GitHub is kept.
    let (pr, head) = open_after_the_spec(
        &f,
        "t3",
        "Search",
        "feat/3-search",
        "docs(spec): search",
        &["feat(search): the search box"],
    )
    .await?;
    assert_eq!(
        f.h.github.pull(pr).ok_or("pull")?.title,
        "docs(spec): search"
    );
    let written = "feat(search): a search box on every page";
    edit_pull(&f, pr, |p| p.title = written.into());
    f.h.github.set_checks(&head, ChecksState::Success);
    let landed = f
        .req(Method::POST, "/api/tasks/t3/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(landed.status, StatusCode::OK, "{}", landed.body);
    assert_eq!(f.h.github.pull(pr).ok_or("pull")?.title, written);
    assert_eq!(squash_subject()?, format!("{written} (#{pr})"));
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with(&format!("update_pull #{pr} "))),
        "kept"
    );
    Ok(())
}

/// SPEC-108 PRL-002 (task #93): a moved delivery stage tells the sandbox's browsers to read its
/// pull requests again, whoever moved it; a read that moves nothing announces nothing.
#[tokio::test]
async fn pull_request_changes_are_announced_to_the_sandbox() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let mut announced = support::PullAnnouncements::new(&f.h.app, "s1");
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    announced.take();

    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(announced.take(), 1, "opened");

    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);
    f.delivery("t1").await?;
    f.h.app.reconcile_deliveries().await;
    assert_eq!(announced.take(), 0, "nothing moved");

    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(announced.take(), 1, "merged by Yulius");

    // Merged on GitHub, outside Yulius: the reconcile loop moves the stage and says so.
    let wt = f
        .add_task("t2", "Cart totals", "feat/2-cart-totals", "dev")
        .await?;
    commit_file(&wt, "cart.txt", "totals", "Add cart totals")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t2"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t2/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    announced.take();
    let number = opened.body["pr"]["number"].as_u64().ok_or("number")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        number,
        "squash",
        &head,
    )
    .await?;
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t2").await?["stage"], "merged_integration");
    assert_eq!(announced.take(), 1, "reconciled with GitHub");
    Ok(())
}

/// SPEC-108 PRL-007: the read a merge's announcement brings may reach GitHub before its list of
/// open pull requests caught up; a pull request Yulius recorded as merged reads merged anyway.
#[tokio::test]
async fn a_merged_pull_request_github_still_lists_as_open_reads_merged() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);

    // GitHub's list still says open, as just after a merge.
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.merged = false;
        pull.state = "open".into();
    }
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let row = page.body["tasks"]
        .as_array()
        .ok_or("tasks")?
        .iter()
        .find(|t| t["task_id"] == "t1")
        .cloned()
        .ok_or("t1")?;
    assert_eq!(row["pr"]["number"], 1, "{row}");
    assert_eq!(row["pr"]["state"], "merged", "{row}");
    assert_eq!(row["pr"]["checks"], "none", "{row}");
    assert!(
        page.body["pulls"]
            .as_array()
            .ok_or("pulls")?
            .iter()
            .all(|p| p["number"] != 1),
        "not among the open pull requests: {}",
        page.body
    );
    Ok(())
}

/// A product profile with `dev-vm` deployed, and task t1's `feature.txt` ("hello") on the
/// integration branch of origin, pushed past the sandbox clone (which stays stale).
async fn deployable(f: &Fixture) -> BoxResult<()> {
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    let origin = f.origin.to_string_lossy().into_owned();
    git(
        &wt,
        &["push", "--quiet", &origin, "HEAD:refs/heads/quartio"],
    )?;
    Ok(())
}

/// `delivery_deploy` of `dev-vm` by t1's agent, to its end.
async fn agent_deploys(f: &Fixture) -> BoxResult<Value> {
    let agent = live_agent(f, "t1", "tok-deployer").await?;
    let started = answer(
        &agent_tool(
            f,
            &agent,
            "delivery_deploy",
            json!({"environment": "dev-vm"}),
        )
        .await?,
    )?;
    let run_id = started["run"]["run_id"].as_str().ok_or("run id")?;
    f.wait_run(run_id).await
}

/// #656 (SPEC-219 AC-1): a filter driver an agent plants in the sandbox clone's own configuration
/// never runs on the host when its agent deploys an environment: the run takes origin's commit
/// into Yulius's own mirror and writes its files from there.
#[tokio::test]
async fn an_agent_planted_git_filter_never_runs_on_the_host() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    deployable(&f).await?;
    let marker = f.root.path().join("smudge-ran");
    git(
        &f.repo,
        &[
            "config",
            "--local",
            "filter.x.smudge",
            &format!("touch '{}'; cat", marker.display()),
        ],
    )?;
    std::fs::create_dir_all(f.repo.join(".git/info"))?;
    std::fs::write(f.repo.join(".git/info/attributes"), "* filter=x\n")?;
    // The filter is live: a checkout in the clone runs it.
    git(&f.repo, &["fetch", "--quiet", "origin"])?;
    let probe = f.root.path().join("probe");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &probe.to_string_lossy(),
            "origin/quartio",
        ],
    )?;
    assert!(marker.exists(), "the planted filter runs in a checkout");
    std::fs::remove_file(&marker)?;

    let run = agent_deploys(&f).await?;
    assert_eq!(run["state"], "succeeded", "{run}");
    assert!(
        run["output"]
            .as_str()
            .unwrap_or("")
            .contains("deployed hello on dev-vm"),
        "{run}"
    );
    assert_eq!(run["commit"], f.origin_sha("quartio")?.as_str());
    assert!(
        !marker.exists(),
        "the planted smudge filter ran on the host"
    );
    Ok(())
}

/// #656 (SPEC-219 AC-3): an environment run never fetches with the token in the sandbox clone,
/// whose configuration its agents write: a planted `url.insteadOf` and `http.proxy` change
/// nothing, and the run deploys origin's head although the clone never saw it.
#[tokio::test]
async fn an_environment_run_never_fetches_in_the_sandbox_clone() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    deployable(&f).await?;
    let evil = f.root.path().join("evil.git");
    git(
        f.root.path(),
        &["init", "--quiet", "--bare", &evil.to_string_lossy()],
    )?;
    let origin = f.origin.to_string_lossy().into_owned();
    for (key, value) in [
        (format!("url.{}.insteadOf", evil.display()), origin.clone()),
        (
            "url.https://evil.invalid/.insteadOf".to_owned(),
            "https://github.com/".to_owned(),
        ),
        ("http.proxy".to_owned(), "http://127.0.0.1:9".to_owned()),
    ] {
        git(&f.repo, &["config", "--local", &key, &value])?;
    }
    let stale = git(&f.repo, &["rev-parse", "refs/remotes/origin/quartio"])?;
    assert_ne!(stale, f.origin_sha("quartio")?);

    let run = agent_deploys(&f).await?;
    assert_eq!(run["state"], "succeeded", "{run}");
    assert_eq!(run["commit"], f.origin_sha("quartio")?.as_str());
    assert_eq!(
        git(&f.repo, &["rev-parse", "refs/remotes/origin/quartio"])?,
        stale,
        "the sandbox clone was not fetched"
    );
    Ok(())
}

/// #656 (SPEC-219 AC-2): with containment on, an environment command runs in the sandbox's
/// containment: a file of the host outside the sandbox and the run is not there, and `HOME` is the
/// run's own.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn contained_environment_runs_see_only_their_sandbox() -> TestResult {
    let probe = Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.is_ok_and(|out| out.status.success()) {
        eprintln!(
            "SKIPPED contained_environment_runs_see_only_their_sandbox: bubblewrap cannot run here"
        );
        return Ok(());
    }
    let f = fixture_with(&DeliveryProfile::product(), contained(), None).await?;
    let host_only = f.h.app.config.data_dir.join("host-only.txt");
    std::fs::write(&host_only, "the host's\n")?;
    let command = format!(
        "if [ -e '{}' ]; then echo leaked; exit 7; fi; echo \"contained home=$HOME\"",
        host_only.display()
    );
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [
                    {"name": "dev-vm", "after": "integration", "verify": "command", "command": command}
                ]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    f.add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;

    let run = agent_deploys(&f).await?;
    let output = run["output"].as_str().unwrap_or("");
    assert_eq!(run["state"], "succeeded", "{run}");
    let runs = f.h.app.config.data_dir.join("runs").join("env-");
    assert!(
        output.contains(&format!("contained home={}", runs.display())),
        "{output}"
    );
    Ok(())
}

/// Containment on.
fn contained() -> support::ConfigureFn {
    Box::new(|config| config.contained = true)
}

/// SPEC-219 ENV-004: an environment command past the CI job timeout (as CD runs) is killed with
/// the processes it started, and the run fails saying so.
#[tokio::test]
async fn an_environment_command_past_its_timeout_is_killed() -> TestResult {
    let f = fixture_with(
        &DeliveryProfile::product(),
        Box::new(|config| config.ci.job_timeout = Duration::from_secs(2)),
        None,
    )
    .await?;
    let pid_file = f.root.path().join("child.pid");
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [{"name": "dev-vm", "after": "integration", "verify": "command",
                    "command": format!("sleep 60 & echo $! > '{}'; echo started; wait", pid_file.display())}]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    f.add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    let run = agent_deploys(&f).await?;
    let output = run["output"].as_str().unwrap_or("");
    assert_eq!(run["state"], "failed", "{run}");
    assert!(
        output.contains("started") && output.contains("the command exceeded"),
        "{output}"
    );
    let pid = std::fs::read_to_string(&pid_file)?.trim().to_owned();
    let alive = || {
        Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .is_ok_and(|o| {
                let stat = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                o.status.success() && !stat.is_empty() && !stat.starts_with('Z')
            })
    };
    for _ in 0..100 {
        if !alive() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("the background child {pid} of the timed-out command was not killed").into())
}

/// SPEC-300 RDB-002 (#664): an environment command whose files grow past the bound for a run is
/// stopped before its timeout, and the run is an `error` that says why.
#[tokio::test]
async fn an_environment_run_writing_past_its_disk_bound_is_stopped() -> TestResult {
    let f = fixture_with(
        &DeliveryProfile::product(),
        Box::new(|config| {
            config.ci.job_timeout = Duration::from_secs(20);
            config.ci.run_disk_max = 8_000_000;
        }),
        None,
    )
    .await?;
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [{"name": "dev-vm", "after": "integration", "verify": "command",
                    "command": "head -c 32000000 /dev/urandom > \"$HOME/fill\"; echo filled; sleep 30"}]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    f.add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    let run = agent_deploys(&f).await?;
    let output = run["output"].as_str().unwrap_or("");
    assert_eq!(run["state"], "error", "{run}");
    assert!(
        output.contains("filled") && output.contains("past the 8 MB disk bound for a run"),
        "{output}"
    );
    Ok(())
}

/// SPEC-219 ENV-004: the process host of a run that a restart left `running` keeps no command
/// going: the startup reconciliation records the run as `error`, then terminates it and removes
/// the run's directory.
#[tokio::test]
async fn a_restart_ends_a_running_environment_command() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    let pid_file = f.root.path().join("sleeper.pid");
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [{"name": "dev-vm", "after": "integration", "verify": "command",
                    "command": format!("echo $$ > '{}'; exec sleep 60", pid_file.display())}]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    f.add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    let agent = live_agent(&f, "t1", "tok-deployer").await?;
    let started = answer(
        &agent_tool(
            &f,
            &agent,
            "delivery_deploy",
            json!({"environment": "dev-vm"}),
        )
        .await?,
    )?;
    let run_id = started["run"]["run_id"].as_str().ok_or("run id")?;
    let mut pid = String::new();
    for _ in 0..400 {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && !text.trim().is_empty()
        {
            pid = text.trim().to_owned();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pid.is_empty(), "the command started");

    assert_eq!(f.h.app.reconcile_environment_runs().await?, 1);
    let run = f.wait_run(run_id).await?;
    assert_eq!(run["state"], "error", "{run}");
    let runs = f.h.app.config.data_dir.join("runs");
    assert!(
        !runs.join(format!("env-{run_id}")).exists(),
        "the run's directory is removed"
    );
    let alive = || {
        Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .is_ok_and(|o| {
                let stat = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                o.status.success() && !stat.is_empty() && !stat.starts_with('Z')
            })
    };
    for _ in 0..100 {
        if !alive() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("the command {pid} still runs").into())
}

/// SPEC-219 AC-2 (review of 5d72af32): a command that replaces its run's log with a link to a host
/// file does not get Yulius to read that file into the run's output: the log is read through the
/// handle Yulius opened before the command started, never by its path again.
#[tokio::test]
async fn a_relinked_run_log_never_reads_a_host_file() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    let host_file = f.h.app.config.data_dir.join("host-secret.txt");
    std::fs::write(&host_file, "HOST-SECRET-7f3a\n")?;
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [{"name": "dev-vm", "after": "integration", "verify": "command",
                    "command": format!(
                        "ln -sf '{}' \"$HOME/../log\"; sleep 3; echo after",
                        host_file.display()
                    )}]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    f.add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    let run = agent_deploys(&f).await?;
    let output = run["output"].as_str().unwrap_or("");
    assert_eq!(run["state"], "succeeded", "{run}");
    assert!(!output.contains("HOST-SECRET"), "{output}");
    assert!(
        output.contains("after"),
        "the command's own output is kept: {output}"
    );
    Ok(())
}

/// Records the plan of the one run it is asked to start and refuses to start it.
#[derive(Default)]
struct RecordingHost {
    plan: std::sync::Mutex<Option<(PathBuf, LaunchPlan)>>,
}

#[async_trait]
impl SessionHost for RecordingHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        _: &str,
        _: &str,
        plan: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        if let Ok(mut slot) = self.plan.lock() {
            *slot = Some((run_dir.to_path_buf(), plan.clone()));
        }
        Err(AppError::Upstream("recorded, not started".into()))
    }
    async fn attach(&self, _: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        Err(AppError::Upstream("nothing runs".into()))
    }
    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }
    fn terminate(&self, _: &Path) -> AppResult<()> {
        Ok(())
    }
}

/// #656 (SPEC-219 ENV-002/003, AC-2 where bubblewrap cannot run): a contained server starts an
/// environment run in the sandbox's containment, as a CD dev run: its root is the sandbox, only its
/// own directory is writable, its `HOME` is fresh there, and it runs the commit's files written into
/// that directory, not the sandbox clone.
#[tokio::test]
async fn a_contained_environment_run_starts_in_the_sandbox_containment() -> TestResult {
    let host = Arc::new(RecordingHost::default());
    let f = fixture_with(&DeliveryProfile::product(), contained(), Some(host.clone())).await?;
    deployable(&f).await?;
    let run = agent_deploys(&f).await?;
    assert_eq!(
        run["state"], "error",
        "the recording host starts nothing: {run}"
    );
    let (run_dir, plan) = host
        .plan
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or("no run was started")?;
    let runs = f.h.app.config.data_dir.join("runs");
    assert!(
        run_dir.starts_with(&runs)
            && run_dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("env-")),
        "{}",
        run_dir.display()
    );
    let work = run_dir.join("work");
    assert_eq!(plan.cwd, work.join("src"));
    assert!(
        plan.argv.iter().any(|arg| arg.contains("cat feature.txt")),
        "{:?}",
        plan.argv
    );
    let Containment::Bubblewrap {
        sandbox_root,
        home,
        writable,
        ..
    } = &plan.containment
    else {
        return Err(format!("not contained: {:?}", plan.containment).into());
    };
    assert_eq!(sandbox_root, &f.h.app.config.data_dir.join("sandboxes/s1"));
    assert_eq!(home, &work.join("home"));
    assert!(
        writable.iter().all(|dir| dir.starts_with(&work)),
        "{writable:?}"
    );
    assert!(
        plan.env
            .iter()
            .any(|(k, v)| k == "HOME" && *v == work.join("home").to_string_lossy()),
        "{:?}",
        plan.env
    );
    assert!(plan.secret_env.is_empty());
    assert!(!run_dir.exists(), "the run's directory goes with it");
    Ok(())
}

/// SPEC-219 ENV-003: a production environment's run, which only a person starts, runs as a CD
/// production run: rooted at its own directory, not the sandbox, with a fresh `HOME` there. Its
/// files are a production run's, in `<data>/cd/<sandbox>/runs` (SPEC-279 PDE-004).
#[tokio::test]
async fn a_production_environment_run_starts_in_production_containment() -> TestResult {
    let host = Arc::new(RecordingHost::default());
    let f = fixture_with(&DeliveryProfile::product(), contained(), Some(host.clone())).await?;
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "product", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": false,
                "environments": [{"name": "rollout", "after": "production", "verify": "command",
                    "command": "./roll.sh"}]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    let started = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/rollout/runs",
            Some(json!({"confirm": true})),
            &f.alice,
        )
        .await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let run = f
        .wait_run(started.body["run_id"].as_str().ok_or("run id")?)
        .await?;
    assert_eq!(run["commit"], f.origin_sha("main")?.as_str(), "{run}");
    let (run_dir, plan) = host
        .plan
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or("no run was started")?;
    let name = run_dir.file_name().ok_or("run directory")?;
    let work =
        f.h.app
            .config
            .data_dir
            .join("cd/s1/runs")
            .join(name)
            .join("work");
    let Containment::Bubblewrap {
        sandbox_root, home, ..
    } = &plan.containment
    else {
        return Err(format!("not contained: {:?}", plan.containment).into());
    };
    assert_eq!(
        sandbox_root, &work,
        "production is not rooted at the sandbox"
    );
    assert_eq!(home, &work.join("home"));
    assert_eq!(plan.cwd, work.join("src"));
    Ok(())
}
