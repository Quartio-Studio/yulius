//! Merges of the integration branch into a task's branch (SPEC-083): what Git's own merge of two
//! commits gives, whether a branch moved past a reviewed head only through clean merges of the
//! integration branch, and the merge attributes Yulius keeps in a repository's `info/attributes`.

use std::path::Path;

use crate::Git;
use crate::commit::{Author, MAX_COMMIT_MESSAGE};
use crate::error::GitError;
use crate::repo::is_object_id;
use crate::runner::{Invocation, Route, failure};
use crate::validate::{validate_branch_name, validate_remote, validate_revision};

/// What [`Git::squash_land`] lands, and where.
#[derive(Debug, Clone, Copy)]
pub struct SquashInput<'a> {
    /// The branch's tip as fetched (e.g. `origin/dev`): the new commit's only parent.
    pub base: &'a str,
    /// The head whose changes land.
    pub head: &'a str,
    /// The commit message.
    pub message: &'a str,
    /// Who makes the commit.
    pub author: &'a Author,
    /// The remote pushed to.
    pub remote: &'a str,
    /// The remote branch that receives the commit.
    pub branch: &'a str,
}

/// What [`Git::squash_land`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SquashLanding {
    /// One new commit, pushed: its id.
    Landed(String),
    /// The head brings nothing the branch does not have; nothing was pushed.
    Unchanged,
    /// Git's merge of the head and the branch conflicts in these paths; nothing was pushed.
    Conflict(Vec<String>),
}

/// First line of Yulius's block in `info/attributes`.
pub const ATTRIBUTES_BEGIN: &str = "# >>> yulius: merge attributes (SPEC-083); rewritten by Yulius";
/// Last line of Yulius's block in `info/attributes`.
pub const ATTRIBUTES_END: &str = "# <<< yulius";

impl Git {
    /// Lands the input's `head` on `branch` of `remote` as one commit (a playground's merge, SPEC-099): the
    /// tree Git's merge of `base` (the branch's tip as fetched, e.g. `origin/dev`) and `head`
    /// gives, run in `worktree` so the repository's merge drivers apply, committed as `author`
    /// with `message` and `base` as its only parent, then pushed without forcing (a branch that
    /// moved meanwhile refuses it). Nothing changes in `worktree`.
    ///
    /// # Errors
    /// Invalid input, unknown revisions, a refused push, or Git failures.
    pub fn squash_land(
        &self,
        worktree: &Path,
        input: &SquashInput<'_>,
    ) -> Result<SquashLanding, GitError> {
        let SquashInput {
            base,
            head,
            message,
            author,
            remote,
            branch,
        } = *input;
        validate_revision(base)?;
        validate_revision(head)?;
        validate_remote(remote)?;
        validate_branch_name(branch)?;
        author.validate()?;
        if message.trim().is_empty() || message.len() > MAX_COMMIT_MESSAGE || message.contains('\0')
        {
            return Err(GitError::InvalidInput {
                what: "commit message",
                reason: "empty, too long or containing NUL".into(),
            });
        }
        let (base, head) = (
            self.rev_parse(worktree, base)?,
            self.rev_parse(worktree, head)?,
        );
        let output = self.run(
            Invocation::new(self, "merge-tree", Some(worktree))
                .args([
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "--no-messages",
                    "--end-of-options",
                ])
                .arg(&base)
                .arg(&head),
        )?;
        let text = output.stdout_text();
        let mut lines = text.lines();
        let tree = lines.next().unwrap_or_default().to_owned();
        match output.status.code() {
            Some(0) if is_object_id(&tree) => {}
            Some(1) => {
                let mut files: Vec<String> = lines
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect();
                files.dedup();
                return Ok(SquashLanding::Conflict(files));
            }
            Some(0) => {
                return Err(GitError::Parse {
                    command: "merge-tree",
                    detail: format!("not a tree id: {tree:?}"),
                });
            }
            _ => return Err(failure("merge-tree", &output)),
        }
        if self.tree_of(worktree, &base)?.as_deref() == Some(tree.as_str()) {
            return Ok(SquashLanding::Unchanged);
        }
        let committed = self.run_ok(
            Invocation::new(self, "commit-tree", Some(worktree))
                .args(["commit-tree", "-p"])
                .arg(&base)
                .arg("-m")
                .arg(message)
                .arg(&tree)
                .env("GIT_AUTHOR_NAME", author.name.as_str())
                .env("GIT_AUTHOR_EMAIL", author.email.as_str())
                .env("GIT_COMMITTER_NAME", author.name.as_str())
                .env("GIT_COMMITTER_EMAIL", author.email.as_str()),
        )?;
        let commit = committed.stdout_trimmed();
        if !is_object_id(&commit) {
            return Err(GitError::Parse {
                command: "commit-tree",
                detail: format!("not a commit id: {commit:?}"),
            });
        }
        let push = Invocation::new(self, "push", Some(worktree)).args(["push", "--quiet", "--"]);
        let push = match self.remote_route(worktree, remote, true)? {
            Route::Folder(origin) => push.arg(&origin).local(&origin),
            Route::Courier(courier, url) => {
                self.push_through(&courier, &url, &commit, &format!("refs/heads/{branch}"))?;
                return Ok(SquashLanding::Landed(commit));
            }
            Route::Named => push.arg(remote),
        };
        self.run_ok(
            push.arg(format!("{commit}:refs/heads/{branch}"))
                .timeout(self.network_timeout),
        )?;
        Ok(SquashLanding::Landed(commit))
    }

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
        self.merge_tree_from(worktree, None, ours, theirs)
    }

    /// [`Git::merge_tree`] with `base` as the merge base instead of the one Git finds: what
    /// `theirs` changed since `base`, applied to `ours` (SPEC-425: a follow-up after a squash
    /// merge, whose own merge base is the branch's old fork point).
    ///
    /// # Errors
    /// As [`Git::merge_tree`]; a Git before 2.40 (no `--merge-base`) fails here.
    pub fn merge_tree_from(
        &self,
        worktree: &Path,
        base: Option<&str>,
        ours: &str,
        theirs: &str,
    ) -> Result<Option<String>, GitError> {
        let (ours, theirs) = (
            self.rev_parse(worktree, ours)?,
            self.rev_parse(worktree, theirs)?,
        );
        let base = base.map(|rev| self.rev_parse(worktree, rev)).transpose()?;
        let mut invocation = Invocation::new(self, "merge-tree", Some(worktree)).args([
            "merge-tree",
            "--write-tree",
            "--no-messages",
        ]);
        if let Some(base) = &base {
            invocation = invocation.arg(format!("--merge-base={base}"));
        }
        let output = self.run(
            invocation
                .args(["--end-of-options"])
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

    /// The commits after `reviewed` on the way from `head` (first parents, newest first), when
    /// every merge on that way is a clean merge of `integration` (SPEC-493 APR-001, APR-003):
    /// `None` when `reviewed` is not reached within `limit` steps (it is not an ancestor of the
    /// head), or a merge on the way resolved a conflict or merged something else.
    pub fn follow_up_since(
        &self,
        worktree: &Path,
        reviewed: &str,
        head: &str,
        integration: &str,
        limit: usize,
    ) -> Result<Option<Vec<String>>, GitError> {
        let Some(reviewed) = self.resolve_commit(worktree, reviewed)? else {
            return Ok(None);
        };
        let mut current = self.rev_parse(worktree, head)?;
        let mut commits = Vec::new();
        for _ in 0..=limit {
            if current == reviewed {
                return Ok(Some(commits));
            }
            let parents = self.parents(worktree, &current)?;
            match parents.as_slice() {
                [parent] => {
                    commits.push(current.clone());
                    current.clone_from(parent);
                }
                [first, second] => {
                    if !self.is_ancestor(worktree, second, integration)? {
                        return Ok(None);
                    }
                    let Some(merged) = self.merge_tree(worktree, first, second)? else {
                        return Ok(None);
                    };
                    if self.tree_of(worktree, &current)?.as_deref() != Some(merged.as_str()) {
                        return Ok(None);
                    }
                    current.clone_from(first);
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// The lines `commit` added and removed against its first parent, per file, by line number
    /// (`-U0`, renames as a deletion and an addition, binary files as no line).
    pub fn commit_lines(&self, repo: &Path, commit: &str) -> Result<Vec<CommitFileLines>, GitError> {
        let commit = self.rev_parse(repo, commit)?;
        let output = self.run_ok(
            Invocation::new(self, "show", Some(repo))
                .args([
                    "show",
                    "--format=",
                    "-U0",
                    "--no-renames",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--first-parent",
                    "-m",
                    "--end-of-options",
                ])
                .arg(&commit)
                .arg("--"),
        )?;
        Ok(parse_unified_zero(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Keeps Yulius's block in `info/attributes` of the clone `repo` (read by every worktree of
    /// the repository) at exactly `lines`; the rest of the file is kept as it is. `repo` is the
    /// clone itself, with its own `.git` directory, never one of its worktrees: the file is
    /// reached from there through handles that follow no link (ADR-0314), read only when it is a
    /// regular file of at most 256 KiB, and replaced whole, only when it
    /// changes. Returns whether it changed.
    ///
    /// # Errors
    /// A line with a line break or control character; a link, a FIFO or a larger file where the
    /// attributes or their folder are; file failures.
    #[cfg(unix)]
    pub fn set_managed_attributes(&self, repo: &Path, lines: &[String]) -> Result<bool, GitError> {
        use crate::handles::{open_dir_in, open_git_dir, read_in, replace_in};
        if let Some(bad) = lines
            .iter()
            .find(|line| line.trim().is_empty() || line.chars().any(char::is_control))
        {
            return Err(GitError::InvalidInput {
                what: "attributes line",
                reason: crate::error::truncate_for_error(bad),
            });
        }
        let git_dir = open_git_dir(repo)?;
        let Some(info) = open_dir_in(&git_dir, "info", !lines.is_empty())? else {
            return Ok(false);
        };
        let what = "info/attributes";
        let (current, mode) = match read_in(&info, "attributes", MAX_ATTRIBUTES_BYTES, what)? {
            Some((bytes, mode)) => (
                String::from_utf8(bytes).map_err(|_| GitError::InvalidInput {
                    what,
                    reason: "it is not text".to_owned(),
                })?,
                mode,
            ),
            None => (
                String::new(),
                nix::sys::stat::Mode::from_bits_truncate(0o644),
            ),
        };
        let next = with_managed_block(&current, lines);
        if next == current {
            return Ok(false);
        }
        replace_in(&info, "attributes", next.as_bytes(), mode)?;
        Ok(true)
    }

    /// Off Unix there are no no-follow handles, and no contained agents.
    #[cfg(not(unix))]
    pub fn set_managed_attributes(&self, _: &Path, _: &[String]) -> Result<bool, GitError> {
        Err(GitError::InvalidInput {
            what: "attributes",
            reason: "merge attributes need a Unix host".to_owned(),
        })
    }
}

/// An `info/attributes` larger than this is refused rather than read.
#[cfg(unix)]
const MAX_ATTRIBUTES_BYTES: u64 = 256 * 1024;

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

/// One file of [`Git::commit_lines`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitFileLines {
    /// Its path after the commit; a deleted file's before it.
    pub path: String,
    /// Line numbers it added (new file).
    pub added: Vec<u32>,
    /// Line numbers it removed (old file).
    pub removed: Vec<u32>,
}

/// Parses `git show -U0` output into each file's added and removed line numbers.
fn parse_unified_zero(text: &str) -> Vec<CommitFileLines> {
    let mut files: Vec<CommitFileLines> = Vec::new();
    let mut old_path = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            old_path = rest.strip_prefix("a/").unwrap_or(rest).to_owned();
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let path = match rest {
                "/dev/null" => old_path.clone(),
                other => other.strip_prefix("b/").unwrap_or(other).to_owned(),
            };
            files.push(CommitFileLines {
                path,
                ..CommitFileLines::default()
            });
        } else if let Some(rest) = line.strip_prefix("@@ ")
            && let Some(file) = files.last_mut()
        {
            let mut parts = rest.split_whitespace();
            let (Some(old), Some(new)) = (parts.next(), parts.next()) else {
                continue;
            };
            let range = |spec: &str| -> (u32, u32) {
                let spec = spec.trim_start_matches(['-', '+']);
                let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
                (start.parse().unwrap_or(0), count.parse().unwrap_or(0))
            };
            let (old_start, old_count) = range(old);
            let (new_start, new_count) = range(new);
            file.removed.extend(old_start..old_start.saturating_add(old_count));
            file.added.extend(new_start..new_start.saturating_add(new_count));
        }
    }
    files
}

#[cfg(test)]
mod unified_tests {
    use super::*;

    #[test]
    fn unified_zero_hunks_give_line_numbers() {
        let text = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                    @@ -3 +3,2 @@ fn a\n-x\n+y\n+z\n@@ -10,2 +11,0 @@\n-p\n-q\n\
                    diff --git a/old.txt b/old.txt\n--- a/old.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        let files = parse_unified_zero(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[0].added, [3, 4]);
        assert_eq!(files[0].removed, [3, 10, 11]);
        assert_eq!(files[1].path, "old.txt");
        assert_eq!(files[1].removed, [1, 2]);
        assert!(files[1].added.is_empty());
    }
}
