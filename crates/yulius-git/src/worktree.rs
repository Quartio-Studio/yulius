//! Task worktrees: creation, listing and guarded removal (GIT-002).

use std::path::{Path, PathBuf};

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::{require_absolute, validate_branch_name};

/// One entry of `git worktree list --porcelain -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute worktree path as Git records it.
    pub path: PathBuf,
    /// Checked-out commit, absent for a bare entry.
    pub head: Option<String>,
    /// Checked-out branch without `refs/heads/`, absent when detached or bare.
    pub branch: Option<String>,
    /// The bare repository entry.
    pub bare: bool,
    /// `HEAD` is detached.
    pub detached: bool,
    /// The worktree is locked (`git worktree lock`).
    pub locked: bool,
    /// Git considers the worktree prunable (its directory is gone).
    pub prunable: bool,
}

/// Why [`Git::worktree_remove_if_safe`] refused to remove a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalRefusal {
    /// The path is not strictly inside the caller's owned root (after resolving symlinks).
    OutsideOwnedRoot,
    /// The path does not exist.
    Missing,
    /// The path is not a worktree registered in this repository.
    NotRegistered,
    /// The path is the repository's main worktree.
    MainWorktree,
    /// The worktree is locked.
    Locked,
    /// The worktree has tracked modifications or untracked, non-ignored files.
    Dirty {
        /// Number of `git status` entries found.
        entries: usize,
    },
}

/// Outcome of [`Git::worktree_remove_if_safe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum WorktreeRemoval {
    /// The worktree was removed (its branch is kept).
    Removed,
    /// Nothing was changed.
    Refused(RemovalRefusal),
}

impl Git {
    /// Creates a worktree at the absolute `path` on the new branch `new_branch` starting at
    /// `base` (resolved to a commit first). Returns the base commit id.
    pub fn worktree_add(
        &self,
        repo: &Path,
        path: &Path,
        new_branch: &str,
        base: &str,
    ) -> Result<String, GitError> {
        validate_branch_name(new_branch)?;
        require_absolute("worktree path", path)?;
        let base_sha = self.rev_parse(repo, base)?;
        if self.branch_exists(repo, new_branch)? {
            return Err(GitError::BranchExists(new_branch.to_owned()));
        }
        self.run_ok(
            Invocation::new(self, "worktree add", Some(repo))
                .args(["worktree", "add", "--quiet", "-b", new_branch, "--"])
                .arg(path)
                .arg(&base_sha),
        )?;
        Ok(base_sha)
    }

    /// Checks out the existing local branch `branch` in a new worktree at `path` (absolute).
    /// Returns the branch head.
    ///
    /// # Errors
    /// Invalid names, a missing branch, or Git failures (for example a branch already checked
    /// out elsewhere).
    pub fn worktree_add_existing(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
    ) -> Result<String, GitError> {
        validate_branch_name(branch)?;
        require_absolute("worktree path", path)?;
        if !self.branch_exists(repo, branch)? {
            return Err(GitError::InvalidInput {
                what: "branch",
                reason: format!("{branch} does not exist"),
            });
        }
        self.run_ok(
            Invocation::new(self, "worktree add", Some(repo))
                .args(["worktree", "add", "--quiet", "--"])
                .arg(path)
                .arg(branch),
        )?;
        self.rev_parse(path, "HEAD")
    }

    /// Lists the repository's worktrees; the main worktree comes first.
    pub fn worktree_list(&self, repo: &Path) -> Result<Vec<Worktree>, GitError> {
        let output = self.run_ok(Invocation::new(self, "worktree list", Some(repo)).args([
            "worktree",
            "list",
            "--porcelain",
            "-z",
        ]))?;
        parse_worktree_list(&output.stdout)
    }

    /// Removes the worktree at `path` only when it is strictly inside `owned_root`,
    /// registered in `repo`, not the main worktree, not locked and clean (no tracked
    /// changes, no untracked non-ignored files). Never uses `--force`; a refusal changes
    /// nothing. The worktree's branch is kept.
    pub fn worktree_remove_if_safe(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
    ) -> Result<WorktreeRemoval, GitError> {
        let root = owned_root.canonicalize().map_err(|source| GitError::Io {
            context: "resolving the owned worktree root",
            source,
        })?;
        let target = match path.canonicalize() {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WorktreeRemoval::Refused(RemovalRefusal::Missing));
            }
            Err(source) => {
                return Err(GitError::Io {
                    context: "resolving the worktree path",
                    source,
                });
            }
        };
        if target == root || !target.starts_with(&root) {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::OutsideOwnedRoot));
        }
        let worktrees = self.worktree_list(repo)?;
        let position = worktrees.iter().position(|entry| {
            entry
                .path
                .canonicalize()
                .is_ok_and(|candidate| candidate == target)
        });
        let Some(index) = position else {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::NotRegistered));
        };
        if index == 0 {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::MainWorktree));
        }
        if worktrees[index].locked {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::Locked));
        }
        let entries = self.status_porcelain(&target)?.len();
        if entries > 0 {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::Dirty { entries }));
        }
        self.run_ok(
            Invocation::new(self, "worktree remove", Some(repo))
                .args(["worktree", "remove", "--"])
                .arg(&target),
        )?;
        Ok(WorktreeRemoval::Removed)
    }
}

fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn parse_worktree_list(stdout: &[u8]) -> Result<Vec<Worktree>, GitError> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            continue;
        }
        let (key, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(space) => (&field[..space], &field[space + 1..]),
            None => (field, &[][..]),
        };
        if key == b"worktree" {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            current = Some(Worktree {
                path: bytes_to_path(value),
                head: None,
                branch: None,
                bare: false,
                detached: false,
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            return Err(GitError::Parse {
                command: "worktree list",
                detail: "attribute before worktree line".to_owned(),
            });
        };
        let text = String::from_utf8_lossy(value).into_owned();
        match key {
            b"HEAD" => entry.head = Some(text),
            b"branch" => {
                entry.branch = Some(text.strip_prefix("refs/heads/").unwrap_or(&text).to_owned());
            }
            b"bare" => entry.bare = true,
            b"detached" => entry.detached = true,
            b"locked" => entry.locked = true,
            b"prunable" => entry.prunable = true,
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        worktrees.push(done);
    }
    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_records() -> Result<(), GitError> {
        let raw = b"worktree /r\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0\
worktree /w/a b\0HEAD 2222222222222222222222222222222222222222\0detached\0locked reason here\0\0";
        let list = parse_worktree_list(raw)?;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].path, PathBuf::from("/w/a b"));
        assert!(list[1].detached && list[1].locked && list[1].branch.is_none());
        Ok(())
    }
}
