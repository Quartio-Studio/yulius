//! Merges of the integration branch into a task's branch (SPEC-083): what Git's own merge of two
//! commits gives, whether a branch moved past a reviewed head only through clean merges of the
//! integration branch, and the merge attributes Yulius keeps in a repository's `info/attributes`.

use std::path::{Path, PathBuf};

use crate::Git;
use crate::error::GitError;
use crate::repo::is_object_id;
use crate::runner::{Invocation, failure};

/// First line of Yulius's block in `info/attributes`.
pub const ATTRIBUTES_BEGIN: &str = "# >>> yulius: merge attributes (SPEC-083); rewritten by Yulius";
/// Last line of Yulius's block in `info/attributes`.
pub const ATTRIBUTES_END: &str = "# <<< yulius";

impl Git {
    /// The parents of commit `rev`, in order (none for a root commit).
    ///
    /// # Errors
    /// Invalid or unknown revisions, or Git failures.
    pub fn parents(&self, repo: &Path, rev: &str) -> Result<Vec<String>, GitError> {
        let sha = self.rev_parse(repo, rev)?;
        let output = self.run_ok(
            Invocation::new(self, "rev-list", Some(repo))
                .args(["rev-list", "--parents", "-n", "1", "--end-of-options"])
                .arg(&sha),
        )?;
        let text = output.stdout_trimmed();
        let mut ids = text.split_whitespace();
        if ids.next() != Some(sha.as_str()) {
            return Err(GitError::Parse {
                command: "rev-list",
                detail: format!("unexpected parents line {text:?}"),
            });
        }
        ids.map(|id| {
            is_object_id(id)
                .then(|| id.to_owned())
                .ok_or_else(|| GitError::Parse {
                    command: "rev-list",
                    detail: format!("not an object id: {id:?}"),
                })
        })
        .collect()
    }

    /// The tree Git's own merge of `ours` and `theirs` gives, run in `worktree` so the
    /// repository's merge drivers apply (its checked-out `.gitattributes` and `info/attributes`);
    /// `None` when the merge conflicts. Nothing in the worktree changes.
    ///
    /// # Errors
    /// Invalid or unknown revisions, or Git failures (a Git without `merge-tree --write-tree`,
    /// before 2.38, fails here).
    pub fn merge_tree(
        &self,
        worktree: &Path,
        ours: &str,
        theirs: &str,
    ) -> Result<Option<String>, GitError> {
        let (ours, theirs) = (
            self.rev_parse(worktree, ours)?,
            self.rev_parse(worktree, theirs)?,
        );
        let output = self.run(
            Invocation::new(self, "merge-tree", Some(worktree))
                .args([
                    "merge-tree",
                    "--write-tree",
                    "--no-messages",
                    "--end-of-options",
                ])
                .arg(&ours)
                .arg(&theirs),
        )?;
        match output.status.code() {
            Some(0) => {
                let text = output.stdout_text();
                let tree = text.lines().next().unwrap_or_default();
                if is_object_id(tree) {
                    Ok(Some(tree.to_owned()))
                } else {
                    Err(GitError::Parse {
                        command: "merge-tree",
                        detail: format!("not a tree id: {tree:?}"),
                    })
                }
            }
            Some(1) => Ok(None),
            _ => Err(failure("merge-tree", &output)),
        }
    }

    /// Whether `head` is `reviewed` plus only clean merges of the integration branch (SPEC-083
    /// REV-001). Walking first parents from `head` back to `reviewed`, every commit must be a
    /// merge of exactly two parents whose second one `integration` (a remote-tracking ref)
    /// already contains, recording exactly the tree [`Git::merge_tree`] gives for its parents:
    /// no conflict resolved by hand, no other change. At most `limit` such merges; `false` when
    /// `reviewed` is not reached.
    ///
    /// # Errors
    /// Invalid revisions or Git failures; an unknown `reviewed` is `Ok(false)`.
    pub fn only_clean_merges_since(
        &self,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: &str,
        limit: usize,
    ) -> Result<bool, GitError> {
        let Some(reviewed) = self.resolve_commit(worktree, reviewed)? else {
            return Ok(false);
        };
        let mut current = self.rev_parse(worktree, head)?;
        for _ in 0..=limit {
            if current == reviewed {
                return Ok(true);
            }
            let parents = self.parents(worktree, &current)?;
            let [first, second] = parents.as_slice() else {
                return Ok(false);
            };
            if !self.is_ancestor(worktree, second, integration)? {
                return Ok(false);
            }
            let Some(merged) = self.merge_tree(worktree, first, second)? else {
                return Ok(false);
            };
            if self.tree_of(worktree, &current)?.as_deref() != Some(merged.as_str()) {
                return Ok(false);
            }
            current.clone_from(first);
        }
        Ok(false)
    }

    /// The repository's common Git directory (shared by all its worktrees), absolute.
    ///
    /// # Errors
    /// Git failures.
    pub fn common_dir(&self, repo: &Path) -> Result<PathBuf, GitError> {
        let output = self.run_ok(
            Invocation::new(self, "rev-parse", Some(repo)).args(["rev-parse", "--git-common-dir"]),
        )?;
        let dir = PathBuf::from(output.stdout_trimmed());
        Ok(if dir.is_absolute() {
            dir
        } else {
            repo.join(dir)
        })
    }

    /// Keeps Yulius's block in the repository's `info/attributes` (read by every worktree of the
    /// repository) at exactly `lines`; the rest of the file is kept as it is. The file is written
    /// whole and renamed into place, only when it changes. Returns whether it changed.
    ///
    /// # Errors
    /// A line with a line break or control character; Git or file failures.
    pub fn set_managed_attributes(&self, repo: &Path, lines: &[String]) -> Result<bool, GitError> {
        if let Some(bad) = lines
            .iter()
            .find(|line| line.trim().is_empty() || line.chars().any(char::is_control))
        {
            return Err(GitError::InvalidInput {
                what: "attributes line",
                reason: crate::error::truncate_for_error(bad),
            });
        }
        let info = self.common_dir(repo)?.join("info");
        let path = info.join("attributes");
        let current = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(GitError::Io {
                    context: "reading info/attributes",
                    source,
                });
            }
        };
        let next = with_managed_block(&current, lines);
        if next == current {
            return Ok(false);
        }
        let io = |context: &'static str| move |source| GitError::Io { context, source };
        std::fs::create_dir_all(&info).map_err(io("creating the info directory"))?;
        let tmp = info.join("attributes.yulius-tmp");
        std::fs::write(&tmp, next).map_err(io("writing info/attributes"))?;
        std::fs::rename(&tmp, &path).map_err(io("replacing info/attributes"))?;
        Ok(true)
    }
}

/// `text` with Yulius's block replaced by `lines` (removed when there are none; appended when it
/// was missing).
fn with_managed_block(text: &str, lines: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == ATTRIBUTES_BEGIN {
            inside = true;
        } else if inside && line == ATTRIBUTES_END {
            inside = false;
        } else if !inside {
            kept.push(line);
        }
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = String::new();
    for line in kept {
        out.push_str(line);
        out.push('\n');
    }
    if !lines.is_empty() {
        out.push_str(ATTRIBUTES_BEGIN);
        out.push('\n');
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(ATTRIBUTES_END);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_managed_block_is_replaced_and_the_rest_kept() {
        let lines = vec!["/docs/changes.md merge=union".to_owned()];
        let first = with_managed_block("*.png binary\n", &lines);
        assert_eq!(
            first,
            format!(
                "*.png binary\n{ATTRIBUTES_BEGIN}\n/docs/changes.md merge=union\n{ATTRIBUTES_END}\n"
            )
        );
        assert_eq!(with_managed_block(&first, &lines), first, "stable");
        let other = vec!["/CHANGELOG.md merge=union".to_owned()];
        let second = with_managed_block(&first, &other);
        assert!(second.starts_with("*.png binary\n"));
        assert!(second.contains("/CHANGELOG.md merge=union") && !second.contains("docs/changes"));
        assert_eq!(with_managed_block(&second, &[]), "*.png binary\n");
        assert_eq!(with_managed_block("", &[]), "");
    }
}
