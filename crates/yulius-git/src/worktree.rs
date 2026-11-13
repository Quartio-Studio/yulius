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
    /// The worktree's Git record holds submodules (`modules/`), which `git worktree remove`
    /// refuses too.
    Submodules,
}

/// Removes `path` (a folder, a file or a link) below the trusted folder `root` without
/// following any link on the way, as `yulius_app::leftovers::remove_below` does: a folder a
/// sandbox swapped for a link after the checks has the link removed, never what it names
/// (SPEC-401, ADR-0255).
pub type RemoveBelow<'a> = &'a dyn Fn(&Path, &Path) -> std::io::Result<()>;

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
                .arg(&base_sha)
                .touching(path),
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
                .arg(branch)
                .touching(path),
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
    /// nothing. The worktree's branch is kept. Git removes it, resolving the path once more, so
    /// a folder others can swap for a link takes [`Git::worktree_remove_if_safe_below`].
    pub fn worktree_remove_if_safe(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
    ) -> Result<WorktreeRemoval, GitError> {
        self.remove_if_safe(repo, path, owned_root, None)
    }

    /// [`Git::worktree_remove_if_safe`] with the same checks, but `remove` removes the worktree
    /// folder (below the parent of `owned_root`) and then its Git record `worktrees/<id>` (below
    /// the parent of `repo`), so nothing resolves the path after the checks (SPEC-401). Both
    /// parents must be folders nobody but the caller changes. A worktree whose record holds
    /// submodules is refused.
    ///
    /// # Errors
    /// Git failures; `Io` when `remove` fails.
    pub fn worktree_remove_if_safe_below(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        remove: RemoveBelow<'_>,
    ) -> Result<WorktreeRemoval, GitError> {
        self.remove_if_safe(repo, path, owned_root, Some(remove))
    }

    fn remove_if_safe(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        remove: Option<RemoveBelow<'_>>,
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
        let Some(remove) = remove else {
            self.run_ok(
                Invocation::new(self, "worktree remove", Some(repo))
                    .args(["worktree", "remove", "--"])
                    .arg(&target)
                    .touching(&target),
            )?;
            return Ok(WorktreeRemoval::Removed);
        };
        let tree_root = trusted_parent(owned_root)?;
        let Some(admin) = self.worktree_record(repo, &target)? else {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::NotRegistered));
        };
        if !target.starts_with(&tree_root) {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::OutsideOwnedRoot));
        }
        if self.holds_submodules(&admin, &target)? {
            return Ok(WorktreeRemoval::Refused(RemovalRefusal::Submodules));
        }
        self.remove_tree_and_record(repo, (&tree_root, &target), Some(&admin), remove)?;
        Ok(WorktreeRemoval::Removed)
    }

    /// Discards a task worktree on explicit request (task deletion, SPEC-001 TSK-009): removes
    /// it even with uncommitted changes, prunes stale registrations and deletes the local
    /// branch. Only worktrees strictly inside `owned_root` are touched; the main worktree never.
    /// Git removes a registered one, resolving the path once more, so a folder others can swap
    /// for a link takes [`Git::worktree_discard_below`].
    ///
    /// # Errors
    /// Git failures; `Invalid` for unsafe branch names.
    pub fn worktree_discard(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        branch: Option<&str>,
    ) -> Result<(), GitError> {
        self.discard(repo, path, owned_root, branch, None)
    }

    /// [`Git::worktree_discard`] where `remove` removes the worktree folder and its Git record,
    /// as for [`Git::worktree_remove_if_safe_below`] (SPEC-401).
    ///
    /// # Errors
    /// Git failures; `Invalid` for unsafe branch names; `Io` when `remove` fails.
    pub fn worktree_discard_below(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        branch: Option<&str>,
        remove: RemoveBelow<'_>,
    ) -> Result<(), GitError> {
        self.discard(repo, path, owned_root, branch, Some(remove))
    }

    fn discard(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
        branch: Option<&str>,
        remove: Option<RemoveBelow<'_>>,
    ) -> Result<(), GitError> {
        if let Some(branch) = branch {
            validate_branch_name(branch)?;
        }
        let root = owned_root.canonicalize().map_err(|source| GitError::Io {
            context: "resolving the owned worktree root",
            source,
        })?;
        if let Ok(target) = path.canonicalize()
            && target != root
            && target.starts_with(&root)
        {
            let worktrees = self.worktree_list(repo)?;
            let registered = worktrees.iter().skip(1).any(|entry| {
                entry
                    .path
                    .canonicalize()
                    .is_ok_and(|candidate| candidate == target)
            });
            if let Some(remove) = remove {
                let tree_root = trusted_parent(owned_root)?;
                let admin = if registered {
                    self.worktree_record(repo, &target)?
                } else {
                    None
                };
                if target.starts_with(&tree_root) {
                    self.remove_tree_and_record(
                        repo,
                        (&tree_root, &target),
                        admin.as_deref(),
                        remove,
                    )?;
                }
            } else if registered {
                self.run_ok(
                    Invocation::new(self, "worktree remove", Some(repo))
                        .args(["worktree", "remove", "--force", "--force", "--"])
                        .arg(&target)
                        .touching(&target),
                )?;
            } else {
                std::fs::remove_dir_all(&target).map_err(|source| GitError::Io {
                    context: "removing an unregistered worktree directory",
                    source,
                })?;
            }
        }
        self.run_ok(
            Invocation::new(self, "worktree prune", Some(repo)).args(["worktree", "prune"]),
        )?;
        if let Some(branch) = branch
            && self.branch_exists(repo, branch)?
        {
            self.run_ok(
                Invocation::new(self, "branch delete", Some(repo))
                    .args(["branch", "-D"])
                    .arg(branch),
            )?;
        }
        Ok(())
    }

    /// Whether the worktree `target` holds submodules, as `git worktree remove` refuses them: its
    /// record has `modules/`, or its index has a gitlink whose folder holds a `.git`.
    fn holds_submodules(&self, admin: &Path, target: &Path) -> Result<bool, GitError> {
        if admin.join("modules").exists() {
            return Ok(true);
        }
        let output = self.run_ok(
            Invocation::new(self, "ls-files", Some(target)).args(["ls-files", "--stage", "-z"]),
        )?;
        Ok(output.stdout.split(|byte| *byte == 0).any(|entry| {
            entry.starts_with(b"160000 ")
                && entry
                    .iter()
                    .position(|byte| *byte == b'\t')
                    .is_some_and(|tab| {
                        target
                            .join(bytes_to_path(&entry[tab + 1..]))
                            .join(".git")
                            .exists()
                    })
        }))
    }

    /// The Git record (`<common dir>/worktrees/<id>`) of the registered worktree `target`, or
    /// `None` when the worktree names another folder as its record or the record does not name
    /// the worktree back (its `gitdir`).
    fn worktree_record(&self, repo: &Path, target: &Path) -> Result<Option<PathBuf>, GitError> {
        let resolved = |dir: &Path, arg: &str| -> Result<PathBuf, GitError> {
            let output = self
                .run_ok(Invocation::new(self, "rev-parse", Some(dir)).args(["rev-parse", arg]))?;
            let text = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
            dir.join(bytes_to_path(text))
                .canonicalize()
                .map_err(|source| GitError::Io {
                    context: "resolving a worktree's Git record",
                    source,
                })
        };
        let records = resolved(repo, "--git-common-dir")?.join("worktrees");
        let record = resolved(target, "--absolute-git-dir")?;
        if record.parent() != Some(records.as_path()) {
            return Ok(None);
        }
        // A relative `gitdir` (Git 2.48+ `--relative-paths`) is relative to the record.
        let named = std::fs::read(record.join("gitdir")).ok().and_then(|text| {
            let text = text.strip_suffix(b"\n").unwrap_or(&text);
            record.join(bytes_to_path(text)).canonicalize().ok()
        });
        let names_back = named.is_some() && named == target.join(".git").canonicalize().ok();
        Ok(names_back.then_some(record))
    }

    /// Removes the worktree folder `tree.1` below `tree.0`, then its Git record `admin` below the
    /// parent of `repo` while no other worktree registration changes (a `git worktree list` of a
    /// half-removed record fails, #666).
    fn remove_tree_and_record(
        &self,
        repo: &Path,
        tree: (&Path, &Path),
        admin: Option<&Path>,
        remove: RemoveBelow<'_>,
    ) -> Result<(), GitError> {
        remove(tree.0, tree.1).map_err(|source| GitError::Io {
            context: "removing the worktree folder",
            source,
        })?;
        if let Some(admin) = admin {
            let admin_root = trusted_parent(repo)?;
            let _changes = crate::runner::worktree_changes();
            remove(&admin_root, admin).map_err(|source| GitError::Io {
                context: "removing the worktree's Git record",
                source,
            })?;
        }
        Ok(())
    }
}

/// The resolved parent of `dir`: the trusted folder a removal walks down from.
fn trusted_parent(dir: &Path) -> Result<PathBuf, GitError> {
    dir.parent()
        .ok_or_else(|| GitError::Io {
            context: "finding the folder above a worktree root",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })?
        .canonicalize()
        .map_err(|source| GitError::Io {
            context: "resolving the folder above a worktree root",
            source,
        })
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
