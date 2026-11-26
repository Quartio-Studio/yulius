//! Git page and delivery flows end to end (AC-DLV-1..3, AC-GIT-1/2, AC-DSC-2): the HTTP API
//! over the application with a fake GitHub whose pull requests and refs are backed by a real
//! bare repository, and a test-local `GitPort` over the real `git` command line.

use crate::support;
use crate::support::git;

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
    AuditStore, CoordinationStore, InstanceStore, SandboxStore, TaskStore, UserStore,
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
    /// A dispatched native Git read can outlive its caller.
    parked_git_hold: std::sync::Mutex<Option<Arc<ParkedGitHold>>>,
    /// Hold the second checkout probe while the predecessor changes state.
    checkout_probe: std::sync::Mutex<Option<Arc<CheckoutProbe>>>,
    overview_log_entered: tokio::sync::Notify,
    overview_log_hold: std::sync::Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// While set, a refresh of a clone fails (SPEC-493 fix 6).
    fetch_fails: std::sync::atomic::AtomicBool,
    /// Moved right after every refresh of a clone, as an agent racing it would: `(ref, commit)`
    /// (review round 2 of SPEC-493 fix 6).
    forged_after_fetch: std::sync::Mutex<Option<(String, String)>>,
}

struct ParkedGitHold {
    started: std::sync::atomic::AtomicUsize,
    released: tokio::sync::Semaphore,
    finishing: tokio::sync::Semaphore,
}

#[derive(Default)]
struct CheckoutProbe {
    calls: std::sync::atomic::AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
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
            if self.fetch_fails.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(AppError::Conflict("the fetch failed (test)".into()));
            }
            self.run(dest, &["fetch", "--prune", "--quiet", "origin"])
                .await?;
            let forged = self.forged_after_fetch.lock().ok().and_then(|f| f.clone());
            if let Some((name, commit)) = forged {
                self.run(dest, &["update-ref", &name, &commit]).await?;
            }
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
        crate::support::confined_git()
            .mirror_fetch_head(mirror, &origin, branch, None)
            .await
    }
    async fn export_tree(&self, mirror: &Path, sha: &str, dest: &Path) -> AppResult<()> {
        crate::support::confined_git()
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
    async fn install_push_guard(&self, _: &Path, _: &Path, _: &[String]) -> AppResult<()> {
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
        crate::support::confined_git()
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
        crate::support::confined_git()
            .review_commits(repo, base, head, limit)
            .await
    }

    /// The production adapter (SPEC-083): record numbers, review carry-over, merge attributes.
    async fn list_dir(&self, repo: &Path, rev: &str, dir: &str) -> AppResult<Vec<String>> {
        crate::support::confined_git()
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
        crate::support::confined_git()
            .only_clean_merges_since(worktree, reviewed, head, integration, limit)
            .await
    }

    async fn resolve_commit(&self, repo: &Path, rev: &str) -> AppResult<Option<String>> {
        let hold = self
            .parked_git_hold
            .lock()
            .map_err(AppError::internal)?
            .clone();
        if rev == "origin/dev"
            && let Some(hold) = hold
        {
            tokio::spawn(async move {
                hold.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = hold.released.acquire().await;
                let _ = hold.finishing.acquire().await;
            })
            .await
            .map_err(AppError::internal)?;
        }
        crate::support::confined_git()
            .resolve_commit(repo, rev)
            .await
    }

    async fn worktrees(&self, repo: &Path) -> AppResult<Vec<yulius_app::ports::git::WorktreeInfo>> {
        let probe = self
            .checkout_probe
            .lock()
            .map_err(AppError::internal)?
            .clone();
        if let Some(probe) = probe
            && probe
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
        {
            probe.entered.notify_one();
            probe.release.notified().await;
        }
        crate::support::confined_git().worktrees(repo).await
    }

    async fn adds_to(
        &self,
        worktree: &Path,
        base: &str,
        since: &str,
        head: &str,
    ) -> AppResult<Option<bool>> {
        crate::support::confined_git()
            .adds_to(worktree, base, since, head)
            .await
    }

    async fn is_ancestor(&self, repo: &Path, ancestor: &str, descendant: &str) -> AppResult<bool> {
        crate::support::confined_git()
            .is_ancestor(repo, ancestor, descendant)
            .await
    }

    async fn set_merge_attributes(&self, repo: &Path, lines: &[String]) -> AppResult<bool> {
        crate::support::confined_git()
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
        let hold = self
            .overview_log_hold
            .lock()
            .map_err(AppError::internal)?
            .clone();
        if let Some(hold) = hold {
            self.overview_log_entered.notify_one();
            hold.notified().await;
        }
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
    async fn follow_up_commits(
        &self,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: &str,
    ) -> AppResult<Option<Vec<String>>> {
        crate::support::confined_git()
            .follow_up_commits(worktree, reviewed, head, integration)
            .await
    }

    async fn commit_lines(
        &self,
        repo: &Path,
        commit: &str,
    ) -> AppResult<Option<Vec<yulius_domain::follow_up::FileLines>>> {
        crate::support::confined_git()
            .commit_lines(repo, commit)
            .await
    }

    async fn changed_paths(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
    ) -> AppResult<Option<Vec<String>>> {
        crate::support::confined_git()
            .changed_paths(repo, base, head)
            .await
    }

    async fn show_file(&self, repo: &Path, rev: &str, path: &str) -> AppResult<Option<String>> {
        let object = format!("{rev}:{path}");
        if self.run(repo, &["cat-file", "-e", &object]).await.is_err() {
            return Ok(None);
        }
        self.run(repo, &["show", &object]).await.map(Some)
    }

    async fn parents(&self, repo: &Path, rev: &str) -> AppResult<Vec<String>> {
        let line = git(repo, &["rev-list", "--parents", "-n", "1", rev])
            .map_err(|e| AppError::Upstream(e.to_string()))?;
        Ok(line.split_whitespace().skip(1).map(str::to_owned).collect())
    }
}

/// The fake GitHub's repository: merges and fast-forwards happen directly in the bare origin
/// (`merge-tree` + `commit-tree` + `update-ref`, no working tree).
pub(crate) struct BareRemote {
    pub(crate) origin: PathBuf,
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

/// #745 (SPEC-409 VTW-001): the spec's folder swapped for a link to a host folder after its path
/// was found inside the worktree: the read goes through a handle whose real path is outside it,
/// so the host's file is refused, not read as the spec.
#[tokio::test]
async fn a_spec_folder_swapped_for_a_link_after_its_check_is_not_read() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    assert_eq!(f.delivery("t1").await?["spec_valid"], true);
    let host = tempfile::tempdir()?;
    std::fs::create_dir_all(host.path().join("specs"))?;
    std::fs::write(
        host.path().join(SPEC_PATH.trim_start_matches("docs/")),
        "host secret\n",
    )?;
    let (folder, target) = (wt.join("docs"), host.path().to_path_buf());
    yulius_app::race::on(
        yulius_app::race::Point::RepoFileRead,
        &wt.join(SPEC_PATH),
        move || {
            std::fs::rename(&folder, folder.with_extension("aside"))
                .and_then(|()| std::os::unix::fs::symlink(&target, &folder))
                .ok();
        },
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["spec_valid"], false, "{d}");
    assert!(
        d["spec_problems"]
            .to_string()
            .contains("points outside the task worktree"),
        "{d}"
    );
    Ok(())
}

/// #745 (SPEC-409 VTW-001): the spec swapped for a link to a host file after its check: the
/// link at the end is not followed, so the host's file is not read as the spec.
#[tokio::test]
async fn a_spec_swapped_for_a_link_after_its_check_is_not_read() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    assert_eq!(f.delivery("t1").await?["spec_valid"], true);
    let host = tempfile::tempdir()?;
    let secret = host.path().join("secret.md");
    std::fs::write(&secret, "host secret\n")?;
    let file = wt.join(SPEC_PATH);
    let link = file.clone();
    yulius_app::race::on(yulius_app::race::Point::RepoFileRead, &file, move || {
        std::fs::remove_file(&link)
            .and_then(|()| std::os::unix::fs::symlink(&secret, &link))
            .ok();
    });
    let d = f.delivery("t1").await?;
    assert_eq!(d["spec_valid"], false, "{d}");
    assert!(
        d["spec_problems"]
            .to_string()
            .contains("points outside the task worktree"),
        "{d}"
    );
    Ok(())
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
        parked_git_hold: Default::default(),
        checkout_probe: Default::default(),
        overview_log_entered: Default::default(),
        overview_log_hold: Default::default(),
        fetch_fails: std::sync::atomic::AtomicBool::default(),
        forged_after_fetch: std::sync::Mutex::default(),
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
        self.insert_task(
            id,
            title,
            branch,
            Some(worktree.to_string_lossy().into_owned()),
            creator,
        )
        .await?;
        Ok(worktree)
    }

    /// A task given its branch at creation that has not started: no worktree yet (SPEC-343).
    async fn add_unstarted_task(&self, id: &str, title: &str, branch: &str) -> BoxResult<()> {
        self.insert_task(id, title, branch, None, "u-bob").await
    }

    async fn insert_task(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        worktree_path: Option<String>,
        creator: &str,
    ) -> BoxResult<()> {
        self.h
            .store
            .insert_task(&TaskRecord {
                resume_attempt_id: None,
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
                worktree_path,
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
        Ok(())
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

/// Whether a pull request conflicts with its base is asked two ways: `merge_base` + `adds_to`
/// (the watch's and delivery's check) and `preview_merge` (the armed-merge pass's). On the real
/// Git, with the repository's merge drivers, they agree in every shape a pull request takes.
#[tokio::test]
async fn preview_merge_and_adds_to_agree_on_conflicts() -> TestResult {
    use yulius_app::ports::git::TrainStep;
    let real = support::confined_git();
    let dir = tempfile::tempdir()?;
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "dev"])?;
    commit_file(repo, ".gitattributes", "changes.md merge=union\n", "attributes")?;
    commit_file(repo, "a.txt", "one\n", "a")?;
    commit_file(repo, "changes.md", "# Changes\n", "changes")?;
    // (shape, the head's commits, then the base's commits, whether it conflicts)
    let shapes: [(&str, &[(&str, &str)], &[(&str, &str)], bool); 6] = [
        ("apart", &[("b.txt", "head\n")], &[("c.txt", "base\n")], false),
        ("conflict", &[("a.txt", "head\n")], &[("a.txt", "base\n")], true),
        (
            "union",
            &[("changes.md", "# Changes\nhead\n")],
            &[("changes.md", "# Changes\nbase\n")],
            false,
        ),
        ("up-to-date", &[("d.txt", "head\n")], &[], false),
        ("behind", &[], &[("e.txt", "base\n")], false),
        ("same-change", &[("f.txt", "same\n")], &[("f.txt", "same\n")], false),
    ];
    for (shape, head_files, base_files, conflicts) in shapes {
        let branch = format!("head-{shape}");
        git(repo, &["checkout", "-q", "-b", &branch, "dev"])?;
        for (path, text) in head_files {
            commit_file(repo, path, text, shape)?;
        }
        git(repo, &["checkout", "-q", "dev"])?;
        for (path, text) in base_files {
            commit_file(repo, path, text, shape)?;
        }
        let base = git(repo, &["rev-parse", "dev"])?;
        let head = git(repo, &["rev-parse", &branch])?;
        let since = real.merge_base(repo, &base, &head).await?.ok_or("merge base")?;
        let by_adds_to = real.adds_to(repo, &base, &since, &head).await?.is_none();
        let by_preview = matches!(
            real.preview_merge(repo, &base, &head).await?,
            TrainStep::Conflict(_)
        );
        assert_eq!(by_adds_to, conflicts, "{shape}: adds_to");
        assert_eq!(by_preview, conflicts, "{shape}: preview_merge");
    }
    // A head with no history in common has no merge base: neither check can tell, so the
    // callers ask GitHub instead.
    git(repo, &["checkout", "-q", "--orphan", "head-unrelated"])?;
    commit_file(repo, "g.txt", "alone\n", "unrelated")?;
    let head = git(repo, &["rev-parse", "head-unrelated"])?;
    let base = git(repo, &["rev-parse", "dev"])?;
    assert_eq!(real.merge_base(repo, &base, &head).await?, None);
    Ok(())
}

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

/// SPEC-483 D5: an automatic promotion (`auto_promote`) is part of the merge it follows. With
/// required checks and Yulius CI, the merge needs the pull request's head green; the
/// integration head that merge makes has no run of its own yet, and the fast-forward goes on
/// the merge's checks instead of waiting for ever on a run nobody would act on.
#[tokio::test]
async fn an_automatic_promotion_goes_on_the_checks_of_its_merge() -> TestResult {
    use yulius_domain::ci::RunState;
    let mut profile = DeliveryProfile::studio();
    profile.require_checks = true;
    let f = fixture(&profile).await?;
    // Yulius CI is the project's CI: a commit without a run is pending.
    let sandbox = SandboxId::parse("s1")?;
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut stored: Value = serde_json::from_str(&record.profile_json)?;
    stored["gate"] = json!("true");
    f.h.store.set_profile(&sandbox, &stored.to_string()).await?;
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
    let waiting = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(
        waiting.status,
        StatusCode::CONFLICT,
        "pending: {}",
        waiting.body
    );
    yulius_ci_run(&f.h.store, "run-pr-1", &head, RunState::Passed, 10).await?;
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    let quartio = f.origin_sha("quartio")?;
    assert_ne!(quartio, head, "the merge made a commit of its own");
    assert_eq!(f.origin_sha("main")?, quartio, "promoted with the merge");
    Ok(())
}

/// SPEC-483 D5: an automatic promotion goes while the integration head's checks have not
/// reported, wherever it starts (after a merge, after an integration environment), and is
/// stopped by a verdict that is there: with a failed run of Yulius CI at that head, the
/// environment's confirmation promotes nothing and says why; a person promotes once the run
/// passed.
#[tokio::test]
async fn an_automatic_promotion_stops_at_a_failed_head() -> TestResult {
    use yulius_domain::ci::RunState;
    let f = fixture(&DeliveryProfile::product()).await?;
    let custom = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {
                "preset": "custom", "integration_branch": "quartio", "production_branch": "main",
                "merge": "merge", "promote": "fast_forward", "auto_promote": true,
                "require_checks": true,
                "environments": [
                    {"name": "staging", "after": "integration", "verify": "manual", "command": null}
                ]
            }})),
            &f.alice,
        )
        .await?;
    assert_eq!(custom.status, StatusCode::OK, "{}", custom.body);
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
        .set_checks(&git(&wt, &["rev-parse", "HEAD"])?, ChecksState::Success);
    let main_before = f.origin_sha("main")?;
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(f.origin_sha("main")?, main_before, "staging comes first");
    let quartio = f.origin_sha("quartio")?;
    yulius_ci_run(&f.h.store, "run-q-1", &quartio, RunState::Failed, 10).await?;
    let confirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/staging/confirm",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    let said = confirmed.body.to_string();
    let why = said
        .split_once("Automatic promotion did not happen:")
        .map(|(_, why)| why)
        .ok_or_else(|| format!("no such note: {said}"))?;
    assert!(why.contains("fail"), "the reason: {why}");
    assert_eq!(
        f.origin_sha("main")?,
        main_before,
        "a failed head is not promoted"
    );
    yulius_ci_run(&f.h.store, "run-q-2", &quartio, RunState::Passed, 20).await?;
    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(f.origin_sha("main")?, quartio);
    Ok(())
}

/// A run of Yulius CI at `sha` of branch `dev`, ended in `state`, queued at `at` (the newest
/// run of a commit is its verdict).
async fn yulius_ci_run(
    store: &impl yulius_app::ports::store_ext::CiStore,
    id: &str,
    sha: &str,
    state: yulius_domain::ci::RunState,
    at: i64,
) -> TestResult {
    use yulius_domain::ci::CiRef;
    insert_ci_attempt(store, id, sha, CiRef::Branch("dev".into()), state, at, &[]).await
}

pub(super) async fn insert_ci_attempt(
    store: &impl yulius_app::ports::store_ext::CiStore,
    id: &str,
    sha: &str,
    reference: yulius_domain::ci::CiRef,
    state: yulius_domain::ci::RunState,
    at: i64,
    checks: &[(&str, yulius_domain::ci::RunState)],
) -> TestResult {
    let jobs: Vec<_> = checks
        .iter()
        .enumerate()
        .map(|(index, (name, verdict))| {
            Ok(yulius_app::ci::CiJobRecord {
                id: format!("{id}-{index}"),
                run_id: id.into(),
                ordinal: u32::try_from(index)?,
                name: (*name).into(),
                key: (*name).into(),
                command: "test".into(),
                matrix: None,
                state: *verdict,
                reason: None,
                exit_code: None,
                output: String::new(),
                failures: None,
                started_at: Some(Timestamp(at)),
                ended_at: Some(Timestamp(at + 1)),
                need: Default::default(),
                timeout_minutes: None,
                exclusive: false,
                waited_ms: None,
            })
        })
        .collect::<BoxResult<_>>()?;
    use yulius_app::ci::CiRunRecord;
    use yulius_domain::ci::GateSource;
    store
        .insert_ci_run(
            &CiRunRecord {
                id: id.to_owned(),
                sandbox_id: SandboxId::parse("s1")?,
                reference,
                sha: sha.to_owned(),
                source: GateSource::Profile,
                state,
                reason: None,
                requested_by: "yulius".to_owned(),
                status_for: None,
                status_note: None,
                status_attempts: 0,
                queued_at: Timestamp(at),
                started_at: Some(Timestamp(at)),
                ended_at: Some(Timestamp(at + 1)),
            },
            &jobs,
        )
        .await?;
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
    // SPEC-483 D5: a promotion asks what every merge asks. With a run of Yulius CI at that
    // head, the run alone decides, whatever GitHub shows from elsewhere: failed, the promotion
    // is refused; run again and passed, it goes.
    use yulius_domain::ci::RunState;
    yulius_ci_run(
        &f.h.store,
        "run-dev-1",
        &promotion_head,
        RunState::Failed,
        10,
    )
    .await?;
    let failing = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(failing.status, StatusCode::CONFLICT, "{}", failing.body);
    assert!(error_message(&failing).contains("fail"), "{}", failing.body);
    assert!(
        !f.h.github
            .writes()
            .contains(&"merge_pull #2 merge".to_owned()),
        "nothing merged past a failed run"
    );
    yulius_ci_run(
        &f.h.store,
        "run-dev-2",
        &promotion_head,
        RunState::Passed,
        20,
    )
    .await?;
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
    // SPEC-483 D5: what goes to production is the integration head, and its checks decide
    // for a fast-forward as for a promotion pull request: a failed run of Yulius CI there
    // refuses it, the run passed again lets it go.
    use yulius_domain::ci::RunState;
    // A run that still goes: a person's promotion waits for it.
    yulius_ci_run(&f.h.store, "run-q-0", &quartio, RunState::Running, 5).await?;
    let running = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(running.status, StatusCode::CONFLICT, "{}", running.body);
    assert_ne!(f.origin_sha("main")?, quartio, "main did not move");
    yulius_ci_run(&f.h.store, "run-q-1", &quartio, RunState::Failed, 10).await?;
    let failing = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({"confirm": "shop"})),
            &f.alice,
        )
        .await?;
    assert_eq!(failing.status, StatusCode::CONFLICT, "{}", failing.body);
    assert!(error_message(&failing).contains("fail"), "{}", failing.body);
    assert_ne!(f.origin_sha("main")?, quartio, "main did not move");
    yulius_ci_run(&f.h.store, "run-q-2", &quartio, RunState::Passed, 20).await?;
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

/// SPEC-348 ERD-001/002 (#952): an environment run whose request is dropped as soon as its run is
/// recorded still runs its command, never staying `running` with nothing executing it, and its
/// permit is let go. Each poll of the request passes at most one store call and the test reads the
/// store between polls, so the request is dropped at most one store call past its insert, while
/// its audit entry remains before its execution's spawn.
#[tokio::test]
async fn an_environment_run_whose_request_is_dropped_after_its_record_still_runs() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    let mut request = Box::pin(f.req(
        Method::POST,
        "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
        Some(json!({"confirm": true})),
        &f.bob,
    ));
    let id = loop {
        if let std::task::Poll::Ready(answer) = futures::poll!(request.as_mut()) {
            let answer = answer?;
            assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
            break answer.body["run_id"].as_str().ok_or("id")?.to_owned();
        }
        if let Some(run) =
            f.h.store
                .running_environment_runs()
                .await?
                .into_iter()
                .next()
        {
            drop(request);
            break run.id;
        }
    };
    let run = f.wait_run(&id).await?;
    assert_eq!(run["state"], "succeeded", "{run}");
    let output = run["output"].as_str().unwrap_or_default();
    assert!(output.contains("on dev-vm"), "{output}");
    assert_eq!(f.h.app.deploys_starting(), 0, "no permit is left behind");
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

/// SPEC-493 REV-005, review round 3 of fix 1c: a merge by merge commit judges the merged head
/// against the integration branch as it was before the merge (the merge commit's first parent),
/// not after it. A clean merge of the integration branch after the approval keeps it; a merged
/// side branch nobody reviewed does not, though the merge put that branch into the integration
/// branch. The same for a merge made on GitHub and found by Yulius afterwards (fix 1d), by the
/// reconcile loop or by the sandbox's Git page: its merge commit says the base. A follow-up within
/// the bound after the approval keeps it too (SPEC-493 APR-001, review round 1 of fix 6).
#[tokio::test]
async fn a_merge_commit_merge_judges_the_head_against_the_branch_before_it() -> TestResult {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Found {
        /// Merged by Yulius.
        Merged,
        /// Merged on GitHub, found by the reconcile loop.
        Reconciled,
        /// Merged on GitHub, found when the sandbox's Git page is read.
        GitPage,
    }
    for (shape, found) in [0, 1, 2].into_iter().flat_map(|shape| {
        [Found::Merged, Found::Reconciled, Found::GitPage].map(move |found| (shape, found))
    }) {
        // 0: a clean merge of the integration branch; 1: a side branch; 2: a small follow-up.
        let side_branch = shape == 1;
        let on_github = found != Found::Merged;
        let mut profile = DeliveryProfile::studio();
        profile.merge = yulius_domain::delivery::MergeMethod::Merge;
        let f = fixture(&profile).await?;
        let wt = f
            .add_task("t1", "Landing page", "feat/1-landing", "quartio")
            .await?;
        commit_file(&wt, "landing.txt", "v1", "Landing")?;
        commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
        let reviewed = git(&wt, &["rev-parse", "HEAD"])?;
        if side_branch {
            git(&wt, &["switch", "--quiet", "-c", "side", "HEAD~2"])?;
            commit_file(&wt, "side.txt", "unreviewed", "Side work")?;
            git(&wt, &["switch", "--quiet", "feat/1-landing"])?;
            git(&wt, &["merge", "--quiet", "--no-edit", "side"])?;
        } else if shape == 2 {
            commit_file(&wt, "nit.txt", "a nit fixed\n", "Nit")?;
        } else {
            // Another task's work reaches the integration branch; the task merges it in.
            let other = f
                .add_task("t9", "Upstream", "feat/9-upstream", "quartio")
                .await?;
            commit_file(&other, "upstream.txt", "u1", "Upstream")?;
            git(&other, &["push", "--quiet", "origin", "HEAD:quartio"])?;
            git(&wt, &["fetch", "--quiet", "origin"])?;
            git(&wt, &["merge", "--quiet", "--no-edit", "origin/quartio"])?;
        }
        let head = git(&wt, &["rev-parse", "HEAD"])?;
        assert_ne!(head, reviewed);
        {
            use yulius_app::ports::store::CoordinationStore;
            f.h.store
                .insert_review_round(&yulius_app::model::ReviewRoundRecord {
                    id: "round-1".into(),
                    task_id: TaskId::parse("t1")?,
                    round: 1,
                    head: Some(reviewed.clone()),
                    agent: "yulius-reviewer".into(),
                    model: "claude-fable-5-2".into(),
                    reason: "round".into(),
                    subagent_id: Some("rev-1".into()),
                    model_answered: Some("claude-fable-5-2".into()),
                    started_at: Timestamp(1),
                    ended_at: Some(Timestamp(2)),
                    verdict: Some("approve".into()),
                    cut_off: false,
                    pr_number: None,
                    merged_at: None,
                })
                .await?;
        }
        let opened = f
            .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
            .await?;
        assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
        f.h.github.set_checks(&head, ChecksState::Success);
        if on_github {
            yulius_app::ports::github::GitHubPort::merge_pull(
                f.h.github.as_ref(),
                &Secret("token-carol".into()),
                "Quartio-Studio",
                "shop",
                1,
                "merge",
                &head,
            )
            .await?;
            if found == Found::Reconciled {
                f.h.app.reconcile_deliveries().await;
            } else {
                let page = f
                    .req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob)
                    .await?;
                assert_eq!(page.status, StatusCode::OK, "{}", page.body);
            }
        } else {
            let merged = f
                .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
                .await?;
            assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
        }
        let quartio = f.origin_sha("quartio")?;
        assert_eq!(
            git(&f.origin, &["rev-parse", &format!("{quartio}^2")])?,
            head,
            "merged by a merge commit"
        );
        use yulius_app::ports::store::{AuditStore as _, TaskStore as _};
        let task = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;
        let record =
            f.h.store
                .audit_details(
                    &format!("shop#{}", task.number),
                    "delivery.review.models",
                    5,
                )
                .await?;
        assert_eq!(record.len(), 1, "{record:?}");
        assert!(
            record[0].contains(&format!("1 claude-fable-5-2 approve at {}", &reviewed[..9])),
            "{record:?}"
        );
        assert_eq!(
            record[0].contains("; no recorded review round approved its head"),
            side_branch,
            "shape {shape}, {found:?}: {record:?}"
        );
    }
    Ok(())
}

/// SPEC-493 CIR-001, CIR-002 (AC-10): a task that starts its review with its work committed and
/// its review step still open gets its head pushed, its pull request opened and a Yulius CI run
/// for that head queued at once, before any poll; a review started again at a newer head pushes
/// it and that head's run supersedes the older one. With uncommitted changes, or a spec that is
/// not valid, the reviewer starts, nothing is pushed, and the agent is told why.
#[tokio::test]
async fn a_review_start_delivers_the_head_and_queues_its_run() -> TestResult {
    use yulius_app::ports::store_ext::CiStore;
    use yulius_domain::ci::RunState;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let sandbox = SandboxId::parse("s1")?;
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut profile: Value = serde_json::from_str(&record.profile_json)?;
    profile["gate"] = json!("true");
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "v1", "Feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: TaskId::parse("t1")?,
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
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
    let review = || async {
        let start = json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Agent",
            "tool_input": {"description": "Review", "prompt": "Review the change.",
                "subagent_type": "yulius-reviewer"}, "tool_use_id": "toolu_1"});
        let answer = call(
            &internal,
            Method::POST,
            "/hooks/claude/PreToolUse",
            Some(start),
            &agent,
        )
        .await?;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        let said = answer.body["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert_ne!(
            answer.body["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
        BoxResult::Ok(said)
    };
    let runs_at = |head: String| {
        let store = f.h.store.clone();
        let sandbox = sandbox.clone();
        async move { store.ci_runs_for_sha(&sandbox, &head).await }
    };
    let wait_for_run = |head: String| async move {
        for _ in 0..400 {
            if !runs_at(head.clone()).await?.is_empty() {
                return BoxResult::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Err(format!("no Yulius CI run was queued at {head}").into())
    };

    let first = git(&wt, &["rev-parse", "HEAD"])?;
    let said = review().await?;
    assert!(
        said.contains("Yulius CI tests it while the review"),
        "{said}"
    );
    wait_for_run(first.clone()).await?;
    assert_eq!(
        f.origin_sha("feat/1-checkout")?,
        first,
        "the head was pushed"
    );
    assert!(
        f.h.github
            .writes()
            .contains(&"create_pull #1 dev<-feat/1-checkout".to_owned()),
        "{:?}",
        f.h.github.writes()
    );

    // Uncommitted work: the reviewer starts, nothing goes out, and the agent hears why.
    std::fs::write(wt.join("feature.txt"), "v2, not committed")?;
    let said = review().await?;
    assert!(said.contains("uncommitted changes"), "{said}");
    assert_eq!(f.origin_sha("feat/1-checkout")?, first);

    // Committed: the newer head goes out, and its run supersedes the first one.
    git(&wt, &["commit", "-qam", "Feature v2"])?;
    let second = git(&wt, &["rev-parse", "HEAD"])?;
    review().await?;
    wait_for_run(second.clone()).await?;
    assert_eq!(f.origin_sha("feat/1-checkout")?, second);
    let older = runs_at(first).await?;
    assert!(
        older.iter().all(|run| run.state == RunState::Cancelled),
        "{older:?}"
    );

    // A spec that is not valid, committed: the agent hears why, and nothing goes out.
    commit_file(&wt, SPEC_PATH, "# not a spec\n", "Spec broken")?;
    let said = review().await?;
    assert!(said.contains("the spec is not valid"), "{said}");
    assert_eq!(f.origin_sha("feat/1-checkout")?, second);
    Ok(())
}

/// Review round 1 of SPEC-493 fix 3: a review's start syncs CI beside the 60 s loop, so two
/// syncs of one sandbox may overlap. Each reads the runs before it queues, so both would queue the
/// same head. One sync of a sandbox runs at a time, and the head is queued once.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn two_ci_syncs_of_a_sandbox_queue_a_head_once() -> TestResult {
    use yulius_app::ports::store_ext::CiStore;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let sandbox = SandboxId::parse("s1")?;
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut profile: Value = serde_json::from_str(&record.profile_json)?;
    profile["gate"] = json!("true");
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "v1", "Feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    // The first sync is held once it read the runs, before it queues.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (reached, planned) = std::sync::mpsc::channel::<()>();
    yulius_app::race::on(
        yulius_app::race::Point::CiSyncPlanned,
        std::path::Path::new("s1"),
        move || {
            reached.send(()).ok();
            held.recv_timeout(Duration::from_secs(20)).ok();
        },
    );
    let sandbox_record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let (app, first_record) = (f.h.app.clone(), sandbox_record.clone());
    let first = tokio::spawn(async move { app.ci_sync_sandbox(&first_record).await });
    tokio::task::spawn_blocking(move || planned.recv_timeout(Duration::from_secs(20))).await??;
    let (app, second_record) = (f.h.app.clone(), sandbox_record.clone());
    let second = tokio::spawn(async move { app.ci_sync_sandbox(&second_record).await });
    // The second goes on, or waits for the first, for a moment; then the first is let go.
    tokio::time::sleep(Duration::from_millis(300)).await;
    release.send(())?;
    first.await??;
    second.await??;
    let runs = f.h.store.ci_runs_for_sha(&sandbox, &head).await?;
    assert_eq!(runs.len(), 1, "{runs:?}");
    Ok(())
}

async fn start_delivery_attempt(f: &Fixture) -> TestResult {
    let task = TaskId::parse("t1")?;
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: task.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
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
    // A live run is a running task (an armed merge lives with it, SPEC-483 D6).
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    Ok(())
}

/// What `delivery_merge` answers for the reviewed head after `follow` changed the branch: whether
/// it refused, its text, and the newest armed merge's audit entries. The task's pull request is open with `feature.rs` (30 lines) and its
/// spec; the head at that point is the reviewed one; the checks are pending (an accepted call
/// arms the merge).
async fn deliver_after_follow_up(
    follow: impl FnOnce(&Fixture, &Path) -> BoxResult<()>,
) -> BoxResult<(bool, String, String)> {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout", "dev")
        .await?;
    let body: String = (1..=30).map(|n| format!("pub fn f{n}() {{}}\n")).collect();
    commit_file(&wt, "src/feature.rs", &body, "Feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let reviewed = git(&wt, &["rev-parse", "HEAD"])?;
    follow(&f, &wt)?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    start_delivery_attempt(&f).await?;
    let internal = yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let agent = Creds {
        bearer: Some("tok-agent".into()),
        ..Creds::default()
    };
    let answer = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "delivery_merge",
            "arguments": {"reviewed_head": reviewed, "review_summary": "approve"}}}),
        ),
        &agent,
    )
    .await?;
    let refused = answer.body["result"]["isError"] == true;
    let text = answer.body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    use yulius_app::ports::store::{AuditStore as _, TaskStore as _};
    let task = f.h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;
    let audited =
        f.h.store
            .audit_details(
                &format!("shop#{}", task.number),
                "delivery.agent.merge_armed",
                5,
            )
            .await?
            .join("\n");
    Ok((refused, text, audited))
}

fn lines(n: u32, tag: &str) -> String {
    (1..=n)
        .map(|i| format!("pub fn {tag}{i}() {{}}\n"))
        .collect()
}

fn assert_follow_up_accepted((refused, said, audited): (bool, String, String), counted: &str) {
    assert!(!refused, "{said}");
    assert!(said.contains(counted), "the answer: {said}");
    assert!(audited.contains(counted), "the audit entry: {audited}");
}

/// SPEC-493 APR-001 to APR-004 (AC-17): an approval carries across a follow-up of at most 20
/// product lines, counted commit by commit, with tests and documents not counted; more, a file of
/// the reviewed change reverted out of it, or a conflicted merge, needs a new review. Accepted
/// calls say what was counted.
#[tokio::test]
async fn an_approval_carries_across_a_small_follow_up() -> TestResult {
    // 20 product lines, and 200 lines of tests and documents.
    assert_follow_up_accepted(
        deliver_after_follow_up(|_, wt| {
            commit_file(wt, "src/extra.rs", &lines(20, "e"), "Nits")?;
            commit_file(wt, "tests/it.rs", &lines(150, "t"), "Tests")?;
            commit_file(wt, "docs/x.md", &"line\n".repeat(50), "Docs")
        })
        .await?,
        "since then 20 product lines in 1 file: src/extra.rs 20",
    );
    // 21: refused, with the count.
    let (refused, said, _) =
        deliver_after_follow_up(|_, wt| commit_file(wt, "src/extra.rs", &lines(21, "e"), "Nits"))
            .await?;
    assert!(refused, "{said}");
    assert!(
        said.contains("21 product lines in 1 file: src/extra.rs 21"),
        "{said}"
    );
    // Two commits of 12: 24.
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        commit_file(wt, "src/a.rs", &lines(12, "a"), "One")?;
        commit_file(wt, "src/b.rs", &lines(12, "b"), "Two")
    })
    .await?;
    assert!(refused && said.contains("24 product lines"), "{said}");
    Ok(())
}

#[tokio::test]
async fn follow_up_approval_counts_rust_and_git_diff_inputs() -> TestResult {
    // A Rust file's test module does not count, its code before it does.
    assert_follow_up_accepted(
        deliver_after_follow_up(|_, wt| {
            let tested = format!(
                "{}#[cfg(test)]\nmod tests {{\n{}}}\n",
                lines(30, "f"),
                lines(40, "t")
            );
            commit_file(wt, "src/feature.rs", &tested, "Tests")?;
            commit_file(wt, "src/extra.rs", &lines(3, "e"), "Nits")
        })
        .await?,
        "since then 3 product lines in 1 file: src/extra.rs 3",
    );
    // A `#[cfg(test)]` that opens no test module hides nothing below it (review round 1).
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        let body = format!("#[cfg(test)]\nmod tests;\n{}", lines(30, "e"));
        commit_file(wt, "src/extra.rs", &body, "Nits")
    })
    .await?;
    assert!(refused && said.contains("32 product lines"), "{said}");
    // The clone's settings do not hide lines: coloured diffs, no prefixes, `-diff` for all.
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        for (key, value) in [("color.diff", "always"), ("diff.noprefix", "true")] {
            git(wt, &["config", key, value])?;
        }
        let common = git(
            wt,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        std::fs::create_dir_all(Path::new(&common).join("info"))?;
        std::fs::write(Path::new(&common).join("info/attributes"), "* -diff\n")?;
        commit_file(wt, "src/extra.rs", &lines(21, "e"), "Nits")
    })
    .await?;
    assert!(refused && said.contains("21 product lines"), "{said}");
    // A line no count can measure (a NUL byte) in product code: refused, naming it.
    let (refused, said, _) =
        deliver_after_follow_up(|_, wt| commit_file(wt, "src/blob.bin", "a\0b\n", "Blob")).await?;
    assert!(
        refused && said.contains("`src/blob.bin` changed a line no count can measure"),
        "{said}"
    );
    // A file of the reviewed change reverted out of it.
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        git(wt, &["rm", "-q", "src/feature.rs"])?;
        git(wt, &["commit", "-q", "-m", "Not this after all"])?;
        Ok(())
    })
    .await?;
    assert!(
        refused && said.contains("`src/feature.rs` of the reviewed change left"),
        "{said}"
    );
    Ok(())
}

#[tokio::test]
async fn follow_up_approval_checks_the_integration_merge() -> TestResult {
    // A clean merge of the integration branch and 5 lines: accepted.
    assert_follow_up_accepted(
        deliver_after_follow_up(|f, wt| {
            advance_base(f, "dev", "other.txt", "other\n")?;
            git(wt, &["fetch", "-q", "origin"])?;
            git(wt, &["merge", "-q", "--no-edit", "origin/dev"])?;
            commit_file(wt, "src/extra.rs", &lines(5, "e"), "Nits")
        })
        .await?,
        "since then 5 product lines in 1 file: src/extra.rs 5",
    );
    // A merge that resolved a conflict: refused.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        advance_base(f, "dev", "src/feature.rs", "pub fn other() {}\n")?;
        git(wt, &["fetch", "-q", "origin"])?;
        let _ = git(wt, &["merge", "-q", "--no-edit", "origin/dev"]);
        std::fs::write(wt.join("src/feature.rs"), lines(30, "f"))?;
        git(wt, &["add", "-A"])?;
        git(wt, &["commit", "-q", "--no-edit"])?;
        Ok(())
    })
    .await?;
    assert!(refused && said.contains("resolved a conflict"), "{said}");
    // A clean merge of the integration branch that changed more than Git's merge: refused.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        advance_base(f, "dev", "other.txt", "other\n")?;
        git(wt, &["fetch", "-q", "origin"])?;
        git(wt, &["merge", "-q", "--no-commit", "origin/dev"])?;
        std::fs::write(wt.join("src/extra.rs"), lines(30, "e"))?;
        git(wt, &["add", "src/extra.rs"])?;
        git(wt, &["commit", "-q", "--no-edit"])?;
        Ok(())
    })
    .await?;
    assert!(refused && said.contains("resolved a conflict"), "{said}");
    Ok(())
}

#[tokio::test]
async fn follow_up_approval_refuses_other_merge_parents() -> TestResult {
    // A merge of a branch other than the integration branch: its lines are not walked past,
    // also when a tag or a local branch named `origin/dev` points at it (review round 1).
    for shadow in [None, Some("tag"), Some("branch")] {
        let (refused, said, _) = deliver_after_follow_up(|_, wt| {
            git(wt, &["checkout", "-q", "-b", "side"])?;
            commit_file(wt, "src/side.rs", &lines(30, "s"), "Side")?;
            git(wt, &["checkout", "-q", "-"])?;
            match shadow {
                Some("tag") => git(wt, &["tag", "origin/dev", "side"]).map(drop)?,
                Some(_) => git(wt, &["branch", "origin/dev", "side"]).map(drop)?,
                None => {}
            }
            git(wt, &["merge", "-q", "--no-ff", "--no-edit", "side"])?;
            Ok(())
        })
        .await?;
        assert!(
            refused && said.contains("merged something other"),
            "{shadow:?}: {said}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn follow_up_approval_requires_current_integration_rules() -> TestResult {
    // The integration branch's `[review] not_product` moves a path out of the count.
    assert_follow_up_accepted(
        deliver_after_follow_up(|f, wt| {
            advance_base(
                f,
                "dev",
                "yulius.toml",
                "[review]\nnot_product = [\"src/gen/\"]\n",
            )?;
            commit_file(wt, "src/gen/x.rs", &lines(25, "g"), "Generated")?;
            commit_file(wt, "src/extra.rs", &lines(2, "e"), "Nits")
        })
        .await?,
        "since then 2 product lines in 1 file: src/extra.rs 2",
    );
    // Without a fresh fetch of the integration branch nothing is counted: refused.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        commit_file(wt, "src/extra.rs", &lines(2, "e"), "Nits")?;
        f.git
            .fetch_fails
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .await?;
    assert!(
        refused && said.contains("could not be refreshed to read the integration branch"),
        "{said}"
    );
    // Nor when origin cannot say where its integration branch is.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        commit_file(wt, "src/extra.rs", &lines(2, "e"), "Nits")?;
        std::fs::rename(&f.origin, f.origin.with_extension("away"))?;
        Ok(())
    })
    .await?;
    assert!(
        refused && said.contains("the integration branch `dev` cannot be read"),
        "{said}"
    );
    // A remote-tracking ref moved right after the fetch (an agent racing it) passes nothing off
    // as the integration branch: the commit origin names is the one compared.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        git(wt, &["checkout", "-q", "-b", "side"])?;
        commit_file(wt, "src/side.rs", &lines(30, "s"), "Side")?;
        let side = git(wt, &["rev-parse", "HEAD"])?;
        git(wt, &["checkout", "-q", "-"])?;
        git(wt, &["merge", "-q", "--no-ff", "--no-edit", "side"])?;
        if let Ok(mut forged) = f.git.forged_after_fetch.lock() {
            *forged = Some(("refs/remotes/origin/dev".to_owned(), side));
        }
        Ok(())
    })
    .await?;
    assert!(refused && said.contains("merged something other"), "{said}");
    Ok(())
}

#[tokio::test]
async fn follow_up_approval_counts_files_loaded_by_product_code() -> TestResult {
    // A fixture that product code now builds in by its path counts, and the line that does it
    // refuses (review round 4).
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        commit_file(wt, "src/fixtures/evil.rs", &lines(30, "x"), "Fixture")?;
        commit_file(
            wt,
            "src/extra.rs",
            "#[path = \"fixtures/evil.rs\"]\nmod evil;\n",
            "Use it",
        )
    })
    .await?;
    assert!(
        refused && said.contains("`src/extra.rs` adds a line that compiles a file in"),
        "{said}"
    );
    // A test file a new import names counts as product code.
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        commit_file(
            wt,
            "web/__tests__/helper.js",
            &"export const x = 1;\n".repeat(25),
            "Helper",
        )?;
        commit_file(
            wt,
            "web/app.js",
            "import { x } from './__tests__/helper';\n",
            "Use it",
        )
    })
    .await?;
    assert!(refused && said.contains("26 product lines"), "{said}");
    // A new line that loads code makes the test files changed beside it count (round 5).
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        commit_file(wt, "tests/evil.py", &"x = 1\n".repeat(30), "Helper")?;
        commit_file(wt, "app/run.py", "from tests.evil import run\n", "Use it")
    })
    .await?;
    assert!(refused && said.contains("31 product lines"), "{said}");
    // A test module faked in a comment hides nothing (review round 2).
    let (refused, said, _) = deliver_after_follow_up(|_, wt| {
        let body = format!("/*\n#[cfg(test)]\nmod t {{\n*/\n{}", lines(30, "e"));
        commit_file(wt, "src/extra.rs", &body, "Nits")
    })
    .await?;
    assert!(refused && said.contains("34 product lines"), "{said}");
    // A `[review]` table that does not parse refuses, naming it.
    let (refused, said, _) = deliver_after_follow_up(|f, wt| {
        advance_base(f, "dev", "yulius.toml", "[review]\nproducts = [\"src/\"]\n")?;
        commit_file(wt, "src/extra.rs", &lines(2, "e"), "Nits")
    })
    .await?;
    assert!(
        refused && said.contains("`[review]` of yulius.toml"),
        "{said}"
    );
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
    start_delivery_attempt(&f).await?;
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

    // SPEC-493 REV-005: a review round is recorded for the pull request; the delivery view
    // lists it, and the merge records who wrote and who reviewed.
    {
        use yulius_app::ports::store::CoordinationStore;
        let task_id = yulius_domain::ids::TaskId::parse("t1")?;
        f.h.store
            .insert_review_round(&yulius_app::model::ReviewRoundRecord {
                id: "round-1".into(),
                task_id: task_id.clone(),
                round: 1,
                head: Some(head.clone()),
                agent: "yulius-reviewer".into(),
                model: "claude-fable-5-2".into(),
                reason: "round".into(),
                subagent_id: Some("rev-1".into()),
                model_answered: Some("claude-fable-5-2".into()),
                started_at: yulius_domain::time::Timestamp(1),
                ended_at: Some(yulius_domain::time::Timestamp(2)),
                verdict: Some("approve".into()),
                cut_off: false,
                pr_number: None,
                merged_at: None,
            })
            .await?;
        let rounds = &f.delivery("t1").await?["review_rounds"];
        assert_eq!(rounds[0]["round"], 1, "{rounds}");
        assert_eq!(rounds[0]["model_answered"], "claude-fable-5-2");
        assert_eq!(rounds[0]["verdict"], "approve");
        assert_eq!(rounds[0]["reported"], true);
    }
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
    {
        use yulius_app::ports::store::{AuditStore as _, TaskStore as _};
        let task =
            f.h.store
                .task(&yulius_domain::ids::TaskId::parse("t1")?)
                .await?
                .ok_or("t1")?;
        let record =
            f.h.store
                .audit_details(
                    &format!("shop#{}", task.number),
                    "delivery.review.models",
                    5,
                )
                .await?;
        assert_eq!(record.len(), 1, "{record:?}");
        assert_eq!(
            record[0],
            format!(
                "pull request #1: written by claude-opus-5-5; review rounds: 1 claude-fable-5-2 \
                 approve at {}",
                &head[..9]
            ),
            "approved at the merged head; the agent's run on the default model wrote it"
        );
        // The delivered PR shows its review. A subsequent PR starts with no open rounds.
        let delivered = f.delivery("t1").await?;
        let rounds = delivered["review_rounds"]
            .as_array()
            .ok_or("review rounds")?;
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0]["pr_number"], 1);
        assert_eq!(rounds[0]["head"], head);
        assert_eq!(rounds[0]["verdict"], "approve");
        assert_eq!(rounds[0]["reported"], true);
        assert!(f.h.store.review_rounds(&task.id, true).await?.is_empty());
    }
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
            model: None,
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
    // A live run is a running task (an armed merge lives with it, SPEC-483 D6).
    f.h.store
        .transition_task(
            &TaskId::parse(task)?,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
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
/// into it; new work after it past 20 product lines (SPEC-493 APR-001) needs a new review.
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
    commit_file(&t.wt, "landing.txt", &"v2\n".repeat(21), "Landing v2")?;
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
    // Brought up to date once its own checks passed (SPEC-441 MQ-001).
    f.h.github.set_checks(&head, ChecksState::Success);
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
    f.h.app.keep_union_merges(&sandbox, &wt).await;
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

/// #943 (SPEC-343 WTG-001): the Git overview says whether each task's worktree is on disk,
/// was never created (a task not started yet) or is gone, exactly as its changes can or cannot
/// be read, so the Git page offers them only while they can be.
#[tokio::test]
async fn git_overview_says_whether_a_tasks_worktree_is_there() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    f.add_unstarted_task("t2", "Queued work", "feat/2-queued-work")
        .await?;
    let worktree_of = |body: &Value, id: &str| {
        body["tasks"]
            .as_array()
            .and_then(|tasks| tasks.iter().find(|t| t["task_id"] == id))
            .map(|t| t["worktree"].clone())
    };

    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        worktree_of(&page.body, "t1"),
        Some(json!("present")),
        "{}",
        page.body
    );
    assert_eq!(
        worktree_of(&page.body, "t2"),
        Some(json!("not_created")),
        "{}",
        page.body
    );
    // A task that has not started says so, not that its worktree is gone.
    let changes = f
        .req(Method::GET, "/api/tasks/t2/changes", None, &f.rita)
        .await?;
    assert_eq!(changes.status, StatusCode::CONFLICT, "{}", changes.body);
    assert!(
        !changes.body.to_string().contains("gone"),
        "{}",
        changes.body
    );

    std::fs::remove_dir_all(&wt)?;
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        worktree_of(&page.body, "t1"),
        Some(json!("gone")),
        "{}",
        page.body
    );
    // The branch is still listed with its facts; only its changes can't be read.
    let task = &page.body["tasks"]
        .as_array()
        .and_then(|tasks| tasks.iter().find(|t| t["task_id"] == "t1").cloned())
        .ok_or("t1")?;
    assert_eq!(task["branch"], "feat/1-checkout-api");
    assert_eq!(task["files_changed"], 1);
    let changes = f
        .req(Method::GET, "/api/tasks/t1/changes", None, &f.rita)
        .await?;
    assert_eq!(changes.status, StatusCode::CONFLICT, "{}", changes.body);
    assert!(
        changes.body.to_string().contains("worktree is gone"),
        "{}",
        changes.body
    );
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
            model: None,
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
/// in the web UI): Your turn (stopped with steps open), held prompts, blocked chains and failing
/// checks count; a paused or ended task, or an idle one with nothing open (answered, SPEC-429),
/// does not; a tested pull request answers for a finished task once its plan is complete (#139).
/// The pull request facts are what the Git overview last read.
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
    commit_file(&wt, SPEC_PATH, &finished_spec("t1"), "Spec")?;
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
    set_steps(&f, "t2", &["done", "pending"]).await?;
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

    // Before any Git overview read, t1's pull request is not known; with its plan complete and
    // nothing asked, it answered (SPEC-429).
    assert_eq!(needs().await?, 3);

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
    commit_file(&wt, SPEC_PATH, &finished_spec("t1"), "Spec")?;
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
        // Your turn (steps open) needs you; an achieved goal is Done; a stop asked for is with the
        // ended.
        ("t8", TaskStatus::Running, Some(Activity::Waiting)),
        ("t9", TaskStatus::Running, Some(Activity::Done)),
        ("t10", TaskStatus::Stopping, Some(Activity::Working)),
    ];
    for (id, status, activity) in tasks {
        f.add_task(id, id, &format!("feat/{id}"), "dev").await?;
        set(id, status, activity).await?;
    }
    set_steps(&f, "t8", &["done", "pending"]).await?;

    // Before any Git overview read, t1's pull request is not known; its plan is complete and
    // nothing asks: it answered, counted nowhere (SPEC-429).
    assert_eq!(counts().await?, (1, 4, 2));

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
    // A historical parked goal with activity Done still needs a person for
    // this errored run. Its chain must agree with the parked owner's group.
    set_steps(&f, "t1", &["done", "done"]).await?;
    set("t1", TaskStatus::Parked, Some(Activity::Done)).await?;
    let before_id = TaskId::parse("t1")?;
    let before = f.h.store.task(&before_id).await?.ok_or("predecessor")?;
    let mut successor = before.clone();
    successor.id = TaskId::parse("t-after-error")?;
    successor.number = 91;
    successor.status = TaskStatus::Chained;
    successor.activity = None;
    successor.branch = None;
    successor.worktree_path = None;
    let waiting = f.h.store.insert_task(&successor).await?;
    yulius_app::ports::store_ext::IntakeStore::insert_chain(
        &f.h.store,
        &yulius_app::intake::ChainRecord {
            task_id: waiting.id.clone(),
            after_task_id: Some(before_id.clone()),
            after_number: before.number,
            blocked_reason: None,
            created_at: f.h.app.now(),
            rule: None,
        },
    )
    .await?;
    f.h.app.advance_chains(&before_id).await;
    assert_eq!(
        f.h.store
            .task(&waiting.id)
            .await?
            .ok_or("successor")?
            .status,
        TaskStatus::Chained
    );
    // Once the same predecessor's PR has merged, the chain can start.
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
    f.h.app.advance_chains(&before_id).await;
    assert_ne!(
        f.h.store
            .task(&waiting.id)
            .await?
            .ok_or("successor")?
            .status,
        TaskStatus::Chained
    );
    Ok(())
}

async fn answer_with_a_successor(f: &Fixture) -> BoxResult<(TaskId, TaskId)> {
    use yulius_app::ports::store_ext::IntakeStore as _;
    let before = TaskId::parse("t1")?;
    let child = TaskId::parse("t2")?;
    f.add_task("t1", "Answer the question", "feat/1-answer", "dev")
        .await?;
    f.add_task("t2", "Use the answer", "feat/2-after", "dev")
        .await?;
    let now = f.h.app.now();
    f.h.store
        .transition_task(&child, None, TaskStatus::Chained, None, now)
        .await?;
    f.h.store
        .insert_chain(&yulius_app::intake::ChainRecord {
            task_id: child.clone(),
            after_task_id: Some(before.clone()),
            after_number: 1,
            blocked_reason: None,
            created_at: now,
            rule: None,
        })
        .await?;
    Ok((before, child))
}

/// Chromium startup can hold the final lookup for outstanding browser decisions.
#[derive(Default)]
pub(super) struct HeldBrowserLookup {
    session: Option<Arc<dyn BrowserSession>>,
    publication_reader: Option<Arc<std::sync::Mutex<Option<tokio::task::Id>>>>,
    calls: std::sync::atomic::AtomicUsize,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl HeldBrowserLookup {
    pub(super) fn with_session(session: Arc<dyn BrowserSession>) -> Self {
        Self {
            session: Some(session),
            ..Self::default()
        }
    }

    pub(super) fn for_publication() -> Self {
        Self {
            publication_reader: Some(Arc::default()),
            ..Self::default()
        }
    }

    pub(super) fn hold_lookup_for(
        &self,
        reader: tokio::task::Id,
    ) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        if let Some(target) = &self.publication_reader {
            *target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reader);
        }
        (self.entered.clone(), self.release.clone())
    }

    pub(super) fn hold_publication_lookup(
        &self,
        task: &TaskId,
    ) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        if let Some(reader) = self.publication_reader.clone() {
            yulius_app::race::on(
                yulius_app::race::Point::ArmedNoticePublicationRead,
                Path::new(task.as_str()),
                move || {
                    *reader
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(tokio::task::id());
                },
            );
        }
        (self.entered.clone(), self.release.clone())
    }
}

use yulius_app::ports::browser::{BrowserFailure, BrowserPort, BrowserSession};

#[async_trait]
impl BrowserPort for HeldBrowserLookup {
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        yulius::wiring::browser::UnavailableBrowser
            .ensure(sandbox, profile_dir)
            .await
    }

    async fn restart(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        yulius::wiring::browser::UnavailableBrowser
            .restart(sandbox, profile_dir)
            .await
    }

    async fn running(&self, _: &SandboxId) -> Option<Arc<dyn BrowserSession>> {
        let hold = match &self.publication_reader {
            Some(reader) => reader
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take_if(|watcher| Some(*watcher) == tokio::task::try_id())
                .is_some(),
            None => self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1,
        };
        if hold {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.session.clone()
    }

    async fn reap_idle(&self) -> Vec<SandboxId> {
        Vec::new()
    }

    async fn stop(&self, _: &SandboxId) -> bool {
        false
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        Vec::new()
    }
}

#[tokio::test]
async fn a_parked_detail_bounds_its_initial_browser_lookup_and_keeps_the_recorded_pr() -> TestResult
{
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, _) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let browser = Arc::new(HeldBrowserLookup::default());
    browser.calls.store(1, std::sync::atomic::Ordering::SeqCst);
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser.clone();
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let reading = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
    tokio::time::timeout(Duration::from_secs(5), browser.entered.notified()).await?;
    let detail = tokio::time::timeout(Duration::from_secs(3), reading).await;
    browser.release.notify_one();
    let detail = serde_json::to_value(
        detail.map_err(|_| "browser startup consumed the parked task read budget")???,
    )?;
    assert_eq!(detail["parked_needs_you"], false);
    assert!(detail["needs_you_notice"].is_null(), "{detail}");
    assert_eq!(detail["parked_pr"], json!({"number":1,"open":true}));
    Ok(())
}

#[tokio::test]
async fn parked_mcp_uses_the_visible_plan_when_browser_verification_times_out() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let id = TaskId::parse("t1")?;
    f.add_task("t1", "Finish the plan", "feat/1-finish", "dev")
        .await?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    let caller = crate::task_message::superagent(
        &SandboxId::parse("s1")?,
        &UserId::parse("alice")?,
        "admin",
    );
    let browser = Arc::new(HeldBrowserLookup::default());
    browser.calls.store(1, std::sync::atomic::Ordering::SeqCst);
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser.clone();
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let reading =
        tokio::spawn(async move { app.tool_task_get(&caller, &json!({"task_id":"t1"})).await });
    tokio::time::timeout(Duration::from_secs(5), browser.entered.notified()).await?;
    let row = tokio::time::timeout(Duration::from_secs(3), reading).await;
    browser.release.notify_one();
    let row = row???;
    assert_eq!(row["shows"], "needs_you");
    assert_eq!(row["steps"]["done"], 1);
    assert_eq!(row["steps"]["total"], 2);
    assert_eq!(
        row["wait_reason"],
        "The task is parked with unfinished steps in its plan (1 open, next: \"Step 1\")."
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_successor_waits_if_its_owner_resumes_during_the_final_browser_read() -> TestResult
{
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (before, child) = answer_with_a_successor(&f).await?;
    let browser = Arc::new(HeldBrowserLookup::default());
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser.clone();
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let now = app.now();
    // The resumed task may already have finished a new turn and parked again.
    for parked_again in [false, true] {
        browser.calls.store(0, std::sync::atomic::Ordering::SeqCst);
        f.h.store
            .transition_task(&before, None, TaskStatus::Parked, Some(Activity::Done), now)
            .await?;
        let predecessor = before.clone();
        let checking = app.clone();
        let advance = tokio::spawn(async move { checking.advance_chains(&predecessor).await });
        tokio::time::timeout(Duration::from_secs(5), browser.entered.notified()).await?;
        f.h.store
            .transition_task(
                &before,
                None,
                TaskStatus::Running,
                Some(Activity::Working),
                now,
            )
            .await?;
        if parked_again {
            f.h.store
                .transition_task(
                    &before,
                    None,
                    TaskStatus::Parked,
                    Some(Activity::Waiting),
                    now,
                )
                .await?;
        }
        browser.release.notify_one();
        advance.await?;
        assert_eq!(
            f.h.store.task(&child).await?.ok_or("successor")?.status,
            TaskStatus::Chained
        );
    }
    f.h.store
        .transition_task(&before, None, TaskStatus::Parked, Some(Activity::Done), now)
        .await?;
    app.advance_chains(&before).await;
    assert_ne!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    Ok(())
}

/// An achieved goal can release its chain even when its work produced no pull request.
#[tokio::test]
async fn a_parked_achieved_goal_releases_its_chain_without_a_pr() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (before, child) = answer_with_a_successor(&f).await?;
    let now = f.h.app.now();
    for activity in [Activity::Waiting, Activity::Done] {
        f.h.store
            .transition_task(&before, None, TaskStatus::Parked, Some(activity), now)
            .await?;
        f.h.app.advance_chains(&before).await;
        let status = f.h.store.task(&child).await?.ok_or("successor")?.status;
        assert_eq!(status == TaskStatus::Chained, activity == Activity::Waiting);
    }
    Ok(())
}

/// A cold server reads the old attempt's unfinished-goal marker even with all
/// steps done and a tested PR. An unresolved owner request also holds the chain.
#[tokio::test]
async fn a_historical_parked_goal_keeps_its_owner_and_chain_state_after_restart() -> TestResult {
    use yulius_app::ports::store_ext::NeedsYouStore as _;
    let f = fixture_with(
        &DeliveryProfile::standard(),
        Box::new(|config| config.parked_pr_read_ms = 10_000),
        None,
    )
    .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    // This test owns a fixed external PR. The independent head-replacement
    // fixture exercises live Git discovery and local mergeability.
    *f.h.github.remote.lock().map_err(|_| "remote fixture")? = None;
    edit_pull(&f, 1, |pull| {
        pull.base = "release".into();
        pull.mergeable = Some(true);
    });
    let before = TaskId::parse("t1")?;
    let child = TaskId::parse("t2")?;
    let dir = f.root.path().join("old-park");
    std::fs::create_dir(&dir)?;
    let marker = dir.join("parked-goal-unfinished");
    std::fs::write(&marker, b"")?;
    let now = f.h.app.now();
    let mut attempt = AttemptRecord {
        id: AttemptId::parse("historic-goal")?,
        task_id: before.clone(),
        seq: 1,
        harness: Harness::ClaudeCode,
        account_id: None,
        provider_session_id: None,
        launch_kind: LaunchKind::New,
        model: None,
        state: AttemptState::Killed,
        run_dir: dir.to_string_lossy().into_owned(),
        token_hash: "synthetic".into(),
        host_token: Sealed(vec![0]),
        ptyhost_pid: None,
        child_pid: None,
        started_at: now,
        ended_at: Some(now),
        exit_code: Some(0),
        exit_reason: Some("parked".into()),
    };
    f.h.store.insert_attempt(&attempt).await?;
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            now,
        )
        .await?;
    let fresh = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    let parked = f.h.store.task(&before).await?.ok_or("parked")?;
    let summary = fresh.task_detail(&owner, parked.id.as_str()).await?;
    assert!(summary.summary.goal_unfinished);
    assert_eq!(summary.summary.parked_needs_you, Some(true));
    fresh.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    let saved = dir.with_extension("saved");
    std::fs::rename(&dir, &saved)?;
    assert_eq!(
        fresh
            .task_detail(&owner, parked.id.as_str())
            .await?
            .summary
            .parked_needs_you,
        Some(true)
    );
    fresh.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "a missing run directory cannot prove the goal finished"
    );
    std::fs::write(&dir, b"not a directory")?;
    fresh.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    std::fs::remove_file(&dir)?;
    std::fs::rename(saved, &dir)?;
    std::fs::remove_file(marker)?;
    f.h.store
        .set_task_needs_you(
            &before,
            Some(&yulius_app::needs_you::NeedsYouRecord {
                action: "Choose the target".into(),
                happened: None,
                problem: None,
                decision: true,
                link: None,
                at: now.0,
                cause: None,
                named: Default::default(),
            }),
            now,
        )
        .await?;
    fresh.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    assert_eq!(
        fresh
            .task_detail(&owner, parked.id.as_str())
            .await?
            .summary
            .parked_needs_you,
        Some(true)
    );
    f.h.store.set_task_needs_you(&before, None, now).await?;
    // The directory itself is readable. Asking for its marker exceeds the OS
    // path limit and must not be mistaken for a verified absent marker.
    let mut long_dir = dir.to_string_lossy().into_owned();
    let limit = usize::try_from(nix::libc::PATH_MAX)?;
    long_dir.push_str(&"/.".repeat((limit - long_dir.len() - 1) / 2));
    assert!(std::fs::metadata(&long_dir)?.is_dir());
    let unreadable = Path::new(&long_dir).join("parked-goal-unfinished");
    assert_ne!(
        std::fs::symlink_metadata(unreadable)
            .err()
            .ok_or("expected an unreadable marker")?
            .kind(),
        std::io::ErrorKind::NotFound
    );
    attempt.seq += 1;
    attempt.id = AttemptId::parse("unreadable-goal-marker")?;
    attempt.token_hash = attempt.id.to_string();
    attempt.run_dir = long_dir;
    f.h.store.insert_attempt(&attempt).await?;
    fresh.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    attempt.seq += 1;
    attempt.id = AttemptId::parse("readable-goal-marker")?;
    attempt.token_hash = attempt.id.to_string();
    attempt.run_dir = dir.to_string_lossy().into_owned();
    f.h.store.insert_attempt(&attempt).await?;
    assert_eq!(
        fresh
            .task_detail(&owner, parked.id.as_str())
            .await?
            .summary
            .parked_needs_you,
        Some(false)
    );
    fresh.advance_chains(&before).await;
    assert_ne!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_achieved_goal_waits_for_its_failed_ordinary_run() -> TestResult {
    use yulius_domain::ci::{CiRef, RunState};
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    // Without instance CI the remote reports its own checks. Persisted ordinary
    // failures still matter while that report is pending.
    let mut config = f.h.app.config.clone();
    config.ci.enabled = false;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    let before = TaskId::parse("t1")?;
    let child = TaskId::parse("t2")?;
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    // This test owns a fixed external PR. The independent head-replacement
    // fixture exercises live Git discovery and local mergeability.
    *f.h.github.remote.lock().map_err(|_| "remote fixture")? = None;
    edit_pull(&f, 1, |pull| {
        pull.base = "release".into();
        pull.mergeable = Some(true);
    });
    f.h.github.set_checks(&head, ChecksState::Pending);
    insert_ci_attempt(
        &f.h.store,
        "ordinary-red",
        &head,
        CiRef::PullRequest(1),
        RunState::Failed,
        10,
        &[],
    )
    .await?;
    app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    // With CI enabled, another pending ref at the same SHA is the newest
    // commit-level report. It still cannot override this PR's failed run.
    insert_ci_attempt(
        &f.h.store,
        "other-ref",
        &head,
        CiRef::Branch("other".into()),
        RunState::Queued,
        11,
        &[],
    )
    .await?;
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    insert_ci_attempt(
        &f.h.store,
        "ordinary-green",
        &head,
        CiRef::PullRequest(1),
        RunState::Passed,
        12,
        &[],
    )
    .await?;
    // The parked detail reads the exact PR reference. A peer at the same
    // commit still has a pending run, while the owner's rerun just passed.
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let detail = f.h.app.task_detail(&owner, before.as_str()).await?;
    assert_eq!(detail.summary.parked_needs_you, Some(false));
    f.h.github.set_checks(&head, ChecksState::Failure);
    app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    f.h.github.set_checks(&head, ChecksState::Success);
    app.advance_chains(&before).await;
    assert_ne!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_successor_waits_for_the_replacement_ordinary_pr() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture_with(
        &DeliveryProfile::standard(),
        Box::new(|config| config.parked_pr_read_ms = 10_000),
        None,
    )
    .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    let before = TaskId::parse("t1")?;
    let child = TaskId::parse("t2")?;
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let replacement = yulius_app::ports::github::GhPull {
            number: 2,
            mergeable: Some(true),
            ..pulls[0].clone()
        };
        pulls[0].state = "closed".into();
        pulls[0].merged = true;
        pulls.push(replacement);
    }
    f.h.github.set_checks(&head, ChecksState::Failure);
    // GitHub recorded the merge before Yulius; the new PR is already red.
    let parked = f.h.store.task(&before).await?.ok_or("parked predecessor")?;
    let summary = serde_json::to_value(f.h.app.task_detail(&owner, parked.id.as_str()).await?)?;
    assert_eq!(
        summary["parked_needs_you"], true,
        "replacement is already red"
    );
    assert_eq!(summary["parked_pr"], json!({"number": 2, "open": true}));
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    let record = f.h.store.delivery(&before).await?.ok_or("delivery")?;
    assert!(
        f.h.store
            .transition_delivery(
                &record.stage,
                &yulius_app::delivery::DeliveryRecord {
                    stage: DeliveryStage::MergedIntegration,
                    ..record.clone()
                }
            )
            .await?
    );
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    let summary = serde_json::to_value(f.h.app.task_detail(&owner, parked.id.as_str()).await?)?;
    assert_eq!(
        summary["parked_needs_you"], true,
        "old merge cannot mask replacement"
    );
    assert_eq!(summary["parked_pr"], json!({"number": 2, "open": true}));
    f.h.github.set_checks(&head, ChecksState::Success);
    let summary = serde_json::to_value(f.h.app.task_detail(&owner, parked.id.as_str()).await?)?;
    assert_eq!(summary["parked_needs_you"], false, "replacement settled");
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let auth = owner.clone();
    let reading = tokio::spawn(async move { app.task_detail(&auth, parked.id.as_str()).await });
    tokio::time::timeout(Duration::from_secs(5), f.h.github.checks_entered.notified()).await?;
    f.h.github
        .change_before_branch_list(2, |pr| pr.state = "closed".into())?;
    f.h.github.release_checks(&held);
    let changed = serde_json::to_value(reading.await??)?;
    assert_eq!(
        changed["parked_needs_you"], true,
        "branch changed during verification"
    );
    assert_eq!(changed["parked_pr"], json!({"number":1, "open":false}));
    f.h.app.advance_chains(&before).await;
    assert_ne!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    Ok(())
}

#[tokio::test]
async fn parked_detail_checks_a_local_conflict_only_against_its_integration_base() -> TestResult {
    let f = fixture_with(
        &DeliveryProfile::standard(),
        Box::new(|config| config.parked_pr_read_ms = 10_000),
        None,
    )
    .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("token")?)
            .await?
            .ok_or("owner")?;
    let (wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    f.h.store
        .transition_task(
            &TaskId::parse("t1")?,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    *f.h.github.remote.lock().map_err(|_| "remote fixture")? = None;
    f.h.github.set_checks(&head, ChecksState::Success);
    // This read needs the production merge-base and merge-driver implementation.
    let mut deps = f.h.app.deps.clone();
    deps.git = Arc::new(support::confined_git());
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    edit_pull(&f, 1, |pr| pr.mergeable = Some(false));
    assert_eq!(
        app.task_detail(&owner, "t1")
            .await?
            .summary
            .parked_needs_you,
        Some(false),
        "the integration base merges cleanly with the configured Git drivers"
    );
    advance_base(&f, "dev", "feature.txt", "conflicting addition\n")?;
    git(&wt, &["fetch", "origin", "dev"])?;
    edit_pull(&f, 1, |pr| pr.mergeable = Some(true));
    assert_eq!(
        app.task_detail(&owner, "t1")
            .await?
            .summary
            .parked_needs_you,
        Some(true),
        "the actual integration base now conflicts"
    );
    edit_pull(&f, 1, |pr| pr.base = "release".into());
    assert_eq!(
        app.task_detail(&owner, "t1")
            .await?
            .summary
            .parked_needs_you,
        Some(false),
        "the release base has its own mergeability"
    );
    Ok(())
}

#[tokio::test]
async fn overlapping_parked_status_reads_bound_git_workers_after_timeouts() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    let task = f.h.store.task(&id).await?.ok_or("task")?;
    let mut config = f.h.app.config.clone();
    config.ci.enabled = false;
    // Allow an uncontended local Git read to finish on a busy CI host. Held reads still time out.
    config.parked_pr_read_ms = 5_000;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    *f.h.github.remote.lock().map_err(|_| "remote")? = None;
    edit_pull(&f, 1, |pull| pull.mergeable = Some(true));
    // Observe a genuine failure before Git stalls. Recovery needs a completed read.
    f.h.github.set_checks(&head, ChecksState::Failure);
    let failure = app
        .task_detail(&owner, task.id.as_str())
        .await?
        .summary
        .needs_you_notice
        .ok_or("expected a notice for the failing PR")?;
    assert!(failure.episode.starts_with("pr_fail:1:"), "{failure:?}");
    f.h.github.set_checks(&head, ChecksState::Success);
    let hold = Arc::new(ParkedGitHold {
        started: Default::default(),
        released: tokio::sync::Semaphore::new(0),
        finishing: tokio::sync::Semaphore::new(0),
    });
    *f.git.parked_git_hold.lock().map_err(|_| "git hold")? = Some(hold.clone());
    let checked = async {
        for _ in 0..2 {
            let reads = (0..12).map(|_| app.task_detail(&owner, task.id.as_str()));
            for row in futures::future::try_join_all(reads).await? {
                assert_eq!(row.summary.parked_needs_you, Some(true));
                assert_eq!(row.summary.needs_you_notice.as_ref(), Some(&failure));
            }
        }
        Ok::<_, AppError>(())
    }
    .await;
    let started = hold.started.load(std::sync::atomic::Ordering::SeqCst);
    hold.released.close();
    checked?;
    assert!(
        (1..=8).contains(&started),
        "{started} Git workers started before any finished"
    );
    // The native work received its release and is still finishing. Its permits remain in use.
    let draining = app.task_detail(&owner, task.id.as_str()).await?.summary;
    *f.git.parked_git_hold.lock().map_err(|_| "git hold")? = None;
    hold.finishing.close();
    assert_eq!(draining.parked_needs_you, Some(true));
    assert_eq!(draining.needs_you_notice.as_ref(), Some(&failure));
    tokio::time::timeout(
        Duration::from_secs(30),
        app.wait_for_parked_git_workers_for_test(),
    )
    .await??;
    let recovered = app.task_detail(&owner, task.id.as_str()).await?.summary;
    assert_eq!(recovered.parked_needs_you, Some(false));
    assert!(recovered.needs_you_notice.is_none());
    Ok(())
}

/// A parked Waiting predecessor releases its chain only when all its PR checks can be read.
#[tokio::test]
async fn a_parked_predecessor_waits_for_the_complete_check_report() -> TestResult {
    use yulius_app::ports::store_ext::CiStore;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    // The fixture changes the remote PR metadata independently of local Git.
    *f.h.github.remote.lock().map_err(|_| "remote fixture")? = None;
    let before = TaskId::parse("t1")?;
    let child = TaskId::parse("t2")?;
    // A running agent that ended its turn with a red PR still holds this chain.
    f.h.github.set_checks(&head, ChecksState::Failure);
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    // A recorded PrOpen stage by itself cannot release a parked idle answer.
    // GitHub closed this PR without merging before the stored stage caught up.
    edit_pull(&f, 1, |pull| pull.state = "closed".into());
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    edit_pull(&f, 1, |pull| pull.state = "open".into());
    f.h.github.set_checks_report(
        &head,
        ChecksReport {
            state: ChecksState::Success,
            unreadable: vec![ChecksSource::WorkflowRuns],
        },
    );
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    f.h.github.set_checks(&head, ChecksState::Success);
    let hold = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let predecessor = before.clone();
    let advancing = tokio::spawn(async move { app.advance_chains(&predecessor).await });
    tokio::time::timeout(Duration::from_secs(5), f.h.github.checks_entered.notified()).await?;
    edit_pull(&f, 1, |pull| pull.head_sha = "replacement-head".into());
    f.h.github
        .set_checks("replacement-head", ChecksState::Failure);
    f.h.github.release_checks(&hold);
    tokio::time::timeout(Duration::from_secs(5), advancing).await??;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "the verified green head was replaced during its checks read"
    );
    edit_pull(&f, 1, |pull| pull.head_sha.clone_from(&head));
    // The real green head is published. On a release base only GitHub can
    // answer mergeability, independently of checkout ancestry.
    edit_pull(&f, 1, |pull| {
        pull.base = "release".into();
        pull.mergeable = None;
    });
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "the release base had unknown mergeability"
    );
    edit_pull(&f, 1, |pull| pull.mergeable = Some(true));
    // A step is reopened while the server is checking a positive PR.
    let read = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let predecessor = before.clone();
    let advance = tokio::spawn(async move { app.advance_chains(&predecessor).await });
    tokio::time::timeout(Duration::from_secs(5), f.h.github.checks_entered.notified()).await?;
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    f.h.github.release_checks(&read);
    advance.await?;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "work reopened during the final PR read"
    );
    set_steps(&f, "t1", &["done", "done"]).await?;
    use yulius_domain::ci::{CiRef, RunState};
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.h.github
        .pull_read_hold
        .lock()
        .map_err(|_| "pull fixture")? = Some(gate.clone());
    let app = f.h.app.clone();
    let predecessor = before.clone();
    let advance = tokio::spawn(async move { app.advance_chains(&predecessor).await });
    tokio::time::timeout(
        Duration::from_secs(5),
        f.h.github.pull_read_entered.notified(),
    )
    .await?;
    gate.notify_one();
    tokio::time::timeout(
        Duration::from_secs(5),
        f.h.github.pull_read_entered.notified(),
    )
    .await?;
    insert_ci_attempt(
        &f.h.store,
        "rerun-red",
        &head,
        CiRef::PullRequest(1),
        RunState::Failed,
        10,
        &[],
    )
    .await?;
    *f.h.github
        .pull_read_hold
        .lock()
        .map_err(|_| "pull fixture")? = None;
    gate.notify_one();
    advance.await?;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "the same head failed during the confirming pull read"
    );
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "the local failure still holds the chain while GitHub reports green"
    );
    insert_ci_attempt(
        &f.h.store,
        "rerun-active",
        &head,
        CiRef::PullRequest(1),
        RunState::Running,
        11,
        &[],
    )
    .await?;
    f.h.app.advance_chains(&before).await;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "a successor cannot rely on the parked PR watcher to settle a running check"
    );
    assert!(
        f.h.store
            .transition_ci_run(
                "rerun-active",
                &[RunState::Running],
                RunState::Passed,
                &Default::default()
            )
            .await?
    );
    let probe = Arc::new(CheckoutProbe::default());
    *f.git.checkout_probe.lock().map_err(|_| "checkout probe")? = Some(probe.clone());
    let app = f.h.app.clone();
    let predecessor = before.clone();
    let advance = tokio::spawn(async move { app.advance_chains(&predecessor).await });
    tokio::time::timeout(Duration::from_secs(5), probe.entered.notified()).await?;
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            f.h.app.now(),
        )
        .await?;
    probe.release.notify_one();
    advance.await?;
    assert_eq!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained,
        "the predecessor resumed while the final checkout check waited"
    );
    f.h.store
        .transition_task(
            &before,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.app.advance_chains(&before).await;
    assert_ne!(
        f.h.store.task(&child).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
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
        start_delivery_attempt(&f).await?;
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

/// SPEC-483 D3: a task's change merges into the integration branch only. An agent's own token
/// may change its pull request's base; a pull request pointed at another branch (production)
/// is not merged by the armed merge, cannot be armed, and is refused to a person too, until it
/// goes into the integration branch again.
#[tokio::test]
async fn a_pull_request_pointed_at_another_branch_is_never_merged() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let sandbox = SandboxId::parse("s1")?;
    let retarget = |base: &str| {
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            base.clone_into(&mut pull.base);
        }
    };
    let merged = || {
        f.h.github
            .pulls
            .lock()
            .is_ok_and(|pulls| pulls.iter().any(|p| p.number == 1 && p.merged))
    };
    t.arm().await?;

    // Pointed at production after it was armed, then green: dropped, not merged.
    retarget("main");
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("dropped") && outcome.contains("goes into `main`, not `quartio`"),
        "{outcome}"
    );
    assert!(!merged());
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // The agent cannot merge or arm it, nor can a person.
    let refused = t.merge().await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("goes into `main`, not `quartio`"),
        "{refused}"
    );
    let person = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(person.status, StatusCode::CONFLICT, "{}", person.body);
    assert!(
        person
            .body
            .to_string()
            .contains("does not go into the integration branch"),
        "{}",
        person.body
    );
    assert!(!merged());

    // Back at the integration branch it merges as any other.
    retarget("quartio");
    let done = t.merge().await?;
    assert_eq!(done["isError"], false, "{done}");
    assert_ne!(f.delivery("t1").await?["stage"], "pr_open");
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
    // Yulius looks at it, the checks still run: it only says where the merge stands (SPEC-441
    // MQ-007), which never ends the wait, and then nothing changes.
    f.h.app.merge_armed(&sandbox).await;
    let next = t.wait(first.clone()).await?;
    assert!(
        next["merge_train"]
            .as_str()
            .is_some_and(|n| n.contains("waits for its own Yulius CI run")),
        "{next}"
    );
    assert_eq!(settled(&next, Some(&first))?, None);
    f.h.app.merge_armed(&sandbox).await;
    let still = t.wait(next.clone()).await?;
    assert_eq!(still, next, "unchanged until the server's wait times out");
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

    // A real conflict first: the base adds the same file differently. The head's own checks
    // finish first (SPEC-441 MQ-001).
    t.arm().await?;
    advance_base(f, "quartio", "landing.txt", "base version")?;
    f.h.app.merge_armed(&sandbox).await;
    assert_eq!(
        git(&t.wt, &["rev-parse", "HEAD"])?,
        t.head,
        "checks still run"
    );
    f.h.github.set_checks(&t.head, ChecksState::Success);
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
    advance_base(f, "quartio", "other.txt", "unrelated")?;
    // Not while the head's own checks run (SPEC-441 MQ-001): only once they passed.
    f.h.app.merge_armed(&sandbox).await;
    assert_eq!(
        git(&t.wt, &["rev-parse", "HEAD"])?,
        t.head,
        "checks still run"
    );
    f.h.github.set_checks(&t.head, ChecksState::Success);
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

/// SPEC-358 MSB-008: a merge armed before a restart whose checks passed while Yulius was down
/// merges as the new server starts, not at its delivery loop's first round minutes later.
#[tokio::test]
async fn an_armed_merge_green_during_a_restart_merges_at_the_start() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    f.h.store
        .transition_task(
            &TaskId::parse("t1")?,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    t.arm().await?;
    // Down: the checks pass meanwhile, and no CI run ends in this process to merge it.
    f.h.github.set_checks(&t.head, ChecksState::Success);
    let next = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    next.resume_armed_merges().await;
    assert_eq!(
        f.delivery("t1").await?["stage"],
        "production_done",
        "merged at the start (this profile's only branch)"
    );
    Ok(())
}

/// SPEC-483 D6: an armed merge lives with the run that armed it, by one rule with and without
/// a restart. A task whose run only waits (no capacity, a subscription switch) keeps its armed
/// merge across a restart. A task that was closed loses it: the next pass says so instead of
/// merging the pull request behind the person's back once its checks pass, and a new server
/// does not merge it either.
#[tokio::test]
async fn an_armed_merge_ends_with_its_tasks_run() -> TestResult {
    let t = ArmedTask::new().await?;
    let f = &t.f;
    let (task, sandbox) = (TaskId::parse("t1")?, SandboxId::parse("s1")?);
    let becomes = |status: TaskStatus| {
        f.h.store
            .transition_task(&task, None, status, Some(Activity::Waiting), f.h.app.now())
    };
    t.arm().await?;
    let next_server =
        || yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    let file =
        f.h.app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let saved = || std::fs::read_to_string(&file).unwrap_or_default();

    // Held, or queued to start again by itself (a held run goes through the queue when its
    // capacity returns while every slot is taken): armed in this process and for the next.
    for held in [
        TaskStatus::WaitingCapacity,
        TaskStatus::Switching,
        TaskStatus::Queued,
    ] {
        becomes(held).await?;
        f.h.app.merge_armed(&sandbox).await;
        assert_eq!(t.wait(Value::Null).await?["merge_armed"], true, "{held:?}");
        next_server().restore_armed_merges().await;
        assert!(saved().contains("\"t1\""), "{held:?}: {}", saved());
    }

    // Closed by its person: the armed merge ends, and says why. Of an ended run only what
    // became of its pull request is asked: that its branch moved since is not the reason.
    commit_file(&t.wt, "landing.txt", "v2", "After the armed head")?;
    becomes(TaskStatus::Closed).await?;
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("dropped") && outcome.contains("run ended (closed)"),
        "{outcome}"
    );
    git(&t.wt, &["reset", "--hard", &t.head])?;
    assert!(!saved().contains("\"t1\""), "{}", saved());
    // Its checks pass afterwards: nothing merges, here or in a new server.
    f.h.github.set_checks(&t.head, ChecksState::Success);
    f.h.app.merge_armed(&sandbox).await;
    next_server().resume_armed_merges().await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // The same for a merge that a server finds saved for a task closed while it was down.
    becomes(TaskStatus::Running).await?;
    f.h.github.set_checks(&t.head, ChecksState::Pending);
    t.arm().await?;
    assert!(saved().contains("\"t1\""));
    becomes(TaskStatus::Closed).await?;
    f.h.github.set_checks(&t.head, ChecksState::Success);
    // The restore itself forgets it; nothing is left for a pass to decide.
    next_server().restore_armed_merges().await;
    assert!(!saved().contains("\"t1\""), "{}", saved());
    next_server().resume_armed_merges().await;
    assert_eq!(
        f.delivery("t1").await?["stage"],
        "pr_open",
        "not merged at the start"
    );

    // A pull request a person merged while it was armed, in a task closed afterwards: what
    // the pass says is that it is merged, not that a merge was dropped.
    becomes(TaskStatus::Running).await?;
    f.h.github.set_checks(&t.head, ChecksState::Pending);
    t.arm().await?;
    let by_hand = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(by_hand.status, StatusCode::OK, "{}", by_hand.body);
    let confirmed = t.wait(Value::Null).await?;
    assert_eq!(confirmed["merge_armed"], false, "{confirmed}");
    assert_eq!(confirmed["merge_outcome_head"], t.head, "{confirmed}");
    assert!(
        confirmed["merge_outcome"]
            .as_str()
            .is_some_and(|text| text.contains("merged")),
        "{confirmed}"
    );
    commit_file(&t.wt, "landing.txt", "v3", "After the merge")?;
    becomes(TaskStatus::Closed).await?;
    f.h.app.merge_armed(&sandbox).await;
    let outcome = t.outcome().await?;
    assert!(
        outcome.contains("merged") && !outcome.contains("dropped"),
        "{outcome}"
    );
    Ok(())
}

#[tokio::test]
async fn a_person_http_merge_of_another_head_leaves_the_original_arm_to_the_queue() -> TestResult {
    let t = ArmedTask::new().await?;
    t.arm().await?;
    commit_file(&t.wt, "landing.txt", "v2", "Owner's later change")?;
    git(&t.wt, &["push", "origin", "HEAD"])?;
    let newer = git(&t.wt, &["rev-parse", "HEAD"])?;
    assert_ne!(t.head, newer);
    edit_pull(&t.f, 1, |pull| pull.head_sha = newer.clone());
    t.f.h.github.set_checks(&newer, ChecksState::Success);
    let by_hand =
        t.f.req(Method::POST, "/api/tasks/t1/delivery/merge", None, &t.f.bob)
            .await?;
    assert_eq!(by_hand.status, StatusCode::OK, "{}", by_hand.body);
    let waiting = t.wait(Value::Null).await?;
    assert_eq!(waiting["merge_armed"], true, "{waiting}");
    assert!(waiting["merge_outcome"].is_null(), "{waiting}");
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

/// The same stale snapshot after a cold ordinary decision must stay consumed.
#[tokio::test]
async fn a_second_restore_cannot_rearm_a_decided_cold_generation() -> TestResult {
    let t = ArmedTask::new().await?;
    t.arm().await?;
    let cold = yulius_app::context::App::new(t.f.h.app.config.clone(), t.f.h.app.deps.clone());
    let path = cold
        .config
        .data_dir
        .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let saved = std::fs::read(&path)?;
    cold.restore_armed_merges().await;
    commit_file(&t.wt, "landing.txt", "v2", "Later work")?;
    cold.merge_armed(&SandboxId::parse("s1")?).await;
    let before = cold.agent_wait_state("tok-agent", Some(&t.head)).await?;
    assert!(!before.merge_armed);
    std::fs::write(path, saved)?;
    cold.restore_armed_merges().await;
    let after = cold.agent_wait_state("tok-agent", Some(&t.head)).await?;
    assert!(!after.merge_armed, "cold ordinary arm restored twice");
    assert_eq!(after.merge_outcome, before.merge_outcome);
    Ok(())
}

/// Issue #1564: the ordinary queue's final verdict for the armed head, with the clear of its
/// saved entry refused (a directory where the replacement file is written). Returns the saved
/// file's path and the entry the verdict consumed, still on disk.
async fn decided_with_a_refused_clear(t: &ArmedTask) -> BoxResult<(PathBuf, Vec<u8>)> {
    t.arm().await?;
    let path =
        t.f.h
            .app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let saved = std::fs::read(&path)?;
    let blocker = path.with_extension("json.tmp");
    std::fs::create_dir(&blocker)?;
    commit_file(&t.wt, "landing.txt", "v2", "Later work")?;
    t.f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    let decided = t.outcome().await?;
    assert!(decided.contains("dropped"), "{decided}");
    std::fs::remove_dir(&blocker)?;
    git(&t.wt, &["reset", "--hard", &t.head])?;
    assert_eq!(std::fs::read(&path)?, saved, "the clear was refused");
    Ok((path, saved))
}

fn saved_arms(path: &Path) -> BoxResult<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// SPEC-615 CMA-001 (issue #1564): a fresh server reading the entry of an activation the ordinary
/// queue already decided does not arm it again; the restore clears it.
#[tokio::test]
async fn an_ordinary_verdict_consumes_its_saved_arm_when_the_clear_failed() -> TestResult {
    let t = ArmedTask::new().await?;
    let (path, _) = decided_with_a_refused_clear(&t).await?;
    let fresh = yulius_app::context::App::new(t.f.h.app.config.clone(), t.f.h.app.deps.clone());
    fresh.restore_armed_merges().await;
    let state = fresh.agent_wait_state("tok-agent", Some(&t.head)).await?;
    assert!(
        !state.merge_armed,
        "the consumed activation was armed again"
    );
    assert_eq!(saved_arms(&path)?, json!([]));
    Ok(())
}

/// SPEC-615 CMA-002: a re-arm of the same head after that verdict is a new activation; a fresh
/// server restores it with the place it was saved with.
#[tokio::test]
async fn a_later_rearm_of_the_same_head_keeps_its_place_across_a_restart() -> TestResult {
    let t = ArmedTask::new().await?;
    let (path, saved) = decided_with_a_refused_clear(&t).await?;
    let consumed = serde_json::from_slice::<Value>(&saved)?[0]["armed_at"]
        .as_i64()
        .ok_or("old stamp")?;
    t.f.h
        .clock
        .0
        .fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
    t.arm().await?;
    let rearmed = saved_arms(&path)?[0]["armed_at"]
        .as_i64()
        .ok_or("new stamp")?;
    assert!(rearmed > consumed, "{rearmed} after {consumed}");
    let fresh = yulius_app::context::App::new(t.f.h.app.config.clone(), t.f.h.app.deps.clone());
    fresh.restore_armed_merges().await;
    let state = fresh.agent_wait_state("tok-agent", Some(&t.head)).await?;
    assert!(state.merge_armed, "the later re-arm was lost");
    assert_eq!(saved_arms(&path)?[0]["armed_at"], rearmed);
    Ok(())
}

/// SPEC-615 CMA-003: while the verdict cannot be read (the read fails, or its detail is damaged),
/// the saved activation stays on disk and cannot be replaced; the restore that reads it again
/// consumes it, and a re-arm then takes a new place.
#[tokio::test]
async fn an_unreadable_verdict_keeps_the_saved_arm_until_it_is_reconciled() -> TestResult {
    let t = ArmedTask::new().await?;
    let (path, saved) = decided_with_a_refused_clear(&t).await?;
    let consumed = serde_json::from_slice::<Value>(&saved)?[0]["armed_at"]
        .as_i64()
        .ok_or("old stamp")?;
    let store = &t.f.h.store;
    store
        .execute_for_tests(
            "CREATE TEMP TABLE verdict AS SELECT id, detail FROM spans \
             WHERE kind = 'wait' AND name = 'merge_armed'",
        )
        .await?;
    let ctx = t.f.h.app.agent_context("tok-agent").await?;
    let args = json!({"reviewed_head": t.head, "review_summary": "approve"});
    for damage in [
        "UPDATE spans SET detail = CAST(detail AS BLOB) WHERE id IN (SELECT id FROM verdict)",
        "UPDATE spans SET detail = '{' WHERE id IN (SELECT id FROM verdict)",
    ] {
        std::fs::write(&path, &saved)?;
        store.execute_for_tests(damage).await?;
        let fresh = yulius_app::context::App::new(t.f.h.app.config.clone(), t.f.h.app.deps.clone());
        fresh.restore_armed_merges().await;
        assert_eq!(std::fs::read(&path)?, saved, "lost: {damage}");
        let refused = fresh.tool_delivery_merge(&ctx, &args).await;
        assert!(
            matches!(&refused, Err(AppError::Conflict(why)) if why.contains("still being restored")),
            "{damage}: {refused:?}"
        );
        assert_eq!(std::fs::read(&path)?, saved, "replaced: {damage}");

        store
            .execute_for_tests(
                "UPDATE spans SET detail = (SELECT detail FROM verdict \
                 WHERE verdict.id = spans.id) WHERE id IN (SELECT id FROM verdict)",
            )
            .await?;
        fresh.restore_armed_merges().await;
        let state = fresh.agent_wait_state("tok-agent", Some(&t.head)).await?;
        assert!(!state.merge_armed, "came back after: {damage}");
        assert_eq!(saved_arms(&path)?, json!([]), "{damage}");
    }
    let fresh = yulius_app::context::App::new(t.f.h.app.config.clone(), t.f.h.app.deps.clone());
    fresh.restore_armed_merges().await;
    let accepted = fresh.tool_delivery_merge(&ctx, &args).await?;
    assert_eq!(accepted["armed"], true, "{accepted}");
    let rearmed = saved_arms(&path)?[0]["armed_at"]
        .as_i64()
        .ok_or("new stamp")?;
    assert!(rearmed > consumed, "{rearmed} after {consumed}");
    Ok(())
}

#[tokio::test]
async fn a_newly_reviewed_head_gets_a_new_place_and_an_immediate_merge_clears_it() -> TestResult {
    let t = ArmedTask::new().await?;
    t.arm().await?;
    let app = &t.f.h.app;
    let path = app
        .config
        .data_dir
        .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let stored = || -> BoxResult<Value> { Ok(serde_json::from_slice(&std::fs::read(&path)?)?) };
    let earlier = stored()?[0]["armed_at"].as_i64().ok_or("old stamp")?;
    commit_file(&t.wt, "landing.txt", "reviewed v2", "New review")?;
    let next = git(&t.wt, &["rev-parse", "HEAD"])?;
    t.f.h
        .clock
        .0
        .fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
    t.f.h.github.set_checks(&next, ChecksState::Pending);
    let ctx = app.agent_context("tok-agent").await?;
    let args = json!({"reviewed_head":next,"review_summary":"approve"});
    let accepted = app.tool_delivery_merge(&ctx, &args).await?;
    assert_eq!(accepted["armed"], true, "{accepted}");
    assert!(
        stored()?[0]["armed_at"].as_i64().ok_or("new stamp")? > earlier,
        "new review inherited an older activation"
    );
    t.f.h.github.set_checks(&next, ChecksState::Success);
    let accepted = app.tool_delivery_merge(&ctx, &args).await?;
    assert_eq!(accepted["merged"], true, "{accepted}");
    assert!(
        !app.agent_wait_state("tok-agent", Some(&next))
            .await?
            .merge_armed
    );
    assert_eq!(stored()?, json!([]));
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
    assert!(
        why.contains("step \"Step 1\" is in progress: finish it if this delivery includes it"),
        "{why}"
    );
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

/// SPEC-484 PLN-001 to PLN-003, the way task #465 ran on 2026-11-13 (one pull request per slice;
/// it read "Step 6/6" with nine slices ahead, because a merge was refused while a later slice
/// was a step): a task keeps every slice as a step from the start. The end of a turn delivers
/// nothing while slices are ahead and says how to deliver the finished part; `delivery_merge`
/// delivers and merges it, refused while a step is in progress and the call has a head to
/// deliver; the answers name what is ahead; the task is not Done, so the task chained after it
/// waits; the next slice goes out as a new pull request. Its armed merge is not undone by a
/// step marked in progress, and is dropped, as any armed merge, by a commit on the branch,
/// which is why the armed answers tell the agent to start the next step after the merge.
#[tokio::test]
async fn a_task_of_several_slices_merges_each_with_the_rest_ahead() -> TestResult {
    use yulius_app::ports::store_ext::IntakeStore;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    git(&wt, &["config", "user.name", "Bob"])?;
    git(&wt, &["config", "user.email", "bob@yulius.test"])?;
    commit_file(&wt, "feature.txt", FEATURE, "Slice 1")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let slice1 = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&slice1, ChecksState::Success);
    let said = |result: &Value| {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let progress = async || -> BoxResult<(u64, u64)> {
        let task = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
        assert_eq!(task.status, StatusCode::OK, "{}", task.body);
        Ok((
            task.body["steps"]["done"].as_u64().ok_or("done")?,
            task.body["steps"]["total"].as_u64().ok_or("total")?,
        ))
    };

    // A task chained after this one: it starts when this one is Done.
    f.add_task("t2", "Docs of the API", "feat/2-docs", "dev")
        .await?;
    let t2 = TaskId::parse("t2")?;
    f.h.store
        .transition_task(&t2, None, TaskStatus::Chained, None, Timestamp(5))
        .await?;
    f.h.store
        .insert_chain(&yulius_app::intake::ChainRecord {
            task_id: t2.clone(),
            after_task_id: Some(t1.clone()),
            after_number: 1,
            blocked_reason: None,
            created_at: Timestamp(5),
            rule: None,
        })
        .await?;
    // What the chain does when this task's turn has ended (its agent idle): the successor's
    // status afterwards.
    let successor = async || -> BoxResult<String> {
        f.h.store
            .transition_task(
                &t1,
                None,
                TaskStatus::Running,
                Some(Activity::Waiting),
                Timestamp(6),
            )
            .await?;
        f.h.app.advance_chains(&t1).await;
        let task = f.req(Method::GET, "/api/tasks/t2", None, &f.bob).await?;
        Ok(task.body["status"].as_str().unwrap_or_default().to_owned())
    };

    // Three slices, the first finished. A turn's end delivers the whole plan only.
    set_steps(&f, "t1", &["done", "pending", "pending"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 0, "{:?}", f.h.github.writes());
    let waiting = wait_state(&f, &agent, &slice1).await?;
    let note = waiting["push_problem"].as_str().unwrap_or_default();
    assert!(
        note.contains("2 step(s) are still ahead (next: \"Step 1\")")
            && note.contains("call `delivery_merge` with its reviewed head"),
        "{waiting}"
    );

    // The work under way is finished first.
    set_steps(&f, "t1", &["done", "in_progress", "pending"]).await?;
    let merge = json!({"reviewed_head": slice1, "review_summary": "approve"});
    let held = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    assert_eq!(held["isError"], true, "{held}");
    assert!(
        said(&held).contains(
            "step \"Step 1\" is in progress: finish it if this delivery includes it, or set it \
             back to pending"
        ),
        "{held}"
    );
    assert_eq!(pulls_opened(&f), 0, "nothing opened");

    // The finished part goes out and merges with two slices ahead, and the answer says so.
    set_steps(&f, "t1", &["done", "pending", "pending"]).await?;
    let merged = agent_tool(&f, &agent, "delivery_merge", merge).await?;
    assert_eq!(merged["isError"], false, "{merged}");
    let answer = said(&merged);
    assert!(
        answer.contains("2 step(s) of your plan are still ahead (next: \"Step 1\")"),
        "{answer}"
    );
    assert_eq!(pulls_opened(&f), 1, "{:?}", f.h.github.writes());
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    assert_eq!(progress().await?, (1, 3), "the plan is the whole task");
    // Merged, and not Done: the task chained after it goes on waiting.
    assert_eq!(successor().await?, "chained");

    // The second slice, on the branch brought up to date: committed and finished, with the
    // third ahead.
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;
    commit_file(&wt, "slice2.txt", "second", "Slice 2")?;
    let slice2 = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&slice2, ChecksState::Pending);
    set_steps(&f, "t1", &["done", "done", "pending"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1, "a turn's end still waits for the plan");
    // Armed, and told that the step ahead waits for the merge; so is a second call.
    let wait_first = "1 step(s) of your plan are still ahead (next: \"Step 2\"): they stay open. \
                      Start their work once `yulius wait` reports this merge: a commit on this \
                      branch before that drops the armed merge.";
    let armed = arm_merge(&f, &agent, &slice2).await?;
    assert_eq!(armed["armed"], true, "{armed}");
    assert!(
        armed["next"]
            .as_str()
            .is_some_and(|next| next.contains(wait_first)),
        "{armed}"
    );
    assert_eq!(pulls_opened(&f), 2, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, slice2, "pushed");
    assert_eq!(f.delivery("t1").await?["pr"]["number"], 2);
    let again = arm_merge(&f, &agent, &slice2).await?;
    assert!(
        again["next"]
            .as_str()
            .is_some_and(|next| next.contains("armed already") && next.contains(wait_first)),
        "{again}"
    );

    // The turn ends with the part delivered and armed: nothing is "not delivered", so the
    // `yulius wait` the agent started goes on waiting for the merge.
    f.h.app.auto_open_pr(&t1).await;
    let waiting = wait_state(&f, &agent, &slice2).await?;
    assert_eq!(waiting["push_problem"], Value::Null, "{waiting}");
    assert_eq!(waiting["merge_armed"], true, "{waiting}");

    // A commit for the step ahead, made all the same: it is not pushed under the armed head
    // (the note says what is missing and why), and, as for any armed merge, the reviewed head
    // is no longer the branch's: the merge is dropped.
    commit_file(&wt, "slice3.txt", "third, begun", "Slice 3, begun")?;
    let begun = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, slice2, "not pushed");
    let note = wait_state(&f, &agent, &begun).await?;
    assert!(
        note["push_problem"].as_str().is_some_and(|text| {
            text.contains("(not pushed to its open pull request)")
                && text.contains("1 step(s) are still ahead (next: \"Step 2\")")
        }),
        "{note}"
    );
    f.h.github.set_checks(&slice2, ChecksState::Success);
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    let dropped = wait_state(&f, &agent, &begun).await?;
    assert!(
        dropped["merge_outcome"]
            .as_str()
            .is_some_and(|text| text.contains("The merge you armed was dropped")
                && text.contains(yulius_domain::review::KEEP_APPROVAL)),
        "{dropped}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open", "{dropped}");

    // The commit taken back, the reviewed head is armed again, although the agent has marked
    // the step ahead in progress meanwhile: the pull request already has this head, so the
    // call has nothing to deliver that the step could be part of. Its checks pass and the
    // armed merge lands; its message names the step ahead instead of the task's end.
    git(&wt, &["reset", "--quiet", "--hard", &slice2])?;
    set_steps(&f, "t1", &["done", "done", "in_progress"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        wait_state(&f, &agent, &slice2).await?["push_problem"],
        Value::Null,
        "the note of the commit taken back is gone"
    );
    f.h.github.set_checks(&slice2, ChecksState::Pending);
    let rearmed = arm_merge(&f, &agent, &slice2).await?;
    assert_eq!(rearmed["armed"], true, "{rearmed}");
    f.h.github.set_checks(&slice2, ChecksState::Success);
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    let landed = wait_state(&f, &agent, &slice2).await?;
    assert_eq!(
        f.delivery("t1").await?["stage"],
        "merged_integration",
        "{landed}"
    );
    assert!(
        f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull #2")),
        "{:?}",
        f.h.github.writes()
    );
    let outcome = landed["merge_outcome"].as_str().unwrap_or_default();
    assert!(
        outcome.starts_with("Yulius merged pull request #2 into `dev`")
            && outcome.contains("then go on with the next step")
            && outcome.contains("1 step(s) of your plan are still ahead (next: \"Step 2\")")
            && outcome.contains("is delivered next, by a `delivery_merge` of its own")
            && !outcome.contains("end with your summary"),
        "{landed}"
    );
    assert_eq!(progress().await?, (2, 3));
    assert_eq!(successor().await?, "chained", "two of three slices");

    // Work found later adds steps, and the task says what its plan had at first (PLN-010).
    let task = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
    assert_eq!(
        task.body["steps"]["first_total"],
        Value::Null,
        "{}",
        task.body
    );
    set_steps(
        &f,
        "t1",
        &["done", "done", "in_progress", "pending", "pending"],
    )
    .await?;
    let task = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
    assert_eq!(
        (
            &task.body["steps"]["total"],
            &task.body["steps"]["first_total"]
        ),
        (&json!(5), &json!(3)),
        "{}",
        task.body
    );

    // Every step finished on the merged head, and the spec still says in-progress: the task has
    // not said that nothing is left (SPEC-491 DON-004), so the one chained after it waits.
    set_steps(&f, "t1", &["done", "done", "done", "skipped", "skipped"]).await?;
    assert_eq!(successor().await?, "chained", "its spec says in-progress");
    // Its spec says done, read at the turn's end: the task is Done, and the one chained after
    // it no longer waits.
    commit_file(&wt, SPEC_PATH, &finished_spec("t1"), "Spec: done")?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    f.h.app.sync_steps_from_spec(&task).await?;
    assert_ne!(successor().await?, "chained");
    Ok(())
}

/// SPEC-484 PLN-002: the hold of a step in progress belongs to delivering a head. A head the
/// open pull request already has is merged whatever step was started since, by the agent's call
/// and by a person's merge alike, and the delivery view offers the merge it will get.
#[tokio::test]
async fn a_head_the_pull_request_already_has_merges_whatever_step_was_started_since() -> TestResult
{
    for by_person in [false, true] {
        let f = fixture(&DeliveryProfile::standard()).await?;
        let wt = f
            .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
            .await?;
        commit_file(&wt, "feature.txt", "hello", "Add feature")?;
        commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
        let head = git(&wt, &["rev-parse", "HEAD"])?;
        f.h.github.set_checks(&head, ChecksState::Success);
        let agent = live_agent(&f, "t1", "tok-agent").await?;
        set_steps(&f, "t1", &["done", "done"]).await?;
        f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
        assert_eq!(pulls_opened(&f), 1, "{:?}", f.h.github.writes());

        // A step added and started after the pull request opened.
        set_steps(&f, "t1", &["done", "done", "in_progress"]).await?;
        let view = f.delivery("t1").await?;
        assert_eq!(
            (&view["next_actions"], &view["blocked"]),
            (&json!(["merge"]), &json!([])),
            "{view}"
        );
        if by_person {
            let merged = f
                .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
                .await?;
            assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
        } else {
            let merged = agent_tool(
                &f,
                &agent,
                "delivery_merge",
                json!({"reviewed_head": head, "review_summary": "approve"}),
            )
            .await?;
            assert_eq!(merged["isError"], false, "{merged}");
        }
        assert_eq!(
            f.delivery("t1").await?["stage"],
            "merged_integration",
            "by a person: {by_person}"
        );
    }
    Ok(())
}

/// SPEC-484 PLN-002: a plan none of whose steps is finished has no finished part to deliver.
#[tokio::test]
async fn a_merge_call_delivers_nothing_while_no_step_is_finished() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    set_steps(&f, "t1", &["pending", "blocked"]).await?;
    let refused = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");
    let why = refused["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        why.contains("no step of the plan is finished: mark done what this delivery includes"),
        "{why}"
    );
    assert_eq!(pulls_opened(&f), 0, "{:?}", f.h.github.writes());
    Ok(())
}

/// SPEC-425 FUP-001 to FUP-003 (#1074): work committed on a task's branch after its pull request
/// merged is delivered again, as a new pull request of the same task, at the end of a turn and
/// on `delivery_merge`; a head that adds nothing to the merged one (Git's own merges of the
/// integration branch) keeps the task merged, and `delivery_merge` never arms a merge of a pull
/// request that is merged already.
#[tokio::test]
async fn work_committed_after_the_merge_opens_a_new_pull_request() -> TestResult {
    let (f, wt, merged_head) = merged_task().await?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let pulls_opened = || pulls_opened(&f);

    // The merged head itself: nothing to deliver, and the merge call arms nothing.
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "nothing new after the merge");
    let done = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": merged_head, "review_summary": "approve"}),
    )
    .await?;
    let said = done["content"][0]["text"].as_str().unwrap_or_default();
    assert!(said.contains("already merged"), "{done}");
    assert_eq!(
        wait_state(&f, &agent, &merged_head).await?["merge_armed"],
        false,
        "no merge armed on a merged pull request"
    );

    // Git's own merge of the integration branch since adds nothing either.
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "a clean merge of dev is not new work");
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");

    // A follow-up fix: pushed, and a new pull request of the same task opens.
    commit_file(&wt, "fix.txt", "fixed", "Fix after the merge")?;
    let fix = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 2, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, fix, "pushed");
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "pr_open", "{d}");
    assert_eq!(d["pr"]["number"], 2, "{d}");
    assert_eq!(d["pr"]["head"], "feat/1-checkout-api");
    let said =
        f.h.store
            .audit_details("shop#1", "delivery.follow_up", 5)
            .await?;
    assert!(
        said.len() == 1 && said[0].contains("pull request #1 merged"),
        "{said:?}"
    );

    // The merge call takes the new pull request on: armed while its checks run, merged on green.
    f.h.github.set_checks(&fix, ChecksState::Pending);
    let armed = arm_merge(&f, &agent, &fix).await?;
    assert_eq!(armed["armed"], true, "{armed}");
    f.h.github.set_checks(&fix, ChecksState::Success);
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    assert!(
        f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull #2")),
        "{:?}",
        f.h.github.writes()
    );

    // How #2's armed merge ended belongs to #2: a follow-up opened at the end of a turn starts
    // without it, so nothing tells #3 that #2 merged (SPEC-444 live check, #1135).
    let merged = wait_state(&f, &agent, &fix).await?;
    assert!(
        merged["merge_outcome"]
            .as_str()
            .is_some_and(|text| text.contains("#2")),
        "{merged}"
    );
    commit_file(&wt, "fix2.txt", "again", "Second fix after the merge")?;
    let fix2 = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.delivery("t1").await?["pr"]["number"], 3);
    let fresh = wait_state(&f, &agent, &fix2).await?;
    assert_eq!(fresh["merge_outcome"], Value::Null, "{fresh}");

    // The merge call arms the follow-up opened at the end of the turn.
    f.h.github.set_checks(&fix2, ChecksState::Pending);
    let again = arm_merge(&f, &agent, &fix2).await?;
    assert_eq!(again["armed"], true, "{again}");
    assert_eq!(pulls_opened(), 3, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, fix2, "pushed");
    let d = f.delivery("t1").await?;
    assert_eq!(
        (&d["stage"], &d["pr"]["number"]),
        (&json!("pr_open"), &json!(3)),
        "{d}"
    );
    Ok(())
}

/// SPEC-532 (#1299): dev and the task append to the same list since the first PR.
#[tokio::test]
async fn a_follow_up_containing_dev_is_judged_by_its_tree() -> TestResult {
    let (f, wt, _) = merged_task().await?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;

    // Incorporate the squash, then the next migration that landed on dev.
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;
    let integrated = format!("{FEATURE}0060\n");
    advance_base(&f, "dev", "feature.txt", &integrated)?;
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;

    // An ordinary commit beyond the clean merges, but still exactly dev's tree.
    git(&wt, &["commit", "--allow-empty", "-m", "Record follow-up"])?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let covered = arm_merge(&f, &agent, &head).await?;
    assert_eq!(covered["merged"], true, "{covered}");
    assert_eq!(covered["armed"], false, "{covered}");
    assert_eq!(pulls_opened(&f), 1);

    // The first PR's tree lacks both entries. Replaying the append from there conflicts.
    commit_file(
        &wt,
        "feature.txt",
        &format!("{integrated}0059\n"),
        "Add migration",
    )?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let follow_up = arm_merge(&f, &agent, &head).await?;
    assert_eq!(follow_up["armed"], true, "{follow_up}");
    assert_eq!(pulls_opened(&f), 2);
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head);
    Ok(())
}

/// `feature.txt` as task `t1`'s pull request #1 adds it.
const FEATURE: &str = "a\nb\nc\nd\ne\nf\ng\n";

/// A standard-profile task `t1` whose pull request #1 merged (squashed into `dev` on the
/// origin) at its head, with its plan done: the fixture, its worktree and the merged head.
async fn merged_task() -> BoxResult<(Fixture, PathBuf, String)> {
    merged_task_with(Box::new(|_| {})).await
}

/// [`merged_task`] with the configuration adjusted.
async fn merged_task_with(
    configure: support::ConfigureFn,
) -> BoxResult<(Fixture, PathBuf, String)> {
    let f = fixture_with(&DeliveryProfile::standard(), configure, None).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    git(&wt, &["config", "user.name", "Bob"])?;
    git(&wt, &["config", "user.email", "bob@yulius.test"])?;
    commit_file(&wt, "feature.txt", FEATURE, "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_eq!(pulls_opened(&f), 1, "{:?}", f.h.github.writes());
    let merged_head = git(&wt, &["rev-parse", "HEAD"])?;
    merge_on_github(&f, 1, &merged_head).await?;
    Ok((f, wt, merged_head))
}

/// A spec whose task says its work is finished: every Plan item ticked, Evidence, `status: done`.
fn finished_spec(task: &str) -> String {
    valid_spec(task)
        .replace("status: in-progress", "status: done")
        .replace("- [ ] Implement", "- [x] Implement")
        + "\n## Evidence\n- `cargo test`: passed.\n"
}

/// `t1` with `spec` committed, its pull request #1 open and every step finished, and `t2`
/// chained after it: the worktree of `t1` and its head.
async fn delivered_with_a_successor(f: &Fixture, spec: &str) -> BoxResult<(PathBuf, String)> {
    use yulius_app::ports::store_ext::IntakeStore;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", FEATURE, "Add feature")?;
    commit_file(&wt, SPEC_PATH, spec, "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    set_steps(f, "t1", &["done", "done"]).await?;
    f.add_task("t2", "Docs of the API", "feat/2-docs", "dev")
        .await?;
    let t2 = TaskId::parse("t2")?;
    f.h.store
        .transition_task(&t2, None, TaskStatus::Chained, None, Timestamp(5))
        .await?;
    f.h.store
        .insert_chain(&yulius_app::intake::ChainRecord {
            task_id: t2,
            after_task_id: Some(TaskId::parse("t1")?),
            after_number: 1,
            blocked_reason: None,
            created_at: Timestamp(5),
            rule: None,
        })
        .await?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    Ok((wt, head))
}

/// What the task chained after `t1` is once `t1`'s turn has ended (its agent idle) and the
/// chains were looked at. A released task is started in the background, so it may already be
/// past `queued`: the tests ask whether it is still `chained`.
async fn successor_after_the_turn(f: &Fixture) -> BoxResult<String> {
    let t1 = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &t1,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            Timestamp(6),
        )
        .await?;
    f.h.app.advance_chains(&t1).await;
    let task = f.req(Method::GET, "/api/tasks/t2", None, &f.bob).await?;
    Ok(task.body["status"].as_str().unwrap_or_default().to_owned())
}

/// SPEC-491 DON-002: "no check reported" is "none required" only where the project requires
/// none. With a profile that requires checks, a pull request nothing reported on is not tested
/// (as the Git overview reads it since #68), so its task is not Done and the task chained after
/// it waits; once its checks pass it starts.
#[tokio::test]
async fn a_pull_request_nothing_reported_on_is_not_done_where_checks_are_required() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    assert_eq!(
        successor_after_the_turn(&f).await?,
        "chained",
        "nothing reported at its head"
    );
    f.h.github.set_checks(&head, ChecksState::Success);
    assert_ne!(successor_after_the_turn(&f).await?, "chained", "released");
    Ok(())
}

/// SPEC-491 DON-002: a project whose profile requires no checks has "none required": a pull
/// request without any is tested, as before.
#[tokio::test]
async fn a_pull_request_without_checks_is_done_where_none_are_required() -> TestResult {
    let mut profile = DeliveryProfile::standard();
    profile.require_checks = false;
    let f = fixture(&profile).await?;
    delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    assert_ne!(successor_after_the_turn(&f).await?, "chained", "released");
    Ok(())
}

/// SPEC-491 DON-001: where Yulius's clone cannot answer whether a pull request merges (this
/// fixture's Git has no merge check), GitHub's flag decides: a pull request GitHub cannot merge
/// is not tested, whatever its checks say. Its task is not Done and the task chained after it
/// waits; it starts once the pull request can merge again. (Where the clone answers, it decides:
/// `merge_train::github_s_flag_does_not_hold_a_pull_request_the_clone_merges`.)
#[tokio::test]
async fn a_green_pull_request_github_cannot_merge_is_not_done_where_the_clone_cannot_tell()
-> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    let set_mergeable = |mergeable: bool| {
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
        {
            pull.mergeable = Some(mergeable);
        }
    };
    set_mergeable(false);
    assert_eq!(
        successor_after_the_turn(&f).await?,
        "chained",
        "it cannot merge"
    );
    set_mergeable(true);
    assert_ne!(successor_after_the_turn(&f).await?, "chained", "released");
    Ok(())
}

/// SPEC-491 DON-004, DON-005 (the way task #465 ran on 14 November 2026: its plan read 28/28
/// between two slices, because it adds a slice's step when the slice before is merged, while
/// its spec said `in-progress` and named twelve findings still to fix): a task whose spec does
/// not say `done` has work left, however complete its plan reads and whatever its pull request
/// says, tested or merged. The task chained after it waits, and the task is the person's turn,
/// as with open steps; when the spec says `done` it is Done and the chained task starts.
#[tokio::test]
async fn a_task_whose_spec_says_in_progress_is_not_done_with_every_step_finished() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let unfinished = valid_spec("t1").replace("- [ ] Implement", "- [x] Implement");
    let (wt, head) = delivered_with_a_successor(&f, &unfinished).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    assert_eq!(
        successor_after_the_turn(&f).await?,
        "chained",
        "tested, and its spec says in-progress"
    );
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    assert_eq!(
        successor_after_the_turn(&f).await?,
        "chained",
        "merged, and its spec says in-progress"
    );
    let shown = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
    assert_eq!(shown.body["spec_unfinished"], true, "{}", shown.body);
    // Its agent is told what keeps it from Done (DON-007).
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let told = "Every step is finished and the spec says `status: in-progress`, so the task is \
                not Done and tasks waiting for it do not start.";
    let checked = agent_tool(&f, &agent, "spec_check", json!({})).await?;
    assert!(
        checked["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains(told)),
        "{checked}"
    );
    let sandbox = f
        .req(Method::GET, "/api/sandboxes/s1", None, &f.bob)
        .await?;
    assert_eq!(
        sandbox.body["counts"]["needs_attention"], 1,
        "stopped with work left: the person's turn ({})",
        sandbox.body
    );

    // The spec moves (still in-progress): found where it is now, and it still holds the task.
    let moved = "docs/specs/2026-10-08-checkout-api.md";
    git(&wt, &["mv", SPEC_PATH, moved])?;
    git(&wt, &["commit", "--quiet", "-m", "Spec: moved"])?;
    let t1 = TaskId::parse("t1")?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    f.h.app.sync_steps_from_spec(&task).await?;
    assert_eq!(
        f.h.store
            .task(&t1)
            .await?
            .and_then(|t| t.spec_path)
            .as_deref(),
        Some(moved),
        "registered where it is now"
    );
    assert_eq!(
        successor_after_the_turn(&f).await?,
        "chained",
        "moved, still in-progress"
    );
    // The moved spec says done, read at the turn's end (review of SPEC-491, m2).
    commit_file(&wt, moved, &finished_spec("t1"), "Spec: done")?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    f.h.app.sync_steps_from_spec(&task).await?;
    let shown = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
    assert_eq!(shown.body["spec_unfinished"], false, "{}", shown.body);
    let checked = agent_tool(&f, &agent, "spec_check", json!({})).await?;
    assert!(
        checked["content"][0]["text"]
            .as_str()
            .is_some_and(|text| !text.contains("the spec says")),
        "{checked}"
    );
    assert_ne!(successor_after_the_turn(&f).await?, "chained", "released");
    Ok(())
}

/// SPEC-491 DON-005: a task closed with its pull request merged and every step finished, whose
/// spec says its work is not finished, did not end Done: the task chained after it needs a
/// person, as after a task closed with steps open. With a spec that says done it starts.
#[tokio::test]
async fn a_closed_task_whose_spec_says_in_progress_did_not_end_done() -> TestResult {
    for (spec, starts) in [
        (
            valid_spec("t1").replace("- [ ] Implement", "- [x] Implement"),
            false,
        ),
        (finished_spec("t1"), true),
    ] {
        let f = fixture(&DeliveryProfile::standard()).await?;
        let (_wt, head) = delivered_with_a_successor(&f, &spec).await?;
        f.h.github.set_checks(&head, ChecksState::Success);
        let merged = f
            .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
            .await?;
        assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
        let t1 = TaskId::parse("t1")?;
        f.h.store
            .transition_task(&t1, None, TaskStatus::Closed, None, Timestamp(7))
            .await?;
        f.h.app.advance_chains(&t1).await;
        let next = f.req(Method::GET, "/api/tasks/t2", None, &f.bob).await?;
        if starts {
            // Released: the queue may already be starting it in the background.
            assert_ne!(next.body["status"], "chained", "{}", next.body);
        } else {
            assert_eq!(next.body["status"], "chained", "{}", next.body);
            assert_eq!(
                next.body["after"]["blocked"],
                "#1 was closed before it was done. Start this task now or cancel it.",
                "{}",
                next.body
            );
        }
    }
    Ok(())
}

/// SPEC-491 DON-004 (review of SPEC-491, m2): a task whose spec said in-progress and was then
/// deleted (the task turned out to need none) has no spec left to say its work is unfinished:
/// it is decided by its steps alone, Done with its tested pull request, and the chained task
/// starts.
#[tokio::test]
async fn a_task_whose_spec_was_deleted_is_decided_by_its_steps() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let unfinished = valid_spec("t1").replace("- [ ] Implement", "- [x] Implement");
    let (wt, head) = delivered_with_a_successor(&f, &unfinished).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    assert_eq!(successor_after_the_turn(&f).await?, "chained");
    git(&wt, &["rm", "--quiet", SPEC_PATH])?;
    git(&wt, &["commit", "--quiet", "-m", "No spec needed"])?;
    let t1 = TaskId::parse("t1")?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    f.h.app.sync_steps_from_spec(&task).await?;
    assert_eq!(f.h.store.spec_status(&t1).await?, None);
    let shown = f.req(Method::GET, "/api/tasks/t1", None, &f.bob).await?;
    assert_eq!(shown.body["spec_unfinished"], false, "{}", shown.body);
    // The pushed head is the pull request's: its checks passed.
    let pushed = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&pushed, ChecksState::Success);
    f.h.app.auto_open_pr(&t1).await;
    assert_ne!(successor_after_the_turn(&f).await?, "chained", "released");
    Ok(())
}

/// SPEC-491 DON-004 (review of SPEC-491, round 2): the live sync runs every few seconds, and
/// every task is given a spec path at its start whether or not it ever writes one. With no
/// status kept there is nothing a moved or deleted spec could leave stale, so the sync does not
/// look through the specs folder for one: a spec written under another name is found by the
/// next delivery call, as before, not by the sync.
#[tokio::test]
async fn the_sync_looks_for_a_moved_spec_only_while_a_status_is_kept() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    let t1 = TaskId::parse("t1")?;
    let allocated = "docs/specs/2026-10-07-allocated.md";
    f.h.store
        .update_task(
            &t1,
            &yulius_app::ports::store::TaskFieldUpdate {
                spec_path: Some(allocated.into()),
                ..yulius_app::ports::store::TaskFieldUpdate::default()
            },
            Timestamp(3),
        )
        .await?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec under another name")?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    assert!(!f.h.app.sync_steps_from_spec(&task).await?);
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    assert_eq!(task.spec_path.as_deref(), Some(allocated), "not looked for");
    assert_eq!(f.h.store.spec_status(&t1).await?, None);
    // Once a status is kept, a spec missing at its path is looked for and followed.
    f.h.store.set_spec_status(&t1, Some("draft")).await?;
    f.h.app.sync_steps_from_spec(&task).await?;
    let task = f.h.store.task(&t1).await?.ok_or("t1")?;
    assert_eq!(task.spec_path.as_deref(), Some(SPEC_PATH));
    assert_eq!(
        f.h.store.spec_status(&t1).await?.as_deref(),
        Some("in-progress")
    );
    Ok(())
}

/// Pull requests Yulius opened so far.
fn pulls_opened(f: &Fixture) -> usize {
    f.h.github
        .writes()
        .iter()
        .filter(|w| w.starts_with("create_pull"))
        .count()
}

/// Pull request `number` squash-merged on GitHub at `head` (outside Yulius), and followed.
async fn merge_on_github(f: &Fixture, number: u64, head: &str) -> TestResult {
    f.h.github.set_checks(head, ChecksState::Success);
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        number,
        "squash",
        head,
    )
    .await?;
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    Ok(())
}

/// SPEC-425 FUP-002 and FUP-003 (review of #407): what is not new work after the merge opens
/// nothing, and what Yulius cannot judge is said to `delivery_merge`, never "already merged" nor
/// armed: a follow-up merged outside Yulius, an open step, a rewritten branch, GitHub not
/// answering for the merged pull request.
#[tokio::test]
async fn work_after_the_merge_opens_nothing_it_cannot_judge() -> TestResult {
    let (f, wt, merged_head) = merged_task().await?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let merge_text = |result: &Value| {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let merge_call = |head: String| {
        let (f, agent) = (&f, &agent);
        async move {
            agent_tool(
                f,
                agent,
                "delivery_merge",
                json!({"reviewed_head": head, "review_summary": "approve"}),
            )
            .await
        }
    };

    // An open step with new work: blocked by the step, the task stays merged.
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;
    commit_file(
        &wt,
        "feature.txt",
        "A\nb\nc\nd\ne\nf\ng\n",
        "Fix after the merge",
    )?;
    let fix = git(&wt, &["rev-parse", "HEAD"])?;
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    let blocked = merge_call(fix.clone()).await?;
    assert_eq!(blocked["isError"], true, "{blocked}");
    assert!(merge_text(&blocked).contains("is in progress"), "{blocked}");
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    set_steps(&f, "t1", &["done", "done"]).await?;

    // The agent delivered the fix by hand as #2 and it merged outside Yulius: the task's record
    // still names #1, but the head adds nothing to `dev`, so no third pull request opens, even
    // once another change on `dev` touched the same file since.
    git(
        &wt,
        &["push", "--quiet", "origin", "HEAD:feat/1-checkout-api"],
    )?;
    let manual = yulius_app::ports::github::GitHubPort::create_pull(
        f.h.github.as_ref(),
        &Secret("token-bob".into()),
        "Quartio-Studio",
        "shop",
        &yulius_app::ports::github::NewPull {
            title: "fix: after the merge".into(),
            head: "feat/1-checkout-api".into(),
            base: "dev".into(),
            body: String::new(),
        },
    )
    .await?;
    f.h.github.set_checks(&fix, ChecksState::Success);
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        manual.number,
        "squash",
        &fix,
    )
    .await?;
    advance_base(&f, "dev", "feature.txt", "A\nb\nc\nd\ne\nf\nG\n")?;
    git(&wt, &["fetch", "--quiet", "origin"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        pulls_opened(&f),
        2,
        "the hand-made #2 only: {:?}",
        f.h.github.writes()
    );
    let done = merge_call(fix.clone()).await?;
    assert!(merge_text(&done).contains("already merged"), "{done}");
    assert_eq!(wait_state(&f, &agent, &fix).await?["merge_armed"], false);

    // That verdict is kept for the head: GitHub is not asked again at it (it could not answer).
    let pulls_before =
        f.h.github
            .pulls
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default();
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.retain(|p| p.number != 1);
    }
    let kept = merge_call(fix.clone()).await?;
    assert!(merge_text(&kept).contains("already merged"), "{kept}");
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        *pulls = pulls_before;
    }

    // A branch rewritten onto `dev` with new work: Yulius says how to bring the merged head
    // back, and neither pushes nor opens anything.
    git(&wt, &["reset", "--quiet", "--hard", "origin/dev"])?;
    commit_file(&wt, "rewritten.txt", "other", "Rewritten")?;
    let rewritten = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    let refused = merge_call(rewritten.clone()).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    let why = merge_text(&refused);
    assert!(
        why.contains("no longer contains its merged head")
            && why.contains(&format!("git merge --no-edit {merged_head}")),
        "{why}"
    );
    assert_eq!(pulls_opened(&f), 2);
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, fix, "not pushed");
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");

    // GitHub cannot answer for the merged pull request: said, nothing armed or opened.
    git(&wt, &["reset", "--quiet", "--hard", &fix])?;
    commit_file(&wt, "more.txt", "more", "More after the merge")?;
    let back = git(&wt, &["rev-parse", "HEAD"])?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.retain(|p| p.number != 1);
    }
    let unknown = merge_call(back.clone()).await?;
    assert_eq!(unknown["isError"], true, "{unknown}");
    let why = merge_text(&unknown);
    assert!(
        why.contains("whether this head adds to pull request #1, merged already, is unknown")
            && !why.contains("Yulius cannot deliver this head yet: Yulius"),
        "{why}"
    );
    assert_eq!(wait_state(&f, &agent, &back).await?["merge_armed"], false);
    assert_eq!(pulls_opened(&f), 2);
    Ok(())
}

/// SPEC-425 (review of #407, rounds 2 and 3): a follow-up merged by hand from a branch that
/// never merged `dev` stays covered once `dev` changes a file of the merged pull request (what
/// the head changed is read since the merged head, not since the old fork point); a head whose
/// change conflicts with `dev` is unknown and noted for `yulius wait`, the note goes once the
/// head is covered again; a dirty follow-up keeps the task merged; a merged head missing from the
/// clone and a failed fetch are said with their remedy; then the follow-up opens.
#[tokio::test]
async fn a_follow_up_merged_by_hand_is_judged_since_the_merged_head() -> TestResult {
    let (f, wt, _) = merged_task().await?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let merge_call = |head: String| {
        let (f, agent) = (&f, &agent);
        async move {
            agent_tool(
                f,
                agent,
                "delivery_merge",
                json!({"reviewed_head": head, "review_summary": "approve"}),
            )
            .await
        }
    };
    let text = |result: &Value| {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    // The follow-up, delivered and squash-merged by hand as #2; then `dev` edits the merged
    // pull request's file.
    commit_file(&wt, "other.txt", "other", "Follow-up by hand")?;
    let by_hand = git(&wt, &["rev-parse", "HEAD"])?;
    git(
        &wt,
        &["push", "--quiet", "origin", "HEAD:feat/1-checkout-api"],
    )?;
    let manual = yulius_app::ports::github::GitHubPort::create_pull(
        f.h.github.as_ref(),
        &Secret("token-bob".into()),
        "Quartio-Studio",
        "shop",
        &yulius_app::ports::github::NewPull {
            title: "fix: by hand".into(),
            head: "feat/1-checkout-api".into(),
            base: "dev".into(),
            body: String::new(),
        },
    )
    .await?;
    f.h.github.set_checks(&by_hand, ChecksState::Success);
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        manual.number,
        "squash",
        &by_hand,
    )
    .await?;
    advance_base(&f, "dev", "feature.txt", "a\nb\nc\nd\ne\nf\nG\n")?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        pulls_opened(&f),
        2,
        "the hand-made #2 only: {:?}",
        f.h.github.writes()
    );
    let done = merge_call(by_hand.clone()).await?;
    assert!(text(&done).contains("already merged"), "{done}");

    // A change that conflicts with `dev`: unknown, said to the merge call and to `yulius wait`.
    commit_file(
        &wt,
        "feature.txt",
        "a\nb\nc\nd\ne\nf\nH\n",
        "Conflicting follow-up",
    )?;
    let conflicting = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    let note = wait_state(&f, &agent, &conflicting).await?["push_problem"].clone();
    assert!(
        note.as_str()
            .unwrap_or_default()
            .contains("conflicts with `origin/dev`"),
        "{note}"
    );
    let refused = merge_call(conflicting.clone()).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        text(&refused).contains("git merge --no-edit origin/dev"),
        "{refused}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    assert_eq!(pulls_opened(&f), 2);

    // Back at the covered head: the note is stale and goes.
    git(&wt, &["reset", "--quiet", "--hard", &by_hand])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        wait_state(&f, &agent, &conflicting).await?["push_problem"],
        Value::Null
    );

    // New work with uncommitted changes: not ready, so the task stays merged.
    commit_file(&wt, "more.txt", "more", "More work")?;
    let more = git(&wt, &["rev-parse", "HEAD"])?;
    std::fs::write(wt.join("scratch.txt"), "work in progress")?;
    let dirty = merge_call(more.clone()).await?;
    assert_eq!(dirty["isError"], true, "{dirty}");
    assert!(text(&dirty).contains("uncommitted changes"), "{dirty}");
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    std::fs::remove_file(wt.join("scratch.txt"))?;

    // A merged head this clone does not have (made on GitHub): how to fetch it.
    let merged_head =
        f.h.github
            .pulls
            .lock()
            .ok()
            .and_then(|p| p.iter().find(|p| p.number == 1).map(|p| p.head_sha.clone()))
            .unwrap_or_default();
    let absent = "1".repeat(40);
    edit_pull(&f, 1, |p| p.head_sha = absent.clone());
    let missing = merge_call(more.clone()).await?;
    assert!(
        text(&missing).contains("git fetch origin pull/1/head"),
        "{missing}"
    );
    edit_pull(&f, 1, |p| p.head_sha = merged_head.clone());

    // The origin cannot be fetched: unknown, never a pull request from a stale `dev`.
    let moved = f.origin.with_extension("moved");
    std::fs::rename(&f.origin, &moved)?;
    let unfetched = merge_call(more.clone()).await?;
    std::fs::rename(&moved, &f.origin)?;
    assert!(
        text(&unfetched).contains("could not be refreshed"),
        "{unfetched}"
    );
    assert_eq!(pulls_opened(&f), 2);

    // All clear: the follow-up opens as #3 and arms.
    f.h.github.set_checks(&more, ChecksState::Pending);
    let armed = arm_merge(&f, &agent, &more).await?;
    assert_eq!(armed["armed"], true, "{armed}");
    assert_eq!(pulls_opened(&f), 3, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, more, "pushed");
    Ok(())
}

/// SPEC-425 (review of #407, finding 4): a merged task that is a member of a live merge train
/// is not delivered anew while the train counts on its stage, once the train ends it is; a head
/// with nothing new reads "already merged" during the train too (round 4).
#[tokio::test]
async fn work_after_the_merge_waits_for_a_live_merge_train() -> TestResult {
    let (f, wt, merged_head) = merged_task().await?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;
    let trains =
        f.h.app
            .config
            .data_dir
            .join(yulius_app::merge_train::MERGE_TRAINS_FILE);
    std::fs::write(
        &trains,
        json!({"s1": {"train": {"run": "r1", "commit": merged_head, "base": merged_head,
            "base_tree": "t", "members": [{"task": "t1", "pr": 1, "head": merged_head,
            "armed_at": 1, "tree": "t"}]}}})
        .to_string(),
    )?;
    // Nothing new (a clean merge of `dev`): merged already, train or not.
    git(&wt, &["fetch", "--quiet", "origin"])?;
    git(&wt, &["merge", "--quiet", "--no-edit", "origin/dev"])?;
    let synced = git(&wt, &["rev-parse", "HEAD"])?;
    let done = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": synced, "review_summary": "approve"}),
    )
    .await?;
    assert!(
        done["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("already merged"),
        "{done}"
    );

    // New work waits for the train.
    commit_file(&wt, "fix.txt", "fixed", "Fix after the merge")?;
    let fix = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1, "{:?}", f.h.github.writes());
    let held = agent_tool(
        &f,
        &agent,
        "delivery_merge",
        json!({"reviewed_head": fix, "review_summary": "approve"}),
    )
    .await?;
    assert_eq!(held["isError"], true, "{held}");
    assert!(
        held["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("merge train that has not ended"),
        "{held}"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");

    std::fs::remove_file(&trains)?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 2, "{:?}", f.h.github.writes());
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, fix, "pushed");
    Ok(())
}

/// SPEC-425 (review of #407, round 5): without CI no train advances or ends, so a merge-trains
/// file left over from when it ran holds nothing: the follow-up opens.
#[tokio::test]
async fn a_leftover_train_holds_nothing_without_ci() -> TestResult {
    let (f, wt, merged_head) = merged_task_with(Box::new(|c| c.ci.enabled = false)).await?;
    std::fs::write(
        f.h.app
            .config
            .data_dir
            .join(yulius_app::merge_train::MERGE_TRAINS_FILE),
        json!({"s1": {"train": {"run": "r1", "commit": merged_head, "base": merged_head,
            "base_tree": "t", "members": [{"task": "t1", "pr": 1, "head": merged_head,
            "armed_at": 1, "tree": "t"}]}}})
        .to_string(),
    )?;
    commit_file(&wt, "fix.txt", "fixed", "Fix after the merge")?;
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_eq!(pulls_opened(&f), 2, "{:?}", f.h.github.writes());
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
    assert!(refusal(&blocked).contains("is in progress"), "{blocked}");

    // Resolved without a new commit; the next call fails reading the branch.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.git.fail_ahead_behind(Some("the object store is busy"));
    let failed = agent_tool(&f, &agent, "delivery_merge", merge.clone()).await?;
    let why = refusal(&failed);
    assert!(
        why.contains("the object store is busy") && !why.contains("in progress"),
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
        outcome.contains("dropped")
            && outcome.contains("checks")
            && outcome.contains("failed")
            && outcome.contains(yulius_domain::review::KEEP_APPROVAL),
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
        base_sha: String::new(),
        head: "dev".into(),
        head_sha: head.clone(),
        mergeable: None,
        behind: false,
        from_fork: true,
        merge_commit_sha: None,
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

/// SPEC-484 PLN-003: the merge call whose merge GitHub cannot take just now (its new title, a
/// 502) arms it for the next pass, and with a step of the plan still ahead says that the step
/// waits for the merge, as every armed answer does.
#[tokio::test]
async fn a_merge_left_for_the_next_pass_says_the_step_ahead_waits() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_, head) = open_after_the_spec(
        &f,
        "t1",
        "yulius doctor warns about a GitHub CLI older than 2.82.1",
        "feat/1-doctor-gh",
        "docs(spec): SPEC-088 doctor warns about a GitHub CLI older than 2.82.1",
        &["feat(doctor): warn about a GitHub CLI older than 2.82.1 (SPEC-088)"],
    )
    .await?;
    let agent = live_agent(&f, "t1", "tok-t1").await?;
    set_steps(&f, "t1", &["done", "pending"]).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    fail_retitles(
        &f,
        vec![AppError::Upstream(
            "GitHub: GitHub upstream error (HTTP 502)".into(),
        )],
    );
    let later = arm_merge(&f, &agent, &head).await?;
    assert_eq!(
        (&later["merged"], &later["armed"]),
        (&json!(false), &json!(true)),
        "{later}"
    );
    let next = later["next"].as_str().unwrap_or_default();
    assert!(
        next.contains("Yulius merges this exact head by itself on its next pass")
            && next.contains(
                "1 step(s) of your plan are still ahead (next: \"Step 1\"): they stay open. \
                 Start their work once `yulius wait` reports this merge"
            ),
        "{later}"
    );
    Ok(())
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

#[tokio::test]
async fn a_later_unavailable_overview_cannot_overwrite_earlier_checks_in_flight() -> TestResult {
    use std::sync::atomic::Ordering;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    let overview = || f.req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob);
    let checks = |page: &Value| {
        page["tasks"]
            .as_array()
            .and_then(|tasks| tasks.iter().find(|row| row["task_id"] == "t1"))
            .and_then(|row| row["pr"]["checks"].as_str())
            .map(str::to_owned)
    };
    f.h.github.set_checks(&head, ChecksState::Success);
    assert_eq!(checks(&overview().await?.body).as_deref(), Some("success"));
    f.h.github.set_checks(&head, ChecksState::Failure);
    let hold = Arc::new(tokio::sync::Notify::new());
    *f.git.overview_log_hold.lock().map_err(|_| "log hold")? = Some(Arc::clone(&hold));
    let mut earlier = Box::pin(overview());
    tokio::select! {
        _ = &mut earlier => return Err("earlier red read missed its Git log hold".into()),
        reached = tokio::time::timeout(Duration::from_secs(5), f.git.overview_log_entered.notified()) => reached?,
    }
    *f.git.overview_log_hold.lock().map_err(|_| "log hold")? = None;
    f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
    assert_eq!(checks(&overview().await?.body).as_deref(), Some("success"));
    hold.notify_one();
    assert_eq!(checks(&earlier.await?.body).as_deref(), Some("failure"));
    assert_eq!(checks(&overview().await?.body).as_deref(), Some("failure"));
    Ok(())
}

/// Cancelling HTTP during the final Git log must return even a non-front slot.
#[tokio::test]
async fn cancelling_an_overview_at_the_final_git_log_admits_a_replacement() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (_wt, head) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    let overview = || f.req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob);
    let held = f.h.github.hold_checks();
    let mut oldest = Box::pin(overview());
    tokio::select! {
        _ = &mut oldest => return Err("oldest request did not reach the held PR check".into()),
        reached = tokio::time::timeout(Duration::from_secs(5), f.h.github.checks_entered.notified()) => reached?,
    }
    // The first request owns a reference to the held check. Later requests finish.
    *f.h.github.checks_hold.lock().map_err(|_| "check hold")? = None;
    for _ in 1..29 {
        assert_eq!(overview().await?.status, StatusCode::OK);
    }
    let log_hold = Arc::new(tokio::sync::Notify::new());
    *f.git.overview_log_hold.lock().map_err(|_| "log hold")? = Some(log_hold);
    let mut last = Box::pin(overview());
    tokio::select! {
        _ = &mut last => return Err("last request did not reach its held Git log".into()),
        reached = tokio::time::timeout(Duration::from_secs(5), f.git.overview_log_entered.notified()) => reached?,
    }
    assert_eq!(overview().await?.status, StatusCode::TOO_MANY_REQUESTS);
    drop(last);
    *f.git.overview_log_hold.lock().map_err(|_| "log hold")? = None;
    let replacement = overview().await?;
    assert_eq!(
        replacement.status,
        StatusCode::OK,
        "the cancelled HTTP request kept its slot: {}",
        replacement.body
    );
    f.h.github.release_checks(&held);
    assert_eq!(oldest.await?.status, StatusCode::OK);
    Ok(())
}

/// SPEC-108 PRL-007: the read a merge's announcement brings may reach GitHub before its list of
/// open pull requests caught up; a pull request Yulius recorded as merged reads merged anyway.
#[tokio::test]
async fn a_merged_pull_request_github_still_lists_as_open_reads_merged() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.alice.bearer.as_deref().ok_or("owner token")?)
            .await?
            .ok_or("owner")?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &finished_spec("t1"), "Spec")?;
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
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    let task = f.h.store.task(&id).await?.ok_or("parked task")?;
    let summary = serde_json::to_value(f.h.app.task_detail(&owner, task.id.as_str()).await?)?;
    assert_eq!(summary["parked_needs_you"], false);
    assert_eq!(summary["parked_pr"], json!({"number":1,"open":false}));
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
    // The command of an environment that follows production is a production deploy (ADR-0110,
    // SPEC-483 P8): an administrator's, with the project's slug typed.
    let rollout = "/api/sandboxes/s1/delivery/environments/rollout/runs";
    let typed = json!({"confirm": true, "slug": "shop"});
    let writer = f
        .req(Method::POST, rollout, Some(typed.clone()), &f.bob)
        .await?;
    assert_eq!(writer.status, StatusCode::FORBIDDEN, "{}", writer.body);
    assert!(
        error_message(&writer).contains("administrator"),
        "{}",
        writer.body
    );
    for untyped in [
        json!({"confirm": true}),
        json!({"confirm": true, "slug": "other"}),
    ] {
        let refused = f
            .req(Method::POST, rollout, Some(untyped), &f.alice)
            .await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.body
        );
    }
    let started = f.req(Method::POST, rollout, Some(typed), &f.alice).await?;
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

/// SPEC-553 REP-001, REP-002, REP-004: reproduces and verifies the fix for the circular wait
/// (`quartio-v2` #322/#323/#324) where an open task-owned PR on a failing previous head waited for
/// CI on an unpublished clean committed repair while the final verification step remained open.
#[tokio::test]
async fn a_clean_committed_ci_repair_publishes_to_failing_open_pr_with_verification_open()
-> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Fix failing CI", "feat/322-ci-repair", "dev")
        .await?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    commit_file(&wt, "app.txt", "v1\n", "Initial change")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;

    // Open PR #1 on initial head `failing1` and fail CI there.
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1);
    let failing1 = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(f.origin_sha("feat/322-ci-repair")?, failing1);
    f.h.github.set_checks(&failing1, ChecksState::Failure);

    // Final verification step is open (`in_progress`) while waiting for CI on the committed repair.
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    commit_file(&wt, "app.txt", "v2 repair\n", "Commit tested repair")?;
    let repair1 = git(&wt, &["rev-parse", "HEAD"])?;

    // Turn-end auto delivery publishes the clean committed repair to the SAME open PR without
    // requiring the verification step to be falsely marked done, and never arms or merges.
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1, "same open PR updated, no new PR");
    assert_eq!(
        f.origin_sha("feat/322-ci-repair")?,
        repair1,
        "repair pushed to feature branch"
    );
    let view = f.delivery("t1").await?;
    assert_eq!(view["stage"], "pr_open", "{view}");
    assert_eq!(view["pr"]["number"], 1, "{view}");
    assert_eq!(view["pr"]["head"], "feat/322-ci-repair", "{view}");
    assert_eq!(
        view["pr"]["checks"], "none",
        "new published repair head has no failing checks yet: {view}"
    );
    let wait = wait_state(&f, &agent, &repair1).await?;
    assert_eq!(
        wait["merge_armed"], false,
        "publication never arms merge: {wait}"
    );
    assert_eq!(wait["push_problem"], Value::Null, "{wait}");

    // Second failing head + pending final verification step: `yulius wait` (and post-restart
    // reconnect) also publishes the eligible repair once with no duplicate push.
    f.h.github.set_checks(&repair1, ChecksState::Failure);
    set_steps(&f, "t1", &["done", "pending"]).await?;
    commit_file(&wt, "app.txt", "v3 repair\n", "Second CI repair")?;
    let repair2 = git(&wt, &["rev-parse", "HEAD"])?;
    let pushes_before = f.git.pushes.lock().map(|p| p.len()).unwrap_or(0);
    let wait2 = wait_state(&f, &agent, &repair2).await?;
    assert_eq!(wait2["push_problem"], Value::Null, "{wait2}");
    assert_eq!(
        f.origin_sha("feat/322-ci-repair")?,
        repair2,
        "yulius wait published eligible repair"
    );
    let pushes_after = f.git.pushes.lock().map(|p| p.len()).unwrap_or(0);
    assert_eq!(pushes_after, pushes_before + 1, "pushed once");

    // Repeated turn-end and wait calls for the same head do not push again.
    f.h.app.auto_open_pr(&t1).await;
    let _ = wait_state(&f, &agent, &repair2).await?;
    let pushes_repeat = f.git.pushes.lock().map(|p| p.len()).unwrap_or(0);
    assert_eq!(pushes_repeat, pushes_after, "no duplicate push");
    Ok(())
}

/// SPEC-553 REP-001, REP-003, REP-004: preserves safeguards for new PRs, non-failing/armed open
/// PRs, dirty worktrees, divergent branches, detached HEAD, human decisions (`needs_you` and
/// `Blocked` steps), fork/mismatched PRs, and surfaces failed publication in `yulius wait` and
/// `delivery_status`.
#[tokio::test]
async fn open_pr_ci_repair_preserves_safeguards_and_surfaces_actionable_failures() -> TestResult {
    use yulius_app::ports::store_ext::NeedsYouStore;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Guarded repair", "feat/323-guards", "dev")
        .await?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    commit_file(&wt, "app.txt", "v1\n", "Initial")?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;

    // 1. New PR with open step: still blocked at TurnEnd (no PR opened).
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 0, "new PR still waits for full plan");

    // Open PR #1 cleanly.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1);
    let pr_head = git(&wt, &["rev-parse", "HEAD"])?;

    // 2. Non-failing (Pending) open PR with open step: NOT pushed.
    f.h.github.set_checks(&pr_head, ChecksState::Pending);
    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    commit_file(&wt, "app.txt", "v2\n", "Next slice")?;
    let v2 = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "pending PR does not bypass plan_hold"
    );

    // 3. Failing open PR + dirty worktree: refused with actionable note in wait & delivery_status.
    f.h.github.set_checks(&pr_head, ChecksState::Failure);
    std::fs::write(wt.join("dirty.tmp"), "uncommitted\n")?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "dirty worktree must not push"
    );
    let dirty_wait = wait_state(&f, &agent, &v2).await?;
    assert!(
        dirty_wait["push_problem"]
            .as_str()
            .is_some_and(|s| s.contains("uncommitted changes")),
        "{dirty_wait}"
    );
    let dirty_view = f.delivery("t1").await?;
    assert!(
        dirty_view["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|n| n
                .as_str()
                .is_some_and(|s| s.contains("uncommitted changes")))),
        "{dirty_view}"
    );
    std::fs::remove_file(wt.join("dirty.tmp"))?;

    // 4. Failing open PR + pending human decision (`needs_you`): NOT pushed.
    let asked = yulius_app::needs_you::NeedsYouRecord {
        action: "Approve migration".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: 10,
        cause: None,
        named: Default::default(),
    };
    f.h.store
        .set_task_needs_you(&t1, Some(&asked), Timestamp(10))
        .await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "needs_you blocks auto repair publication"
    );
    f.h.store
        .set_task_needs_you(&t1, None, Timestamp(11))
        .await?;

    // 5. Failing open PR + Blocked step: NOT pushed.
    set_steps(&f, "t1", &["done", "blocked"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "blocked step prevents auto repair publication"
    );
    set_steps(&f, "t1", &["done", "in_progress"]).await?;

    // 6. Failing open PR + detached HEAD (even at the exact same SHA as `refs/heads/<branch>`,
    // and also with a detached commit): NOT pushed, actionable note set.
    git(&wt, &["checkout", "--quiet", "--detach", &v2])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "detached HEAD at same SHA must not push"
    );
    commit_file(&wt, "app.txt", "detached\n", "Detached commit")?;
    let detached = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "detached HEAD must not push"
    );
    let detached_wait = wait_state(&f, &agent, &detached).await?;
    assert!(
        detached_wait["push_problem"]
            .as_str()
            .is_some_and(|s| s.contains("not on `feat/323-guards`")),
        "{detached_wait}"
    );
    git(&wt, &["checkout", "--quiet", "feat/323-guards"])?;

    // 7. Failing open PR + diverged/amended branch: NOT force-pushed, actionable remedy recorded.
    git(&wt, &["reset", "--quiet", "--hard", &pr_head])?;
    std::fs::write(wt.join("app.txt"), "amended\n")?;
    git(&wt, &["commit", "--quiet", "--amend", "-am", "Amended"])?;
    let amended = git(&wt, &["rev-parse", "HEAD"])?;
    assert_ne!(amended, pr_head);
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/323-guards")?,
        pr_head,
        "diverged branch must never force-push"
    );
    let div_wait = wait_state(&f, &agent, &amended).await?;
    assert!(
        div_wait["push_problem"].as_str().is_some_and(
            |s| s.contains("Do not force-push") && s.contains("origin/feat/323-guards")
        ),
        "{div_wait}"
    );
    let div_view = f.delivery("t1").await?;
    assert!(
        div_view["notes"].as_array().is_some_and(|notes| notes
            .iter()
            .any(|n| n.as_str().is_some_and(|s| s.contains("Do not force-push")))),
        "{div_view}"
    );

    // 8. Push failure on an otherwise eligible clean fast-forward repair is actionable in both
    // `yulius wait` and `delivery_status`, then succeeds once the remote accepts pushes again.
    git(&wt, &["reset", "--quiet", "--hard", &pr_head])?;
    commit_file(&wt, "app.txt", "clean repair\n", "Clean repair")?;
    let clean = git(&wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.origin_sha("feat/323-guards")?, pr_head);
    let fail_wait = wait_state(&f, &agent, &clean).await?;
    assert!(
        fail_wait["push_problem"]
            .as_str()
            .is_some_and(|s| s.starts_with("Yulius could not push")),
        "{fail_wait}"
    );
    let fail_view = f.delivery("t1").await?;
    assert!(
        fail_view["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|n| n
                .as_str()
                .is_some_and(|s| s.starts_with("Yulius could not push")))),
        "{fail_view}"
    );

    // Clear push fault: publishes cleanly and clears the note.
    f.git.fail_pushes(None);
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.origin_sha("feat/323-guards")?, clean);
    assert_eq!(
        wait_state(&f, &agent, &clean).await?["push_problem"],
        Value::Null
    );
    Ok(())
}

/// SPEC-553 REP-002, REP-004: publishing an eligible open-PR CI repair immediately queues
/// exact-head Yulius CI; `watch_prs` recovers an unpublished repair for a live task without
/// emitting a stale failed-run notice across passes, and failed publications surface cleanly in
/// `yulius wait` and `delivery_status` (live PTY `pr_watch` delivery is tested in `pr_watch.rs`).
#[tokio::test]
async fn open_pr_ci_repair_queues_exact_head_ci_and_recovers_via_pr_watch_without_stale_notice()
-> TestResult {
    check_repair_publication(false).await
}

#[tokio::test]
async fn a_recorded_singleton_does_not_hide_an_unpublished_ordinary_ci_repair() -> TestResult {
    check_repair_publication(true).await
}

async fn check_repair_publication(recorded_singleton: bool) -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore;
    use yulius_app::pr_watch::WatchBounds;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::resources::Need;

    let f = fixture(&DeliveryProfile::standard()).await?;
    let sandbox = SandboxId::parse("s1")?;
    let record = f.h.store.sandbox(&sandbox).await?.ok_or("s1")?;
    let mut profile: Value = serde_json::from_str(&record.profile_json)?;
    profile["gate"] = json!("true");
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;

    let wt = f
        .add_task("t1", "CI repair sync", "feat/324-ci-sync", "dev")
        .await?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    commit_file(&wt, "app.txt", "v1\n", "Initial")?;
    let t1 = TaskId::parse("t1")?;
    let agent = live_agent(&f, "t1", "tok-agent").await?;

    // Open PR #1 cleanly.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(&f), 1);
    let pr_head = git(&wt, &["rev-parse", "HEAD"])?;

    // Record a failing CI run at `pr_head` (newer than any run queued when PR #1 opened), leave
    // final verification step open, and commit repair1.
    f.h.github.set_checks(&pr_head, ChecksState::Failure);
    let t_now = f.h.app.now().0;
    let failed_run = CiRunRecord {
        id: "run-failed-1".into(),
        sandbox_id: sandbox.clone(),
        reference: CiRef::PullRequest(1),
        sha: pr_head.clone(),
        source: GateSource::ProjectConfig,
        state: RunState::Failed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(t_now + 10),
        started_at: Some(Timestamp(t_now + 11)),
        ended_at: Some(Timestamp(t_now + 12)),
    };
    let failed_job = CiJobRecord {
        id: "job-failed-1".into(),
        run_id: "run-failed-1".into(),
        ordinal: 0,
        name: "checks".into(),
        key: "checks".into(),
        command: "true".into(),
        matrix: None,
        state: RunState::Failed,
        reason: None,
        exit_code: Some(1),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(t_now + 11)),
        ended_at: Some(Timestamp(t_now + 12)),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store.insert_ci_run(&failed_run, &[failed_job]).await?;

    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    commit_file(&wt, "app.txt", "v2 repair\n", "CI repair 1")?;
    let repair1 = git(&wt, &["rev-parse", "HEAD"])?;

    // 1. `auto_open_pr` publishes `repair1` AND immediately triggers `ci_sync_sandbox`, queueing
    // an exact-head `CiRunRecord` for `repair1`.
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.origin_sha("feat/324-ci-sync")?, repair1);
    let runs_after_push = f.h.store.ci_runs(&sandbox, 20).await?;
    assert!(
        runs_after_push
            .iter()
            .any(|r| r.reference == CiRef::PullRequest(1) && r.sha == repair1),
        "expected exact-head CI run queued for {repair1}: {runs_after_push:?}"
    );

    // 2. Mark `repair1` failed, remove the gate so `ci_sync_sandbox` queues no new run, commit
    // `repair2` cleanly (without calling `auto_open_pr`), and run `watch_prs`:
    // `watch_prs` must publish `repair2` AND must NOT emit a stale failed-run notice for `repair1`.
    profile["gate"] = Value::Null;
    f.h.store
        .set_profile(&sandbox, &profile.to_string())
        .await?;
    f.h.github.set_checks(&repair1, ChecksState::Failure);
    let failed_run2 = CiRunRecord {
        id: "run-failed-2".into(),
        sha: repair1.clone(),
        queued_at: Timestamp(t_now + 30_000),
        started_at: Some(Timestamp(t_now + 30_001)),
        ended_at: Some(Timestamp(t_now + 30_002)),
        ..failed_run.clone()
    };
    let failed_job2 = CiJobRecord {
        id: "job-failed-2".into(),
        run_id: "run-failed-2".into(),
        ordinal: 0,
        name: "checks".into(),
        key: "checks".into(),
        command: "true".into(),
        matrix: None,
        state: RunState::Failed,
        reason: None,
        exit_code: Some(1),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(t_now + 30_001)),
        ended_at: Some(Timestamp(t_now + 30_002)),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store
        .insert_ci_run(&failed_run2, &[failed_job2])
        .await?;
    if recorded_singleton {
        let detail = json!({
            "head": repair1,
            "message": "The candidate check failed.",
            "singleton": {
                "pr": 1,
                "base": f.origin_sha("dev")?,
                "tree": git(&wt, &["rev-parse", "HEAD^{tree}"])?,
                "run": "earlier-candidate",
            },
        });
        f.h.app
            .deps
            .store
            .insert_span(&yulius_app::stages::NewSpan {
                sandbox_id: sandbox.clone(),
                subject_kind: yulius_domain::stages::SubjectKind::Task,
                subject_id: t1.to_string(),
                key: "wait@1".into(),
                parent_key: None,
                kind: "wait".into(),
                name: "merge_armed".into(),
                environment: None,
                job: None,
                started_at: Timestamp(1),
                ended_at: Some(Timestamp(2)),
                outcome: Some("dropped".into()),
                detail: Some(detail.to_string()),
            })
            .await?;
    }
    commit_file(&wt, "app.txt", "v3 repair\n", "CI repair 2")?;
    let repair2 = git(&wt, &["rev-parse", "HEAD"])?;
    let bounds = WatchBounds::default();
    let typed = f.h.app.watch_prs(&bounds).await;
    assert_eq!(
        f.origin_sha("feat/324-ci-sync")?,
        repair2,
        "watch_prs must recover and publish eligible repair2"
    );
    assert!(
        !typed
            .iter()
            .any(|(task, text)| task == &t1 && text.contains("failed")),
        "watch_prs must not emit a stale failed-run notice after pushing repair2: {typed:?}"
    );
    // Second pass with no new CI run queued yet must also stay silent.
    let typed_pass2 = f.h.app.watch_prs(&bounds).await;
    assert!(
        !typed_pass2
            .iter()
            .any(|(task, text)| task == &t1 && text.contains("failed")),
        "watch_prs pass 2 must not emit a stale failed-run notice: {typed_pass2:?}"
    );

    // 3. Failed publication surfaces in `yulius wait` and `delivery_status` notes.
    f.h.github.set_checks(&repair2, ChecksState::Failure);
    let failed_run3 = CiRunRecord {
        id: "run-failed-3".into(),
        sha: repair2.clone(),
        queued_at: Timestamp(t_now + 40_000),
        started_at: Some(Timestamp(t_now + 40_001)),
        ended_at: Some(Timestamp(t_now + 40_002)),
        ..failed_run
    };
    let failed_job3 = CiJobRecord {
        id: "job-failed-3".into(),
        run_id: "run-failed-3".into(),
        ordinal: 0,
        name: "checks".into(),
        key: "checks".into(),
        command: "true".into(),
        matrix: None,
        state: RunState::Failed,
        reason: None,
        exit_code: Some(1),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(t_now + 40_001)),
        ended_at: Some(Timestamp(t_now + 40_002)),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store
        .insert_ci_run(&failed_run3, &[failed_job3])
        .await?;
    commit_file(&wt, "app.txt", "v4 repair\n", "CI repair 3")?;
    let repair3 = git(&wt, &["rev-parse", "HEAD"])?;
    f.git.fail_pushes(Some(PushFailure::Refused));
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.origin_sha("feat/324-ci-sync")?, repair2);
    let wait_fail = wait_state(&f, &agent, &repair3).await?;
    assert!(
        wait_fail["push_problem"]
            .as_str()
            .is_some_and(|s| s.starts_with("Yulius could not push")),
        "{wait_fail}"
    );
    // Actionable failed publication also surfaces in `delivery_status` notes.
    let fail_view = f.delivery("t1").await?;
    assert!(
        fail_view["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|n| n
                .as_str()
                .is_some_and(|s| s.starts_with("Yulius could not push")))),
        "failed publication must surface in delivery_status: {fail_view}"
    );
    Ok(())
}

/// A landed change has its own verification record even if dev moves or CI runs again.
#[tokio::test]
async fn post_merge_evidence_follows_the_landing_through_reruns_and_retention() -> TestResult {
    use yulius_app::ports::store_ext::CiStore;
    use yulius_domain::ci::RunState;

    // This repository accepts a task note for machine results. Its source work and changelog
    // ship in the first PR, with the post-merge check already in the plan.
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Publish the setup guide", "docs/1-setup", "dev")
        .await?;
    commit_file(
        &wt,
        "docs/setup.md",
        "# Setup guide",
        "docs: document setup",
    )?;
    commit_file(
        &wt,
        "docs/changes.md",
        "Added the setup guide.",
        "docs: record setup change",
    )?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let agent = live_agent(&f, "t1", "tok-evidence").await?;
    answer(
        &agent_tool(
            &f,
            &agent,
            "steps_set",
            json!({"steps":[
                {"title":"Publish setup and changelog","status":"done"},
                {"title":"Verify the published guide on dev","status":"pending"}
            ]}),
        )
        .await?,
    )?;
    f.h.github.set_checks(&head, ChecksState::Success);
    arm_merge(&f, &agent, &head).await?;
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    assert_eq!(f.delivery("t1").await?["steps_open"], 1);
    let landed = f.origin_sha("dev")?;
    let path = format!("/api/tasks/t1/delivery/evidence/{landed}");
    let view = f.delivery("t1").await?;
    assert_eq!(view["evidence"]["url"], path);
    let read = || f.req(Method::GET, &path, None, &f.rita);
    let missing = read().await?;
    assert_eq!(missing.status, StatusCode::OK);
    assert_eq!(missing.body["task_id"], "t1");
    assert_eq!(missing.body["pr_number"], 1);
    assert_eq!(missing.body["merged_sha"], landed);
    assert_eq!(missing.body["head_sha"], head);
    assert_eq!(missing.body["runs"], json!([]));

    let ancestor = git(&f.origin, &["rev-parse", "dev^"])?;
    yulius_ci_run(&f.h.store, "before", &ancestor, RunState::Passed, 1).await?;
    advance_base(&f, "dev", "docs/setup.md", "Changed after publication")?;
    let later = f.origin_sha("dev")?;
    assert_ne!(later, landed);
    yulius_ci_run(&f.h.store, "after", &later, RunState::Passed, 4).await?;
    assert_eq!(read().await?.body["runs"], json!([]));
    yulius_ci_run(&f.h.store, "first", &landed, RunState::Failed, 2).await?;
    use yulius_domain::ci::CiRef;
    insert_ci_attempt(
        &f.h.store,
        "pr-head",
        &head,
        CiRef::PullRequest(1),
        RunState::Passed,
        0,
        &[("security", RunState::Skipped)],
    )
    .await?;
    let provenance =
        "not run again: merge train #1 passed at aaaa000 (run candidate); this is step 1 of 1";
    assert!(
        f.h.store
            .transition_ci_run(
                "pr-head",
                &[RunState::Passed],
                RunState::Passed,
                &yulius_app::ci::CiRunUpdate {
                    reason: Some(provenance.into()),
                    ..Default::default()
                },
            )
            .await?
    );
    insert_ci_attempt(
        &f.h.store,
        "candidate",
        &landed,
        CiRef::Train(vec![1]),
        RunState::Passed,
        8,
        &[],
    )
    .await?;
    insert_ci_attempt(
        &f.h.store,
        "other-pr",
        &head,
        CiRef::PullRequest(2),
        RunState::Passed,
        7,
        &[],
    )
    .await?;
    insert_ci_attempt(
        &f.h.store,
        "rerun",
        &landed,
        CiRef::Branch("dev".into()),
        RunState::Passed,
        3,
        &[
            ("published guide", RunState::Passed),
            ("security", RunState::Skipped),
        ],
    )
    .await?;
    let mut other_project = f.h.store.ci_run("rerun").await?.ok_or("CI attempt")?;
    other_project.id = "parallel-sandbox".into();
    other_project.sandbox_id = SandboxId::parse("s2")?;
    f.h.store.insert_ci_run(&other_project, &[]).await?;
    let evidence = read().await?.body;
    assert_eq!(evidence["runs"][0]["run_id"], "rerun", "{evidence}");
    assert_eq!(evidence["runs"][0]["state"], "passed");
    assert_eq!(
        evidence["runs"][0]["checks"],
        json!([
            {"name":"published guide","state":"passed"},{"name":"security","state":"skipped"}
        ])
    );
    assert_eq!(evidence["runs"][1]["state"], "failed");
    assert_eq!(evidence["runs"].as_array().ok_or("runs")?.len(), 3);
    assert_eq!(evidence["runs"][2]["reference"], "pr/1");
    assert_eq!(evidence["runs"][2]["sha"], head);
    assert_eq!(evidence["runs"][2]["reason"], provenance);
    assert_eq!(evidence["runs"][2]["checks"][0]["state"], "skipped");
    f.h.store.prune_ci_runs(0, Timestamp(i64::MAX)).await?;
    assert!(f.h.store.ci_run("rerun").await?.is_none());
    assert_eq!(read().await?.body, evidence);
    let pinned = f
        .req(Method::GET, &format!("{path}?run_id=first"), None, &f.rita)
        .await?;
    assert_eq!(pinned.body["runs"], json!([evidence["runs"][1]]));
    for unknown in [
        format!("{path}?before=parallel-sandbox"),
        format!("{path}?run_id=parallel-sandbox"),
        format!("{path}?run_id=after"),
        format!("/api/tasks/t1/delivery/evidence/{later}"),
    ] {
        assert_eq!(
            f.req(Method::GET, &unknown, None, &f.rita).await?.status,
            StatusCode::NOT_FOUND
        );
    }
    assert_ne!(
        f.req(Method::GET, &path, None, &Creds::default())
            .await?
            .status,
        StatusCode::OK
    );

    let outsider = add_user(&f.h, "u-outsider", "outsider", Default::default()).await?;
    assert_eq!(
        f.req(Method::GET, &path, None, &outsider).await?.status,
        StatusCode::NOT_FOUND
    );
    let status = agent_tool(&f, &agent, "delivery_status", json!({})).await?;
    let text = status["content"][0]["text"].as_str().ok_or("status")?;
    assert!(text.contains(&path) && text.contains("rerun"), "{text}");
    let closed = agent_tool(
        &f,
        &agent,
        "step_update",
        json!({
            "ordinal": 2, "status": "done", "note": format!("Published guide passed. Security scan skipped. {path}?run_id=rerun")
        }),
    )
    .await?;
    assert_eq!(answer(&closed)?["progress"]["done"], 2);
    let steps = f.h.store.steps(&TaskId::parse("t1")?).await?;
    assert_eq!(steps[1].status, "done");
    assert!(steps[1].note.as_deref().is_some_and(|n| n.contains(&path)));
    f.h.app.auto_open_pr(&TaskId::parse("t1")?).await;
    assert_eq!(pulls_opened(&f), 1);
    assert_eq!(git(&wt, &["rev-parse", "HEAD"])?, head);
    assert!(git(&wt, &["status", "--porcelain"])?.is_empty());
    // A later failing scan is visible under its own ID. The note still cites the attempt
    // whose scan was skipped, so neither attempt becomes a passing security scan.
    insert_ci_attempt(
        &f.h.store,
        "scan-rerun",
        &landed,
        CiRef::Branch("dev".into()),
        RunState::Failed,
        9,
        &[("security", RunState::Failed)],
    )
    .await?;
    assert_eq!(
        read().await?.body["runs"][0]["checks"][0]["state"],
        "failed"
    );
    let cited = f
        .req(Method::GET, &format!("{path}?run_id=rerun"), None, &f.rita)
        .await?;
    assert_eq!(cited.body["runs"][0]["checks"][1]["state"], "skipped");
    Ok(())
}

#[tokio::test]
async fn post_merge_evidence_requires_the_verified_cd_commit_and_both_commands() -> TestResult {
    use yulius_app::ci::cd::CdRunRecord;
    use yulius_app::ports::store_ext::{CdStore, SpanStore};
    use yulius_domain::ci::RunState;
    use yulius_domain::rollback::CdRunKind;

    let (f, _, head) = merged_task().await?;
    let landed = f.origin_sha("dev")?;
    let path = format!("/api/tasks/t1/delivery/evidence/{landed}");
    let attempt = |id: &str, sha: &str, state, at| -> BoxResult<CdRunRecord> {
        Ok(CdRunRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: "staging".into(),
            stage: yulius_domain::cd::CdStage::Dev,
            branch: "dev".into(),
            sha: sha.into(),
            state,
            reason: None,
            requested_by: "person".into(),
            task_id: None,
            deploy: Some("deploy".into()),
            verify: Some("smoke".into()),
            note: None,
            output: String::new(),
            exit_code: Some(if state == RunState::Passed { 0 } else { 1 }),
            started_at: Timestamp(at),
            ended_at: Some(Timestamp(at + 1)),
            kind: CdRunKind::Deploy,
            source: None,
            steps: Vec::new(),
        })
    };
    let store = &f.h.store;
    let insert = |run| async move { store.insert_cd_run(&run).await };
    insert(attempt("previous", &head, RunState::Passed, 1)?).await?;
    insert(CdRunRecord {
        kind: CdRunKind::Drill,
        ..attempt("drill", &landed, RunState::Passed, 2)?
    })
    .await?;
    insert(CdRunRecord {
        verify: None,
        ..attempt("deploy-only", &landed, RunState::Passed, 3)?
    })
    .await?;
    insert(attempt("broken", &landed, RunState::Failed, 4)?).await?;
    // A nested command printed phase markers before the wrapper failed.
    for phase in ["deploy", "verify"] {
        assert!(
            store
                .insert_span(&yulius_app::stages::NewSpan {
                    sandbox_id: SandboxId::parse("s1")?,
                    subject_kind: yulius_domain::stages::SubjectKind::CdRun,
                    subject_id: "broken".into(),
                    key: format!("run/{phase}"),
                    parent_key: Some("run".into()),
                    kind: phase.into(),
                    name: phase.into(),
                    environment: Some("staging".into()),
                    job: None,
                    started_at: Timestamp(4),
                    ended_at: Some(Timestamp(5)),
                    outcome: Some("passed".into()),
                    detail: None,
                })
                .await?
        );
    }
    insert(attempt("smoke", &landed, RunState::Passed, 5)?).await?;
    advance_base(&f, "dev", "feature.txt", "Replacement behavior")?;
    insert(attempt(
        "different",
        &f.origin_sha("dev")?,
        RunState::Passed,
        6,
    )?)
    .await?;
    use yulius_app::delivery::{EnvironmentRunKind, EnvironmentRunRecord, EnvironmentRunState};
    let profile_run = EnvironmentRunRecord {
        id: "profile-command".into(),
        sandbox_id: SandboxId::parse("s1")?,
        environment: "dev-vm".into(),
        kind: EnvironmentRunKind::Command,
        requested_by: UserId::parse("u-bob")?,
        command: Some("smoke".into()),
        commit: Some(landed.clone()),
        state: EnvironmentRunState::Succeeded,
        output: String::new(),
        exit_code: Some(0),
        started_at: Timestamp(0),
        ended_at: Some(Timestamp(1)),
    };
    store.insert_environment_run(&profile_run).await?;
    store
        .insert_environment_run(&EnvironmentRunRecord {
            id: "person-confirmed".into(),
            kind: EnvironmentRunKind::Manual,
            command: None,
            commit: None,
            ..profile_run
        })
        .await?;
    let read = || f.req(Method::GET, &path, None, &f.rita);
    let response = read().await?;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let body = response.body;
    let runs = body["runs"].as_array().ok_or("CD attempts")?;
    assert_eq!(runs.len(), 4, "{body}");
    assert_eq!(runs[3]["kind"], "environment command");
    assert_eq!(runs[3]["reference"], "dev-vm");
    assert_eq!(
        runs[3]["checks"],
        json!([{"name":"command","state":"succeeded"}])
    );
    assert_eq!(runs[0]["reference"], "staging");
    assert_eq!(runs[0]["sha"], landed);
    assert_eq!(
        runs[0]["checks"],
        json!([{"name":"deploy","state":"passed"},{"name":"verify","state":"passed"}])
    );
    assert_eq!(runs[1]["state"], "failed");
    assert_eq!(runs[1]["checks"][0]["state"], "unconfirmed");
    assert_eq!(runs[1]["checks"][1]["state"], "unconfirmed");
    assert_eq!(runs[2]["checks"][1]["state"], "not configured");
    store.prune_spans(Timestamp(i64::MAX)).await?;
    assert_eq!(read().await?.body, body);
    f.h.store.prune_cd_runs(0, Timestamp(i64::MAX)).await?;
    assert!(f.h.store.cd_run("smoke", false).await?.is_none());
    assert_eq!(read().await?.body, body);
    Ok(())
}

#[tokio::test]
async fn post_merge_evidence_refuses_a_pr_retargeted_at_merge() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Update the guide", "docs/1-guide", "dev")
        .await?;
    commit_file(&wt, "guide.md", "guide", "docs: update guide")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let base = f.origin_sha("dev")?;
    git(&f.origin, &["branch", "another-base", &base])?;
    let agent = live_agent(&f, "t1", "tok-retarget").await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    *f.h.github
        .retarget_on_merge
        .lock()
        .map_err(|_| "retarget lock")? = Some("another-base".into());
    arm_merge(&f, &agent, &head).await?;
    f.h.app.merge_armed(&SandboxId::parse("s1")?).await;
    assert_eq!(f.origin_sha("dev")?, base);
    let other = f.origin_sha("another-base")?;
    assert_ne!(other, base);
    assert!(f.delivery("t1").await?["evidence"].is_null());
    assert_eq!(
        f.req(
            Method::GET,
            &format!("/api/tasks/t1/delivery/evidence/{other}"),
            None,
            &f.rita
        )
        .await?
        .status,
        StatusCode::NOT_FOUND
    );
    let mut profile = DeliveryProfile::standard();
    profile.integration_branch = "another-base".into();
    f.h.store
        .set_profile(
            &SandboxId::parse("s1")?,
            &serde_json::to_string(&profile_to_contract(&profile))?,
        )
        .await?;
    assert!(
        f.delivery("t1").await?["evidence"].is_null(),
        "changing the integration branch cannot reinterpret a rejected landing"
    );
    assert_eq!(
        f.req(
            Method::GET,
            &format!("/api/tasks/t1/delivery/evidence/{other}"),
            None,
            &f.rita
        )
        .await?
        .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn post_merge_evidence_pages_over_archived_attempts_while_another_run_lands() -> TestResult {
    use yulius_app::ports::store_ext::CiStore;
    use yulius_domain::ci::RunState;
    let (f, _, _) = merged_task().await?;
    let sha = f.origin_sha("dev")?;
    let agent = live_agent(&f, "t1", "tok-page-evidence").await?;
    let path = format!("/api/tasks/t1/delivery/evidence/{sha}");
    let store = &f.h.store;
    use yulius_app::ci::cd::CdRunRecord;
    use yulius_app::ports::store_ext::CdStore;
    let deployment = CdRunRecord {
        id: "staging-smoke".into(),
        sandbox_id: SandboxId::parse("s1")?,
        environment: "staging".into(),
        stage: yulius_domain::cd::CdStage::Dev,
        branch: "dev".into(),
        sha: sha.clone(),
        state: RunState::Passed,
        reason: None,
        requested_by: "person".into(),
        task_id: None,
        deploy: Some("deploy".into()),
        verify: Some("smoke".into()),
        note: None,
        output: String::new(),
        exit_code: Some(0),
        started_at: Timestamp(0),
        ended_at: Some(Timestamp(1)),
        kind: yulius_domain::rollback::CdRunKind::Deploy,
        source: None,
        steps: vec![],
    };
    store.insert_cd_run(&deployment).await?;
    store
        .insert_cd_run(&CdRunRecord {
            id: "other-release".into(),
            sha: f.origin_sha("main")?,
            started_at: Timestamp(53),
            ended_at: Some(Timestamp(54)),
            ..deployment
        })
        .await?;
    store.prune_cd_runs(0, Timestamp(i64::MAX)).await?;
    assert!(store.cd_run("staging-smoke", false).await?.is_none());
    for at in 1..51 {
        yulius_ci_run(store, &format!("attempt-{at}"), &sha, RunState::Passed, at).await?;
    }
    yulius_ci_run(
        store,
        "new-tip",
        &f.origin_sha("main")?,
        RunState::Passed,
        51,
    )
    .await?;
    store.prune_ci_runs(0, Timestamp(i64::MAX)).await?;
    assert!(store.ci_run("attempt-1").await?.is_none());
    let first = f.req(Method::GET, &path, None, &f.rita).await?.body;
    assert_eq!(first["runs"].as_array().ok_or("page")?.len(), 50);
    assert_eq!(first["runs"][0]["run_id"], "attempt-50");
    let cursor = first["next_before"].as_str().ok_or("missing next page")?;
    yulius_ci_run(store, "new-rerun", &sha, RunState::Failed, 52).await?;
    let last = f
        .req(
            Method::GET,
            &format!("{path}?before={cursor}"),
            None,
            &f.rita,
        )
        .await?
        .body;
    assert_eq!(last["runs"].as_array().ok_or("last page")?.len(), 1);
    assert_eq!(last["runs"][0]["run_id"], "staging-smoke");
    let tool_page = agent_tool(
        &f,
        &agent,
        "delivery_status",
        json!({"merged_sha": sha, "before": cursor}),
    )
    .await?;
    assert_eq!(answer(&tool_page)?["evidence"]["runs"], last["runs"]);
    let pin = agent_tool(
        &f,
        &agent,
        "delivery_status",
        json!({"run_id":"staging-smoke"}),
    )
    .await?;
    let selected = answer(&pin)?["evidence"].clone();
    assert_eq!(selected["runs"], last["runs"]);
    assert_eq!(selected["runs"][0]["checks"][1]["state"], "passed");
    assert!(
        pin["content"][0]["text"]
            .as_str()
            .is_some_and(|s| s.contains("?run_id=staging-smoke"))
    );
    let by_cursor = agent_tool(&f, &agent, "delivery_status", json!({"before":cursor})).await?;
    assert_eq!(answer(&by_cursor)?["evidence"]["runs"], last["runs"]);
    let wrong_landing = agent_tool(
        &f,
        &agent,
        "delivery_status",
        json!({"merged_sha":f.origin_sha("main")?}),
    )
    .await?;
    assert_eq!(wrong_landing["isError"], true);
    f.add_task("t2", "Another task", "docs/2-other", "dev")
        .await?;
    let other = live_agent(&f, "t2", "tok-other-evidence").await?;
    assert_eq!(
        agent_tool(
            &f,
            &other,
            "delivery_status",
            json!({"merged_sha":sha, "run_id":"staging-smoke"})
        )
        .await?["isError"],
        true
    );
    assert!(last["next_before"].is_null());
    Ok(())
}

#[tokio::test]
async fn post_merge_evidence_recovers_the_expected_landing_after_github_returns() -> TestResult {
    use std::sync::atomic::Ordering;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Update the guide", "docs/1-guide", "dev")
        .await?;
    commit_file(&wt, "guide.md", "guide", "docs: update guide")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let agent = live_agent(&f, "t1", "tok-deferred-merge").await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    f.h.github.fail_merged_reads.store(true, Ordering::SeqCst);
    let answer = arm_merge(&f, &agent, &head).await?;
    assert_eq!(answer["merged"], true, "immediate merge: {answer}");
    let sha = f.origin_sha("dev")?;
    let receipt = answer["next"].as_str().ok_or("immediate merge receipt")?;
    assert!(
        receipt.contains(&format!(
            "pull request #1 at {} from PR head {}",
            &sha[..9],
            &head[..9]
        )),
        "{receipt}"
    );
    assert!(f.delivery("t1").await?["evidence"].is_null());
    let mut profile = DeliveryProfile::standard();
    profile.integration_branch = "another-base".into();
    f.h.store
        .set_profile(
            &SandboxId::parse("s1")?,
            &serde_json::to_string(&profile_to_contract(&profile))?,
        )
        .await?;
    f.h.github.fail_merged_reads.store(false, Ordering::SeqCst);
    let view = f.delivery("t1").await?;
    assert_eq!(view["evidence"]["merged_sha"], sha);
    assert_eq!(view["evidence"]["head_sha"], head);
    assert_eq!(view["evidence"]["branch"], "dev");
    Ok(())
}

/// A project's integration branch can change between an external merge and Yulius's first
/// observation of the closed PR. Both task delivery and the Git page can reconcile it.
#[tokio::test]
async fn post_merge_evidence_reconciles_an_external_merge_against_its_original_base() -> TestResult
{
    for landed_on_dev in [false, true] {
        for read_git_page in [false, true] {
            let f = fixture(&DeliveryProfile::standard()).await?;
            let wt = f
                .add_task("t1", "Update the guide", "docs/1-guide", "dev")
                .await?;
            commit_file(&wt, "guide.md", "guide", "docs: update guide")?;
            let head = git(&wt, &["rev-parse", "HEAD"])?;
            git(&f.origin, &["branch", "another-base", "dev"])?;
            if read_git_page {
                git(&wt, &["push", "--quiet", "origin", "HEAD:docs/1-guide"])?;
                yulius_app::ports::github::GitHubPort::create_pull(
                    f.h.github.as_ref(),
                    &Secret("token-carol".into()),
                    "Quartio-Studio",
                    "shop",
                    &yulius_app::ports::github::NewPull {
                        title: "docs: update guide".into(),
                        head: "docs/1-guide".into(),
                        base: "dev".into(),
                        body: String::new(),
                    },
                )
                .await?;
                assert_eq!(
                    f.delivery("t1").await?["stage"],
                    "pr_open",
                    "adopt the open PR"
                );
            } else {
                let agent = live_agent(&f, "t1", "tok-external-base").await?;
                arm_merge(&f, &agent, &head).await?;
            }
            if !landed_on_dev {
                *f.h.github
                    .retarget_on_merge
                    .lock()
                    .map_err(|_| "retarget lock")? = Some("another-base".into());
            }
            f.h.github.set_checks(&head, ChecksState::Success);
            let sha = yulius_app::ports::github::GitHubPort::merge_pull(
                f.h.github.as_ref(),
                &Secret("token-carol".into()),
                "Quartio-Studio",
                "shop",
                1,
                "squash",
                &head,
            )
            .await?;
            let mut profile = DeliveryProfile::standard();
            profile.integration_branch = "another-base".into();
            f.h.store
                .set_profile(
                    &SandboxId::parse("s1")?,
                    &serde_json::to_string(&profile_to_contract(&profile))?,
                )
                .await?;
            if read_git_page {
                let page = f
                    .req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob)
                    .await?;
                assert_eq!(page.status, StatusCode::OK);
                // Read the permalink before a task delivery read gets a chance to recover it.
                let proof = f
                    .req(
                        Method::GET,
                        &format!("/api/tasks/t1/delivery/evidence/{sha}"),
                        None,
                        &f.rita,
                    )
                    .await?;
                assert_eq!(
                    proof.status,
                    if landed_on_dev {
                        StatusCode::OK
                    } else {
                        StatusCode::NOT_FOUND
                    }
                );
            }
            let view = f.delivery("t1").await?;
            let proof = f
                .req(
                    Method::GET,
                    &format!("/api/tasks/t1/delivery/evidence/{sha}"),
                    None,
                    &f.rita,
                )
                .await?;
            if landed_on_dev {
                assert_eq!(proof.status, StatusCode::OK);
                assert_eq!(view["evidence"]["branch"], "dev");
                assert_eq!(proof.body["merged_sha"], sha);
                assert!(
                    view["notes"]
                        .as_array()
                        .ok_or("delivery notes")?
                        .iter()
                        .any(|note| {
                            note.as_str()
                                .is_some_and(|text| text.contains("is merged into `dev`"))
                        }),
                    "the delivery note must report the observed PR base: {}",
                    view["notes"]
                );
            } else {
                assert_eq!(
                    proof.status,
                    StatusCode::NOT_FOUND,
                    "retargeted external merge"
                );
                assert!(view["evidence"].is_null());
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn post_merge_evidence_checks_the_pr_number_on_the_git_page() -> TestResult {
    use std::sync::atomic::Ordering;

    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Update the guide", "docs/1-guide", "dev")
        .await?;
    commit_file(&wt, "guide.md", "guide", "docs: update guide")?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    let agent = live_agent(&f, "t1", "tok-mismatched-pr").await?;
    arm_merge(&f, &agent, &head).await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    let sha = yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        1,
        "squash",
        &head,
    )
    .await?;
    f.h.github
        .misnumber_merged_reads
        .store(true, Ordering::SeqCst);
    let status = agent_tool(&f, &agent, "delivery_status", json!({})).await?;
    let status = answer(&status)?;
    assert_eq!(status["stage"], "pr_open", "{status}");
    assert!(
        status["pull_request"].is_null() && status["evidence"].is_null(),
        "{status}"
    );
    let notes = status["notes"].as_array().ok_or("status notes")?;
    assert!(
        notes.iter().any(|note| note.as_str().is_some_and(
            |text| text.contains("GitHub returned pull request #2 for the requested #1")
        )),
        "{status}"
    );
    assert!(
        !notes.iter().any(|note| note
            .as_str()
            .is_some_and(|text| text.contains("is merged into"))),
        "{status}"
    );
    let record =
        f.h.app
            .deps
            .store
            .delivery(&TaskId::parse("t1")?)
            .await?
            .ok_or("delivery")?;
    assert_eq!(record.pr_number, Some(1));
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.bob)
        .await?;
    assert_eq!(page.status, StatusCode::OK);
    let view = f.delivery("t1").await?;
    assert_eq!(view["stage"], "pr_open");
    assert!(view["evidence"].is_null());
    let path = format!("/api/tasks/t1/delivery/evidence/{sha}");
    assert_eq!(
        f.req(Method::GET, &path, None, &f.rita).await?.status,
        StatusCode::NOT_FOUND
    );
    f.h.github
        .misnumber_merged_reads
        .store(false, Ordering::SeqCst);
    assert_eq!(f.delivery("t1").await?["evidence"]["pr_number"], 1);
    Ok(())
}

#[tokio::test]
async fn reading_a_merged_parked_task_spends_no_pull_requests() -> TestResult {
    use std::sync::atomic::Ordering;
    let (f, wt, _) = merged_task_with(Box::new(|config| config.parked_pr_read_ms = 10_000)).await?;
    std::fs::write(wt.join(SPEC_PATH), finished_spec("t1"))?;
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.bob.bearer.as_deref().ok_or("token")?)
            .await?
            .ok_or("owner")?;
    let counts = || {
        (
            f.h.github.pull_list_count.swap(0, Ordering::SeqCst),
            f.h.github.pull_get_count.swap(0, Ordering::SeqCst),
        )
    };
    counts();
    let mut measured = Vec::new();
    for _ in 0..2 {
        let rows = f.h.app.list_tasks(&owner, "s1", false).await?;
        assert!(rows.iter().any(|r| r.id == "t1"));
        let list = counts();
        assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
        measured.push((list, counts()));
    }
    assert_eq!(measured, vec![((0, 0), (0, 0)); 2]);
    Ok(())
}

#[tokio::test]
async fn reading_a_closed_unmerged_parked_task_spends_no_pull_requests() -> TestResult {
    use std::sync::atomic::Ordering;
    let f = fixture(&DeliveryProfile::standard()).await?;
    let (wt, _) = delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    let id = TaskId::parse("t1")?;
    edit_pull(&f, 1, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    assert!(wt.is_dir());
    let owner =
        f.h.app
            .authenticate_bearer(f.bob.bearer.as_deref().ok_or("token")?)
            .await?
            .ok_or("owner")?;
    f.h.github.pull_list_count.store(0, Ordering::SeqCst);
    f.h.github.pull_get_count.store(0, Ordering::SeqCst);
    for _ in 0..2 {
        f.h.app.list_tasks(&owner, "s1", false).await?;
        assert_eq!(f.delivery("t1").await?["stage"], "none");
    }
    assert_eq!(f.h.github.pull_list_count.load(Ordering::SeqCst), 0);
    assert_eq!(f.h.github.pull_get_count.load(Ordering::SeqCst), 0);
    // A new external PR on the same branch is discovered by the repository tick.
    // Repeated display reads of the old closed number needed no discovery.
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let replacement = yulius_app::ports::github::GhPull {
            number: 2,
            state: "open".into(),
            ..pulls[0].clone()
        };
        pulls.push(replacement);
    }
    f.h.app.reconcile_deliveries().await;
    edit_pull(&f, 2, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    f.h.github.pull_list_count.store(0, Ordering::SeqCst);
    for _ in 0..2 {
        assert_eq!(f.delivery("t1").await?["stage"], "none");
    }
    assert_eq!(f.h.github.pull_list_count.load(Ordering::SeqCst), 0);
    // Restart before any delivery read can adopt the older reopened number.
    edit_pull(&f, 1, |p| p.state = "open".into());
    f.h.app.reconcile_deliveries().await;
    let cold = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    cold.reconcile_deliveries().await;
    let tracked = f.h.store.delivery(&id).await?.ok_or("reopened delivery")?;
    assert_eq!(tracked.pr_number, Some(1));
    assert_eq!(
        cold.task_delivery(&owner, "t1")
            .await?
            .pr
            .ok_or("open PR")?
            .number,
        1
    );
    edit_pull(&f, 1, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    edit_pull(&f, 2, |p| p.state = "open".into());
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t1").await?["pr"]["number"], 2);
    Ok(())
}

#[tokio::test]
async fn a_background_observation_recovers_a_reopened_pr_that_later_merges() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    edit_pull(&f, 1, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t1").await?["stage"], "none");
    edit_pull(&f, 1, |p| p.state = "open".into());
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    // If the Git page sees the next reopening first, direct confirmation retires the
    // stored closure before the next reconciliation tick.
    edit_pull(&f, 1, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    edit_pull(&f, 1, |p| p.state = "open".into());
    let owner =
        f.h.app
            .authenticate_bearer(f.bob.bearer.as_deref().ok_or("token")?)
            .await?
            .ok_or("owner")?;
    f.h.app.git_overview(&owner, "s1").await?;
    // The reopened PR merges before anyone opens delivery. A fresh process can reconcile it.
    let cold = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    edit_pull(&f, 1, |p| {
        p.state = "closed".into();
        p.merged = true;
    });
    cold.reconcile_deliveries().await;
    let view = f.delivery("t1").await?;
    assert_eq!(view["stage"], "merged_integration");
    assert_eq!(view["pr"]["state"], "merged");
    Ok(())
}

#[tokio::test]
async fn reconciliation_enriches_a_terminal_pull_missing_its_merge_commit() -> TestResult {
    use std::sync::atomic::Ordering;
    let f = fixture(&DeliveryProfile::standard()).await?;
    delivered_with_a_successor(&f, &finished_spec("t1")).await?;
    edit_pull(&f, 1, |p| {
        p.state = "closed".into();
        p.merged = true;
        p.merge_commit_sha = None;
    });
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");
    assert!(f.delivery("t1").await?["evidence"].is_null());
    // A later follow-up can open and close before the primary's merge SHA becomes available.
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let followup = yulius_app::ports::github::GhPull {
            number: 2,
            state: "open".into(),
            merged: false,
            ..pulls[0].clone()
        };
        pulls.push(followup);
    }
    f.h.app.reconcile_deliveries().await;
    edit_pull(&f, 2, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    let landed = f.origin_sha("dev")?;
    edit_pull(&f, 1, |p| p.merge_commit_sha = Some(landed.clone()));
    f.h.app.reconcile_deliveries().await;
    f.h.github.pull_get_count.store(0, Ordering::SeqCst);
    assert_eq!(f.delivery("t1").await?["evidence"]["merged_sha"], landed);
    assert_eq!(f.h.github.pull_get_count.load(Ordering::SeqCst), 0);
    Ok(())
}

async fn parked_primary_with_external_followup()
-> BoxResult<(Fixture, TaskId, yulius_app::auth::AuthContext, String)> {
    let (f, wt, head) =
        merged_task_with(Box::new(|config| config.parked_pr_read_ms = 10_000)).await?;
    std::fs::write(wt.join(SPEC_PATH), finished_spec("t1"))?;
    let id = TaskId::parse("t1")?;
    f.h.store.set_spec_status(&id, Some("done")).await?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    let owner =
        f.h.app
            .authenticate_bearer(f.bob.bearer.as_deref().ok_or("token")?)
            .await?
            .ok_or("owner")?;
    assert!(f.h.store.terminal_pull(&id, Some(1)).await?.is_some());
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let followup = yulius_app::ports::github::GhPull {
            number: 2,
            state: "open".into(),
            merged: false,
            mergeable: Some(true),
            behind: false,
            merge_commit_sha: None,
            ..pulls[0].clone()
        };
        pulls.push(followup);
    }
    Ok((f, id, owner, head))
}

#[tokio::test]
async fn a_reopened_followup_survives_restart_without_replacing_the_merged_delivery() -> TestResult
{
    use std::sync::atomic::Ordering;
    let (f, id, owner, head) = parked_primary_with_external_followup().await?;
    f.h.app.reconcile_deliveries().await;
    edit_pull(&f, 2, |p| p.state = "closed".into());
    f.h.app.reconcile_deliveries().await;
    assert!(f.h.store.terminal_pull(&id, Some(2)).await?.is_some());
    edit_pull(&f, 2, |p| p.state = "open".into());
    f.h.github.set_checks(&head, ChecksState::Failure);
    f.h.app.reconcile_deliveries().await;
    // The numbered endpoint still serves #2 while GitHub's listing omits it.
    *f.h.github.omit_listed_pull.lock().map_err(|_| "PR list")? = Some(2);
    let before = f.h.github.pull_get_count.load(Ordering::SeqCst);
    let cold = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    let rows = cold.list_tasks(&owner, "s1", false).await?;
    let task = rows
        .iter()
        .find(|row| row.id == "t1")
        .ok_or("parked task")?;
    let identity = task.parked_pr.as_ref().ok_or("follow-up")?;
    assert_eq!((identity.number, identity.open), (2, true));
    assert_eq!(task.parked_needs_you, Some(true));
    assert!(
        f.h.github.pull_get_count.load(Ordering::SeqCst) > before,
        "cold display verifies the omitted open PR by number"
    );
    let primary = f.h.store.delivery(&id).await?.ok_or("merged delivery")?;
    assert_eq!(primary.stage.encode(), "merged_integration");
    assert_eq!(primary.pr_number, Some(1));
    edit_pull(&f, 2, |p| p.state = "closed".into());
    cold.reconcile_deliveries().await;
    assert!(f.h.store.terminal_pull(&id, Some(2)).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn after_all_tasks_includes_a_new_failing_followup_before_discovery() -> TestResult {
    let (f, task, owner, head) = parked_primary_with_external_followup().await?;
    support::exhausted_subscription(&f.h).await?;
    let predecessor = f.h.store.task(&task).await?.ok_or("predecessor")?;
    let caller = crate::task_message::superagent(&predecessor.sandbox_id, &owner.user.id, "bob");
    f.h.github.set_checks(&head, ChecksState::Failure);
    let created =
        f.h.app
            .tool_task_create(
                &caller,
                &json!({
                    "prompt":"Wait for the follow-up", "harness":"claude", "after_all_tasks":true
                }),
            )
            .await?;
    assert_eq!(
        created["task"]["start_rule"],
        format!("#{} done", predecessor.number)
    );
    let successor = TaskId::parse(created["task"]["task_id"].as_str().ok_or("successor id")?)?;
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.h.store.open_followup(&task).await?, Some(2));
    *f.h.github.omit_listed_pull.lock().map_err(|_| "PR list")? = Some(2);
    f.h.app.advance_chains(&task).await;
    assert_eq!(
        f.h.store.task(&successor).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    f.h.github.set_checks(&head, ChecksState::Success);
    edit_pull(&f, 2, |pr| pr.behind = true);
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let newer = GhPull {
            number: 3,
            base: "release".into(),
            behind: false,
            ..pulls[1].clone()
        };
        pulls.push(newer);
    }
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.h.store.open_followup(&task).await?, Some(3));
    let status = f.h.app.task_detail(&owner, task.as_str()).await?.summary;
    assert_eq!(status.parked_pr.as_ref().map(|pr| pr.number), Some(2));
    assert_eq!(status.parked_needs_you, Some(true));
    f.h.app.advance_chains(&task).await;
    assert_eq!(
        f.h.store.task(&successor).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    edit_pull(&f, 2, |pr| pr.behind = false);
    let mut extra = f.h.github.pulls.lock().map_err(|_| "pull fixture")?[2].clone();
    let extra_numbers = 4..=u64::try_from(yulius_app::delivery::MAX_PULLS)? + 2;
    for number in extra_numbers.clone() {
        extra.number = number;
        f.h.store.remember_pull_observation(&task, &extra).await?;
    }
    let direct_before =
        f.h.github
            .pull_get_count
            .load(std::sync::atomic::Ordering::SeqCst);
    f.h.app.advance_chains(&task).await;
    assert_eq!(
        f.h.store.task(&successor).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    assert_eq!(
        f.h.github
            .pull_get_count
            .load(std::sync::atomic::Ordering::SeqCst),
        direct_before
    );
    extra.state = "closed".into();
    for number in extra_numbers {
        extra.number = number;
        f.h.store.remember_pull_observation(&task, &extra).await?;
    }

    f.h.app.advance_chains(&task).await;
    assert_ne!(
        f.h.store.task(&successor).await?.ok_or("successor")?.status,
        TaskStatus::Chained
    );
    Ok(())
}

#[tokio::test]
async fn a_vanished_memory_hint_keeps_the_independently_confirmed_open_pull() -> TestResult {
    let (f, task, owner, head) = parked_primary_with_external_followup().await?;
    f.h.github.set_checks(&head, ChecksState::Success);
    f.h.app.reconcile_deliveries().await;
    assert_eq!(f.h.store.open_followup(&task).await?, Some(2));
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let newer = GhPull {
            number: 3,
            behind: true,
            ..pulls[1].clone()
        };
        pulls.push(newer);
    }
    let latest = f.h.app.task_detail(&owner, task.as_str()).await?.summary;
    assert_eq!(latest.parked_pr.as_ref().map(|pr| pr.number), Some(3));
    assert_eq!(f.h.store.open_followup(&task).await?, Some(2));
    edit_pull(&f, 3, |pr| pr.state = "closed".into());
    *f.h.github.omit_listed_pull.lock().map_err(|_| "PR list")? = Some(2);
    f.h.github.set_checks(&head, ChecksState::Failure);
    let current = f.h.app.task_detail(&owner, task.as_str()).await?.summary;
    assert_eq!(current.parked_pr.as_ref().map(|pr| pr.number), Some(2));
    assert_eq!(current.parked_needs_you, Some(true));
    Ok(())
}
