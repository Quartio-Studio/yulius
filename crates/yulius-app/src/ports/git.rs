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

/// A task worktree's changes against its merge base, files in the API's form (UI-031).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewChanges {
    /// The merge base.
    pub base_sha: String,
    /// `HEAD`.
    pub head_sha: String,
    /// Changed files.
    pub files: Vec<yulius_contracts::api::git::ChangedFile>,
    /// Files with uncommitted changes.
    pub uncommitted: u32,
    /// Hunks or lines were left out.
    pub truncated: bool,
}

/// A branch tip (CI and hygiene, SPEC-001 DLV-006/007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Branch name.
    pub name: String,
    /// Tip commit.
    pub sha: String,
    /// Committer time of the tip (ms).
    pub committed_ms: i64,
}

/// A registered worktree of a sandbox clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// Path as Git records it.
    pub path: std::path::PathBuf,
    /// Checked-out branch, absent when detached.
    pub branch: Option<String>,
    /// Its directory is gone.
    pub prunable: bool,
    /// The main worktree (the clone itself).
    pub main: bool,
}

fn unsupported<T>(what: &str) -> AppResult<T> {
    Err(crate::error::AppError::Conflict(format!(
        "{what} is not available on this instance"
    )))
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
    /// Discards a task worktree on explicit request (task deletion): removed even when dirty,
    /// and the local `branch` deleted; only paths inside `owned_root`.
    async fn discard_worktree(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        branch: Option<&str>,
    ) -> AppResult<()> {
        let _ = branch;
        self.remove_worktree(repo, path, owned_root)
            .await
            .map(|_| ())
    }
    /// Whether a worktree has tracked or untracked (non-ignored) changes.
    async fn has_changes(&self, worktree: &Path) -> AppResult<bool>;
    /// Installs the pre-push guard rejecting pushes to `protected` branches.
    async fn install_push_guard(&self, worktree: &Path, protected: &[String]) -> AppResult<()>;
    /// Sets the commit identity of a worktree.
    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()>;
    /// Diff statistics between two revisions.
    async fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> AppResult<DiffStat>;
    /// The worktree's changes against its merge base with `base` (committed, uncommitted and
    /// untracked work), bounded, for the task review (UI-031).
    async fn review_changes(&self, worktree: &Path, base: &str) -> AppResult<ReviewChanges>;
    /// Commits on `head` not on `base`, newest first (at most `limit`), with their size.
    async fn review_commits(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
        limit: u32,
    ) -> AppResult<Vec<yulius_contracts::api::git::TaskCommit>>;
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
    /// Keeps a task worktree's work and reverts it (SPEC-002 ADR-16, "redirect"): uncommitted
    /// work is committed as `name <email>`, the new branch `keep` points at the result (an
    /// existing branch is refused), and the worktree is reset to its merge base with
    /// `origin/<base>` (or `<base>`). Returns the kept commit. Adapters without it refuse.
    async fn keep_and_revert(
        &self,
        worktree: &Path,
        keep: &str,
        base: &str,
        name: &str,
        email: &str,
    ) -> AppResult<String> {
        let _ = (worktree, keep, base, name, email);
        Err(crate::error::AppError::Conflict(
            "reverting a task's changes is not available on this instance".into(),
        ))
    }
    /// Pushes `local_branch` to `remote_branch` on origin using `token`.
    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()>;
    /// The commit `rev` names, if it exists in `repo`.
    async fn resolve_commit(&self, repo: &Path, rev: &str) -> AppResult<Option<String>> {
        let _ = (repo, rev);
        unsupported("resolving commits")
    }
    /// A file (repository-relative `path`) at `rev`; `None` when absent.
    async fn show_file(&self, repo: &Path, rev: &str, path: &str) -> AppResult<Option<String>> {
        let _ = (repo, rev, path);
        unsupported("reading files at a revision")
    }
    /// File names directly in directory `dir` at `rev` (empty when absent).
    async fn list_dir(&self, repo: &Path, rev: &str, dir: &str) -> AppResult<Vec<String>> {
        let _ = (repo, rev, dir);
        unsupported("listing files at a revision")
    }
    /// Prepares a persistent detached CI checkout at `path` (inside `owned_root`) on `sha`,
    /// keeping ignored files as a cache (SPEC-002 ADR-21).
    async fn ci_checkout(
        &self,
        repo: &Path,
        path: &Path,
        sha: &str,
        owned_root: &Path,
    ) -> AppResult<()> {
        let _ = (repo, path, sha, owned_root);
        unsupported("CI checkouts")
    }
    /// `origin`'s branches as last fetched.
    async fn remote_branches(&self, repo: &Path) -> AppResult<Vec<BranchInfo>> {
        let _ = repo;
        unsupported("listing branches")
    }
    /// Local branches.
    async fn local_branches(&self, repo: &Path) -> AppResult<Vec<BranchInfo>> {
        let _ = repo;
        unsupported("listing branches")
    }
    /// Whether `ancestor` is reachable from `descendant`.
    async fn is_ancestor(&self, repo: &Path, ancestor: &str, descendant: &str) -> AppResult<bool> {
        let _ = (repo, ancestor, descendant);
        unsupported("comparing commits")
    }
    /// Whether a remote-tracking branch contains `sha`.
    async fn on_remote(&self, repo: &Path, sha: &str) -> AppResult<bool> {
        let _ = (repo, sha);
        unsupported("comparing commits")
    }
    /// Registered worktrees (the main one first).
    async fn worktrees(&self, repo: &Path) -> AppResult<Vec<WorktreeInfo>> {
        let _ = repo;
        unsupported("listing worktrees")
    }
    /// Deletes a local branch that no worktree has checked out.
    async fn delete_local_branch(&self, repo: &Path, name: &str) -> AppResult<()> {
        let _ = (repo, name);
        unsupported("deleting branches")
    }
    /// Forgets worktrees whose directories are gone.
    async fn prune_worktrees(&self, repo: &Path) -> AppResult<()> {
        let _ = repo;
        unsupported("pruning worktrees")
    }
    /// Imports a bundle's refs under `refs/heads/<prefix>/…`.
    async fn import_bundle(
        &self,
        repo: &Path,
        bundle: &Path,
        prefix: &str,
    ) -> AppResult<Vec<String>>;
}
