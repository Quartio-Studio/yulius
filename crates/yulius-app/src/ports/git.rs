//! Git port (SPEC-002 GIT-001/002). Implemented over `yulius-git`; callers never pass user text
//! as options.

use std::path::Path;

use async_trait::async_trait;

use crate::error::AppResult;
use crate::model::Secret;

/// Diff statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStat {
    /// Files changed.
    pub files: u32,
    /// Inserted lines.
    pub insertions: u32,
    /// Deleted lines.
    pub deletions: u32,
}

/// A commit summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    /// SHA.
    pub sha: String,
    /// Subject.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Commit time (ms).
    pub at_ms: i64,
}

/// Git operations on sandbox repositories.
#[async_trait]
pub trait GitPort: Send + Sync {
    /// Whether `name` is a safe branch name.
    fn valid_branch(&self, name: &str) -> bool;
    /// Clones `url` into `dest` (or fetches when it already exists) using `token` for HTTPS.
    async fn sync_clone(&self, url: &str, dest: &Path, token: &Secret) -> AppResult<()>;
    /// Default branch of a clone.
    async fn default_branch(&self, repo: &Path) -> AppResult<String>;
    /// Whether a branch exists locally or on `origin`.
    async fn branch_exists(&self, repo: &Path, branch: &str) -> AppResult<bool>;
    /// Adds a worktree at `path` on `branch`, creating it from `base` when missing.
    async fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> AppResult<()>;
    /// Removes a worktree only when it is clean, registered and inside `owned_root`.
    async fn remove_worktree(&self, repo: &Path, path: &Path, owned_root: &Path)
    -> AppResult<bool>;
    /// Whether a worktree has tracked or untracked (non-ignored) changes.
    async fn has_changes(&self, worktree: &Path) -> AppResult<bool>;
    /// Installs the pre-push guard rejecting pushes to `protected` branches.
    async fn install_push_guard(&self, worktree: &Path, protected: &[String]) -> AppResult<()>;
    /// Sets the commit identity of a worktree.
    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()>;
    /// Diff statistics between two revisions.
    async fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> AppResult<DiffStat>;
    /// Commits ahead/behind between two revisions.
    async fn ahead_behind(&self, repo: &Path, base: &str, head: &str) -> AppResult<(u32, u32)>;
    /// Recent commits of a revision.
    async fn log(&self, repo: &Path, rev: &str, limit: u32) -> AppResult<Vec<CommitInfo>>;
    /// Commits all changes in a worktree (returns false when nothing changed).
    async fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        name: &str,
        email: &str,
    ) -> AppResult<bool>;
    /// Pushes `local_branch` to `remote_branch` on origin using `token`.
    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()>;
    /// Imports a bundle's refs under `refs/heads/<prefix>/…`.
    async fn import_bundle(
        &self,
        repo: &Path,
        bundle: &Path,
        prefix: &str,
    ) -> AppResult<Vec<String>>;
}
