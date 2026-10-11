//! Git operations for Yulius-run CI and repository hygiene (SPEC-001 DLV-006/007, SPEC-002
//! ADR-20): persistent detached CI checkouts, reading one file or directory at a revision, and
//! listing and deleting branches.

use std::path::Path;

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::{require_absolute, validate_branch_name, validate_relative_path};

/// One branch with its tip and the time of its tip commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTip {
    /// Name without `refs/heads/` or `refs/remotes/origin/`.
    pub name: String,
    /// Tip commit.
    pub sha: String,
    /// Committer time of the tip, seconds since the Unix epoch.
    pub committed_at: i64,
}

impl Git {
    /// Whether `rev` names a commit in `repo`; its full id when it does.
    ///
    /// # Errors
    /// Invalid revisions or Git failures other than "not found".
    pub fn resolve_commit(&self, repo: &Path, rev: &str) -> Result<Option<String>, GitError> {
        match self.rev_parse(repo, rev) {
            Ok(sha) => Ok(Some(sha)),
            Err(GitError::RevisionNotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The contents of `path` (repository-relative) at `rev`; `None` when the file does not
    /// exist there. At most [`Git::max_output_bytes`] are read.
    ///
    /// # Errors
    /// Invalid input, an unknown revision, or Git failures.
    pub fn show_file(
        &self,
        repo: &Path,
        rev: &str,
        path: &str,
    ) -> Result<Option<String>, GitError> {
        let relative = validate_relative_path(path)?;
        let sha = self.rev_parse(repo, rev)?;
        let spec = format!("{sha}:{}", relative.to_string_lossy());
        let exists = self.run(
            Invocation::new(self, "cat-file", Some(repo))
                .args(["cat-file", "-e", "--end-of-options"])
                .arg(&spec),
        )?;
        if !exists.status.success() {
            return Ok(None);
        }
        let output = self.run_ok(
            Invocation::new(self, "cat-file", Some(repo))
                .args(["cat-file", "blob", "--end-of-options"])
                .arg(&spec),
        )?;
        Ok(Some(output.stdout_text()))
    }

    /// Names of the files directly in directory `dir` (repository-relative) at `rev`.
    ///
    /// # Errors
    /// Invalid input, an unknown revision, or Git failures.
    pub fn list_dir(&self, repo: &Path, rev: &str, dir: &str) -> Result<Vec<String>, GitError> {
        let relative = validate_relative_path(dir)?;
        let sha = self.rev_parse(repo, rev)?;
        let output = self.run_ok(
            Invocation::new(self, "ls-tree", Some(repo))
                .args(["ls-tree", "--name-only", "-z", "--end-of-options"])
                .arg(&sha)
                .arg("--")
                .arg(format!("{}/", relative.to_string_lossy())),
        )?;
        Ok(output
            .stdout
            .split(|b| *b == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect())
    }

    /// Prepares a persistent CI checkout at the absolute `path` (inside `owned_root`) on commit
    /// `sha`, detached: added as a worktree of `repo` the first time, then force-checked-out and
    /// cleaned of untracked files. Ignored files (build outputs, dependency directories) are
    /// kept, so the next run of the job reuses them as a cache.
    ///
    /// # Errors
    /// Paths outside `owned_root`, unknown commits, or Git failures.
    pub fn ci_checkout(
        &self,
        repo: &Path,
        path: &Path,
        sha: &str,
        owned_root: &Path,
    ) -> Result<(), GitError> {
        require_absolute("CI checkout path", path)?;
        if !path.starts_with(owned_root) || path == owned_root {
            return Err(GitError::InvalidInput {
                what: "CI checkout path",
                reason: "outside the CI directory".into(),
            });
        }
        let commit = self.rev_parse(repo, sha)?;
        let registered = self
            .worktree_list(repo)?
            .iter()
            .skip(1)
            .any(|w| w.path == path);
        if !registered || !path.join(".git").exists() {
            if path.exists() {
                std::fs::remove_dir_all(path).map_err(|source| GitError::Io {
                    context: "removing a broken CI checkout",
                    source,
                })?;
            }
            self.run_ok(
                Invocation::new(self, "worktree prune", Some(repo)).args(["worktree", "prune"]),
            )?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| GitError::Io {
                    context: "creating the CI directory",
                    source,
                })?;
            }
            self.run_ok(
                Invocation::new(self, "worktree add", Some(repo))
                    .args(["worktree", "add", "--quiet", "--detach", "--"])
                    .arg(path)
                    .arg(&commit),
            )?;
            return Ok(());
        }
        // `commit` is a full object id from `rev-parse`, never an option. (`checkout --detach`
        // refuses `--end-of-options` in Git 2.43, Ubuntu 24.04's.)
        self.run_ok(
            Invocation::new(self, "checkout", Some(path))
                .args(["checkout", "--quiet", "--force", "--detach"])
                .arg(&commit),
        )?;
        // Untracked files go; ignored ones (caches) stay. Submodules are not touched.
        self.run_ok(Invocation::new(self, "clean", Some(path)).args(["clean", "-f", "-d", "-q"]))?;
        Ok(())
    }

    fn branch_tips(&self, repo: &Path, prefix: &str) -> Result<Vec<BranchTip>, GitError> {
        let output = self.run_ok(
            Invocation::new(self, "for-each-ref", Some(repo))
                .args([
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)%00%(committerdate:unix)",
                ])
                .arg(prefix),
        )?;
        let mut tips = Vec::new();
        for line in output.stdout_text().lines() {
            let mut parts = line.split('\0');
            let (Some(name), Some(sha), Some(at)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(name) = name.strip_prefix(prefix) else {
                continue;
            };
            if name == "HEAD" || validate_branch_name(name).is_err() {
                continue;
            }
            tips.push(BranchTip {
                name: name.to_owned(),
                sha: sha.to_owned(),
                committed_at: at.trim().parse().unwrap_or(0),
            });
        }
        Ok(tips)
    }

    /// Branches of `origin` as last fetched (`refs/remotes/origin/*`, without `HEAD`).
    ///
    /// # Errors
    /// Git failures.
    pub fn remote_branches(&self, repo: &Path) -> Result<Vec<BranchTip>, GitError> {
        self.branch_tips(repo, "refs/remotes/origin/")
    }

    /// Local branches (`refs/heads/*`).
    ///
    /// # Errors
    /// Git failures.
    pub fn local_branches(&self, repo: &Path) -> Result<Vec<BranchTip>, GitError> {
        self.branch_tips(repo, "refs/heads/")
    }

    /// Whether commit `ancestor` is reachable from `descendant`.
    ///
    /// # Errors
    /// Invalid revisions or Git failures.
    pub fn is_ancestor(
        &self,
        repo: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, GitError> {
        let a = self.rev_parse(repo, ancestor)?;
        let d = self.rev_parse(repo, descendant)?;
        let output = self.run(
            Invocation::new(self, "merge-base", Some(repo))
                .args(["merge-base", "--is-ancestor", "--end-of-options"])
                .arg(&a)
                .arg(&d),
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(crate::runner::failure("merge-base", &output)),
        }
    }

    /// Whether some remote-tracking branch contains commit `sha` (nothing would be lost by
    /// deleting a local branch at it).
    ///
    /// # Errors
    /// Invalid revisions or Git failures.
    pub fn on_remote(&self, repo: &Path, sha: &str) -> Result<bool, GitError> {
        let commit = self.rev_parse(repo, sha)?;
        let output = self.run_ok(
            Invocation::new(self, "for-each-ref", Some(repo))
                .args(["for-each-ref", "--count=1", "--format=%(refname)"])
                .arg(format!("--contains={commit}"))
                .arg("refs/remotes/"),
        )?;
        Ok(!output.stdout_trimmed().is_empty())
    }

    /// Deletes local branch `name` (it must not be checked out).
    ///
    /// # Errors
    /// Invalid names or Git failures.
    pub fn delete_local_branch(&self, repo: &Path, name: &str) -> Result<(), GitError> {
        validate_branch_name(name)?;
        self.run_ok(
            Invocation::new(self, "branch delete", Some(repo))
                .args(["branch", "-D"])
                .arg(name),
        )?;
        Ok(())
    }

    /// Forgets worktrees whose directories are gone (`git worktree prune`).
    ///
    /// # Errors
    /// Git failures.
    pub fn worktree_prune(&self, repo: &Path) -> Result<(), GitError> {
        self.run_ok(
            Invocation::new(self, "worktree prune", Some(repo)).args(["worktree", "prune"]),
        )?;
        Ok(())
    }
}
