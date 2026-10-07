//! Adapters reported as unavailable until their crates are wired (ptyhost: CP2, git: CP4).
//! Every call fails with a clear error; nothing pretends to succeed.

use std::path::Path;

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort};
use yulius_app::ports::session::{AttachedRun, LaunchPlan, RunInspection, SessionHost, SpawnedRun};

fn unavailable<T>(what: &str) -> AppResult<T> {
    Err(AppError::Upstream(format!(
        "{what} is not available in this build"
    )))
}

/// Process host placeholder.
pub struct UnavailableSessionHost;

#[async_trait]
impl SessionHost for UnavailableSessionHost {
    async fn spawn(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        unavailable("the process host")
    }
    async fn attach(&self, _: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        unavailable("the process host")
    }
    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }
    fn terminate(&self, _: &Path) -> AppResult<()> {
        unavailable("the process host")
    }
}

/// Git placeholder.
pub struct UnavailableGit;

#[async_trait]
impl GitPort for UnavailableGit {
    fn valid_branch(&self, _: &str) -> bool {
        false
    }
    async fn sync_clone(&self, _: &str, _: &Path, _: &Secret) -> AppResult<()> {
        unavailable("git")
    }
    async fn default_branch(&self, _: &Path) -> AppResult<String> {
        unavailable("git")
    }
    async fn branch_exists(&self, _: &Path, _: &str) -> AppResult<bool> {
        unavailable("git")
    }
    async fn add_worktree(&self, _: &Path, _: &Path, _: &str, _: &str) -> AppResult<()> {
        unavailable("git")
    }
    async fn remove_worktree(&self, _: &Path, _: &Path, _: &Path) -> AppResult<bool> {
        unavailable("git")
    }
    async fn has_changes(&self, _: &Path) -> AppResult<bool> {
        unavailable("git")
    }
    async fn install_push_guard(&self, _: &Path, _: &[String]) -> AppResult<()> {
        unavailable("git")
    }
    async fn set_identity(&self, _: &Path, _: &str, _: &str) -> AppResult<()> {
        unavailable("git")
    }
    async fn diff_stat(&self, _: &Path, _: &str, _: &str) -> AppResult<DiffStat> {
        unavailable("git")
    }
    async fn ahead_behind(&self, _: &Path, _: &str, _: &str) -> AppResult<(u32, u32)> {
        unavailable("git")
    }
    async fn log(&self, _: &Path, _: &str, _: u32) -> AppResult<Vec<CommitInfo>> {
        unavailable("git")
    }
    async fn commit_all(&self, _: &Path, _: &str, _: &str, _: &str) -> AppResult<bool> {
        unavailable("git")
    }
    async fn push(&self, _: &Path, _: &str, _: &str, _: &Secret) -> AppResult<()> {
        unavailable("git")
    }
    async fn import_bundle(&self, _: &Path, _: &Path, _: &str) -> AppResult<Vec<String>> {
        unavailable("git")
    }
}
