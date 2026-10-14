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
    /// Whether `relative` inside `worktree` is ignored by Git and holds no tracked file, so
    /// removing it loses nothing a commit has. Adapters without Git answer no.
    async fn is_disposable(&self, _worktree: &Path, _relative: &str) -> AppResult<bool> {
        Ok(false)
    }
    /// Whether the worktree holds files Git ignores. Adapters without Git answer yes.
    async fn has_ignored_files(&self, _worktree: &Path) -> AppResult<bool> {
        Ok(true)
    }
    /// The worktree's ignored files (directories with a trailing `/`); `None` when the adapter
    /// cannot tell (then nothing is removed).
    async fn ignored_entries(&self, _worktree: &Path) -> AppResult<Option<Vec<String>>> {
        Ok(None)
    }
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
    /// Fetches origin with `token`, then merges `origin/<integration>` into the worktree's branch
    /// with its own identity and the repository's merge drivers; a conflict is aborted (DLV-203).
    async fn merge_integration(
        &self,
        worktree: &Path,
        integration: &str,
        token: &Secret,
    ) -> AppResult<MergeIntegration> {
        let _ = (worktree, integration, token);
        unsupported("merging the integration branch")
    }
    /// Builds a merge train (SPEC-084 TRN-002) in the scratch worktree `scratch` inside
    /// `owned_root`: from `base`, each of `heads` in order is brought up to date as
    /// [`GitPort::merge_integration`] would (merge drivers apply) and lands as `landing` makes
    /// it land; a head that conflicts is left out. Nothing is pushed. Adapters without it
    /// refuse, and Yulius merges one by one.
    async fn build_train(
        &self,
        repo: &Path,
        scratch: &Path,
        owned_root: &Path,
        base: &str,
        heads: &[String],
        landing: TrainLanding,
    ) -> AppResult<TrainBuild> {
        let _ = (repo, scratch, owned_root, base, heads, landing);
        unsupported("merge trains")
    }
    /// Whether `head` is `reviewed` plus only Git's own merges of commits `integration` (a
    /// remote-tracking ref) contains, at most `limit` of them: no conflict resolved by hand, no
    /// other change (SPEC-083 REV-001).
    async fn only_clean_merges_since(
        &self,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: &str,
        limit: usize,
    ) -> AppResult<bool> {
        let _ = (worktree, reviewed, head, integration, limit);
        unsupported("comparing a head with its review")
    }
    /// Keeps Yulius's block of merge attributes in `repo`'s `info/attributes`, read by all its
    /// worktrees, at exactly `lines` (SPEC-083 MRG-001). Returns whether the file changed.
    async fn set_merge_attributes(&self, repo: &Path, lines: &[String]) -> AppResult<bool> {
        let _ = (repo, lines);
        unsupported("setting merge attributes")
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
    /// The tree commit `rev` records; `None` when unknown or the adapter cannot tell.
    async fn tree_of(&self, repo: &Path, rev: &str) -> AppResult<Option<String>> {
        let _ = (repo, rev);
        Ok(None)
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
    /// The best common ancestor of two revisions, if any.
    async fn merge_base(&self, repo: &Path, a: &str, b: &str) -> AppResult<Option<String>> {
        let _ = (repo, a, b);
        unsupported("finding a merge base")
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
    /// Creates an empty bare repository at the absolute `path` whose first branch is `branch`:
    /// a playground's `origin` (SPEC-099).
    async fn init_bare(&self, path: &Path, branch: &str) -> AppResult<()> {
        let _ = (path, branch);
        unsupported("creating a repository")
    }
    /// Makes the worktree ignore file modes and links a browser turned into files (SPEC-099).
    async fn keep_modes_and_links(&self, worktree: &Path) -> AppResult<()> {
        let _ = worktree;
        unsupported("keeping a repository's modes")
    }
    /// Fetches every branch of the repository at `source` into `repo`'s `refs/remotes/origin/*`
    /// by that path, not by `repo`'s configured URL (a playground's refresh, SPEC-099).
    async fn fetch_branches_from(&self, repo: &Path, source: &Path) -> AppResult<()> {
        let _ = (repo, source);
        unsupported("fetching a playground's origin")
    }
    /// Copies every branch and tag of the repository at `source` into the bare repository
    /// `repo` (SPEC-099: an uploaded folder's history).
    async fn copy_refs(&self, repo: &Path, source: &Path) -> AppResult<()> {
        let _ = (repo, source);
        unsupported("copying a repository's history")
    }
    /// Lands the request's head on its branch of `origin` as one commit: the tree Git's merge of
    /// `base` and `head` gives, with `base` as its only parent, pushed without forcing (SPEC-099:
    /// a playground's merge).
    async fn land_squash(
        &self,
        repo: &Path,
        request: &SquashRequest<'_>,
    ) -> AppResult<SquashLanding> {
        let _ = (repo, request);
        unsupported("merging in a playground")
    }
    /// Imports a bundle's refs under `refs/heads/<prefix>/…`.
    async fn import_bundle(
        &self,
        repo: &Path,
        bundle: &Path,
        prefix: &str,
    ) -> AppResult<Vec<String>>;
}

/// What [`GitPort::land_squash`] lands on `origin` (a playground's merge, SPEC-099).
#[derive(Debug, Clone, Copy)]
pub struct SquashRequest<'a> {
    /// The integration branch as fetched (`origin/dev`): the commit's only parent.
    pub base: &'a str,
    /// The task branch's head.
    pub head: &'a str,
    /// The commit message.
    pub message: &'a str,
    /// Author and committer name.
    pub name: &'a str,
    /// Author and committer email.
    pub email: &'a str,
    /// The repository pushed to: the playground's `origin` by its path, never by the URL in
    /// `repo`'s configuration, which a sandbox may rewrite.
    pub remote: &'a str,
    /// The branch of `remote` that receives the commit.
    pub branch: &'a str,
}

/// What [`GitPort::land_squash`] did (a playground's merge, SPEC-099).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SquashLanding {
    /// One new commit on the branch: its id.
    Landed(String),
    /// The head brings nothing new; nothing was pushed.
    Unchanged,
    /// Git's merge conflicts in these paths; nothing was pushed.
    Conflict(Vec<String>),
}

/// How a pull request lands on the integration branch (the profile's merge method).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainLanding {
    /// A squash: one commit with the head's tree on top of the integration branch.
    Squash,
    /// A merge commit of the integration branch and the head.
    MergeCommit,
}

/// One pull request of a built merge train.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainStep {
    /// It merged cleanly: the integration branch's tree once it has landed.
    Clean {
        /// That tree.
        tree: String,
    },
    /// Bringing it up to date conflicts in these paths: it is left out.
    Conflict(Vec<String>),
}

/// What [`GitPort::build_train`] built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainBuild {
    /// The integration branch commit it starts from.
    pub base: String,
    /// Its tree.
    pub base_tree: String,
    /// One step per head, in order.
    pub steps: Vec<TrainStep>,
    /// The integration branch once every clean head landed: the commit CI tests.
    pub head: String,
}

/// What [`GitPort::merge_integration`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeIntegration {
    /// The branch already contains the integration branch.
    UpToDate,
    /// The new head after the merge.
    Merged(String),
    /// The merge conflicted in these paths and was aborted.
    Conflict(Vec<String>),
}
