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

enum MergeTree {
    Clean(String),
    Conflict(Vec<String>),
}

/// A leading double quote identifies Git's byte-quoted representation. Quote paths
/// that begin with one as well as non-UTF-8 paths, so a displayed name identifies one path.
pub(crate) fn conflict_path(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    if let Ok(path) = std::str::from_utf8(bytes)
        && !path.starts_with('"')
    {
        return path.to_owned();
    }
    let mut quoted = String::from("\"");
    for &byte in bytes {
        match byte {
            b'"' => quoted.push_str(r#"\""#),
            b'\\' => quoted.push_str(r"\\"),
            7 => quoted.push_str(r"\a"),
            8 => quoted.push_str(r"\b"),
            b'\t' => quoted.push_str(r"\t"),
            b'\n' => quoted.push_str(r"\n"),
            11 => quoted.push_str(r"\v"),
            12 => quoted.push_str(r"\f"),
            b'\r' => quoted.push_str(r"\r"),
            b' '..=b'~' => quoted.push(char::from(byte)),
            _ => {
                let _ = write!(quoted, "\\{byte:03o}");
            }
        }
    }
    quoted.push('"');
    quoted
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
        let tree = match self.merged_tree_at(worktree, None, &base, &head)? {
            MergeTree::Clean(tree) => tree,
            MergeTree::Conflict(files) => return Ok(SquashLanding::Conflict(files)),
        };
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
                .author(author),
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
    /// `None` when the merge conflicts. Nothing in the worktree changes. With a `base`, that is
    /// the merge base instead of the one Git finds: what `theirs` changed since `base`, applied
    /// to `ours` (a follow-up after a squash merge, whose own merge base is the branch's old fork
    /// point).
    ///
    /// # Errors
    /// Invalid or unknown revisions, or Git failures (a Git before 2.38, without `merge-tree
    /// --write-tree`, fails here; with a `base`, one before 2.40).
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
        Ok(
            match self.merged_tree_at(worktree, base.as_deref(), &ours, &theirs)? {
                MergeTree::Clean(tree) => Some(tree),
                MergeTree::Conflict(_) => None,
            },
        )
    }

    /// The tree or conflict paths produced by merging `theirs` into `ours`, using the
    /// attributes of `worktree` while leaving its checkout, index and refs untouched.
    ///
    /// # Errors
    /// Invalid or unknown revisions, or Git failures.
    pub fn preview_merge(
        &self,
        worktree: &Path,
        ours: &str,
        theirs: &str,
    ) -> Result<crate::TrainStep, GitError> {
        let ours = self.rev_parse(worktree, ours)?;
        let theirs = self.rev_parse(worktree, theirs)?;
        Ok(match self.merged_tree_at(worktree, None, &ours, &theirs)? {
            MergeTree::Clean(tree) => crate::TrainStep::Clean { tree },
            MergeTree::Conflict(files) => crate::TrainStep::Conflict(files),
        })
    }

    /// Run the same merge for preflight and landing. Revision arguments are resolved object ids.
    fn merged_tree_at(
        &self,
        worktree: &Path,
        base: Option<&str>,
        ours: &str,
        theirs: &str,
    ) -> Result<MergeTree, GitError> {
        let mut invocation = Invocation::new(self, "merge-tree", Some(worktree)).args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            "--no-messages",
        ]);
        if let Some(base) = base {
            invocation = invocation.arg(format!("--merge-base={base}"));
        }
        let output = self.run(invocation.args(["--end-of-options", ours, theirs]))?;
        let mut records = output.stdout.split(|&byte| byte == 0);
        let first = records.next().unwrap_or_default();
        match output.status.code() {
            Some(0) => match std::str::from_utf8(first) {
                Ok(tree) if is_object_id(tree) => Ok(MergeTree::Clean(tree.to_owned())),
                _ => Err(GitError::Parse {
                    command: "merge-tree",
                    detail: format!("not a tree id: {:?}", String::from_utf8_lossy(first)),
                }),
            },
            // --name-only already lists each conflicted path once, before rendering it.
            Some(1) => Ok(MergeTree::Conflict(
                records
                    .filter(|path| !path.is_empty())
                    .map(conflict_path)
                    .collect(),
            )),
            _ => Err(failure("merge-tree", &output)),
        }
    }

    /// Whether `head` is `reviewed` plus only clean merges of the integration branch (SPEC-083
    /// REV-001). Walking first parents from `head` back to `reviewed`, every commit must be a
    /// merge of exactly two parents whose second one `integration` (a remote-tracking ref)
    /// already contains, recording exactly the tree [`Git::merge_tree_from`] gives for its parents:
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
        // The follow-up walk with nothing to count (review round 1 of SPEC-493 fix 6).
        Ok(self
            .follow_up_since(worktree, reviewed, head, integration, limit)?
            .is_some_and(|commits| commits.is_empty()))
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
                    let Some(merged) = self.merge_tree_from(worktree, None, first, second)? else {
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

    /// The lines `commit` (one parent) added and removed against its parent, per file, by line
    /// number (SPEC-493 APR-001). Read with plumbing and `--text`, so no configuration of the
    /// clone and no attribute changes what is counted (review round 1 of fix 6): `diff-tree
    /// --numstat -z` gives each file's exact path and its counts, `diff-tree -p -U0` the line
    /// numbers, and the two must agree (`--numstat` follows a `-diff` attribute and gives no
    /// counts there; the patch does not). A file with a NUL byte or a line over
    /// [`MAX_COUNTED_LINE_BYTES`] in its changed lines is opaque: no line count says what it
    /// changed.
    ///
    /// # Errors
    /// Git failures; a commit with another number of parents than one; a patch that does not
    /// parse or disagrees with the counts.
    pub fn commit_lines(
        &self,
        repo: &Path,
        commit: &str,
    ) -> Result<Vec<CommitFileLines>, GitError> {
        let commit = self.rev_parse(repo, commit)?;
        let parents = self.parents(repo, &commit)?;
        let [parent] = parents.as_slice() else {
            return Err(GitError::InvalidInput {
                what: "commit",
                reason: format!("{commit} has {} parents, not one", parents.len()),
            });
        };
        let diff_tree = |extra: &[&str]| {
            self.run_ok(
                Invocation::new(self, "diff-tree", Some(repo))
                    .env("GIT_ATTR_NOSYSTEM", "1")
                    .args(["-c", "core.quotePath=false", "-c", "diff.orderFile="])
                    .args([
                        "diff-tree",
                        "-r",
                        "--no-renames",
                        "--no-color",
                        "--ignore-submodules=none",
                        "--text",
                    ])
                    .args(extra)
                    .args(["--end-of-options", parent.as_str(), commit.as_str(), "--"]),
            )
        };
        let counts = parse_numstat_z(&diff_tree(&["--numstat", "-z"])?.stdout);
        let patch = diff_tree(&["-p", "-U0", "--src-prefix=a/", "--dst-prefix=b/"])?;
        let disagree = |why: &str| GitError::InvalidInput {
            what: "commit lines",
            reason: format!("what {commit} changed does not read back: {why}"),
        };
        let counts = counts.ok_or_else(|| disagree("its counts do not parse"))?;
        let sections = parse_unified_zero(&patch.stdout)
            .ok_or_else(|| disagree("its patch does not parse"))?;
        // One record per file; a type change (a link become a file) is two sections of the
        // patch, a deletion and an addition, and one record (review round 2 of fix 6).
        let mut sections = sections.into_iter().peekable();
        let mut files = Vec::with_capacity(counts.len());
        let sizes = |s: &Section| (s.added.len(), s.removed.len());
        for (path, numbers) in counts {
            let mut section = sections
                .next()
                .ok_or_else(|| disagree("its patch and its counts name different files"))?;
            if let Some(n) = numbers
                && n != sizes(&section)
                && section.added.is_empty()
                && let Some(next) = sections.next_if(|next| {
                    next.removed.is_empty() && n == (next.added.len(), section.removed.len())
                })
            {
                section.added = next.added;
                section.added_text = next.added_text;
                section.opaque |= next.opaque;
            }
            // Git counts a file its attributes call binary as `-`: the patch (`--text`) says.
            if numbers.is_some_and(|n| n != sizes(&section)) {
                return Err(disagree(&format!("the counts of `{path}` differ")));
            }
            files.push(CommitFileLines {
                path,
                added: section.added,
                added_text: section.added_text,
                removed: section.removed,
                opaque: section.opaque,
            });
        }
        if sections.next().is_some() {
            return Err(disagree("its patch and its counts name different files"));
        }
        Ok(files)
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

/// Longest changed line [`Git::commit_lines`] counts as one line.
pub const MAX_COUNTED_LINE_BYTES: usize = 200;

/// One file's section of a `-U0` patch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Section {
    added: Vec<u32>,
    added_text: Vec<String>,
    removed: Vec<u32>,
    opaque: bool,
}

/// One file of [`Git::commit_lines`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitFileLines {
    /// Its path after the commit; a deleted file's before it.
    pub path: String,
    /// Line numbers it added (new file).
    pub added: Vec<u32>,
    /// The text of each added line (at most [`MAX_COUNTED_LINE_BYTES`] of it), as `added`.
    pub added_text: Vec<String>,
    /// Line numbers it removed (old file).
    pub removed: Vec<u32>,
    /// Whether a changed line holds a NUL byte or is longer than [`MAX_COUNTED_LINE_BYTES`]:
    /// its line count does not say what it changed.
    pub opaque: bool,
}

/// One file of `--numstat`: its path and its added and removed counts (`None`: binary to Git).
type NumStat = (String, Option<(usize, usize)>);

/// `diff-tree --numstat -z` output: each file's path and its added and removed counts
/// (`None` for a binary file); `None` when it does not parse.
fn parse_numstat_z(bytes: &[u8]) -> Option<Vec<NumStat>> {
    let mut out = Vec::new();
    for record in bytes.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let mut parts = text.splitn(3, '\t');
        let (plus, minus, path) = (parts.next()?, parts.next()?, parts.next()?);
        let numbers = match (plus, minus) {
            ("-", "-") => None,
            (plus, minus) => Some((plus.parse().ok()?, minus.parse().ok()?)),
        };
        out.push((path.to_owned(), numbers));
    }
    Some(out)
}

/// `diff-tree -p -U0` output: each file's added and removed line numbers, in order, and whether
/// a changed line is opaque. A file starts only at `diff --git`, and a hunk takes exactly the
/// lines its header counts, so no content line is read as a header (review round 1 of fix 6).
/// `None` when it does not parse.
fn parse_unified_zero(bytes: &[u8]) -> Option<Vec<Section>> {
    let mut files: Vec<Section> = Vec::new();
    // In a hunk: the old and new lines still to take, and their next numbers.
    let mut hunk: Option<(u32, u32, u32, u32)> = None;
    let range = |spec: &str| -> Option<(u32, u32)> {
        let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    for line in bytes.split(|&b| b == b'\n') {
        if let Some((old_left, new_left, old_at, new_at)) = hunk.as_mut() {
            let file = files.last_mut()?;
            match line.first() {
                Some(b'-') if *old_left > 0 => {
                    file.removed.push(*old_at);
                    *old_at += 1;
                    *old_left -= 1;
                }
                Some(b'+') if *new_left > 0 => {
                    file.added.push(*new_at);
                    let text = &line[1..line.len().min(MAX_COUNTED_LINE_BYTES + 1)];
                    file.added_text
                        .push(String::from_utf8_lossy(text).into_owned());
                    *new_at += 1;
                    *new_left -= 1;
                }
                Some(b'\\') => continue,
                _ => return None,
            }
            if line.contains(&0) || line.len() > MAX_COUNTED_LINE_BYTES + 1 {
                file.opaque = true;
            }
            if *old_left == 0 && *new_left == 0 {
                hunk = None;
            }
        } else if line.starts_with(b"diff --git ") {
            files.push(Section::default());
        } else if let Some(rest) = line.strip_prefix(b"@@ -") {
            files.last()?;
            let rest = std::str::from_utf8(rest).ok()?;
            let (old, rest) = rest.split_once(" +")?;
            let (new, _) = rest.split_once(" @@")?;
            let ((old_at, old_left), (new_at, new_left)) = (range(old)?, range(new)?);
            if old_left > 0 || new_left > 0 {
                hunk = Some((old_left, new_left, old_at, new_at));
            }
        }
    }
    hunk.is_none().then_some(files)
}

#[cfg(test)]
mod unified_tests {
    use super::*;

    #[test]
    fn unified_zero_hunks_give_line_numbers() {
        let text = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                    @@ -3 +3,2 @@ fn a\n-x\n+y\n+z\n@@ -10,2 +11,0 @@\n-p\n-q\n\
                    diff --git a/old.txt b/old.txt\n--- a/old.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        let files = parse_unified_zero(text.as_bytes()).unwrap_or_default();
        let lines: Vec<(Vec<u32>, Vec<u32>)> =
            files.into_iter().map(|f| (f.added, f.removed)).collect();
        assert_eq!(lines, [(vec![3, 4], vec![3, 10, 11]), (vec![], vec![1, 2])]);
    }

    /// Review round 1 of SPEC-493 fix 6: a content line that looks like a header stays content,
    /// a missing newline's marker is no line, a new file and a mode change parse, and a hunk
    /// shorter than its header is refused.
    #[test]
    fn a_patch_is_read_by_its_hunk_counts() {
        let text = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n\
                    @@ -1,0 +2,2 @@\n++ b/docs/x.md\n+--- a/docs/x.md\n@@ -9 +10,3 @@\n-old\n\
                    \\ No newline at end of file\n+a\n+b\n+c\n\\ No newline at end of file\n\
                    diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n\
                    @@ -0,0 +1 @@\n+fn n() {}\n\
                    diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        let files = parse_unified_zero(text.as_bytes()).unwrap_or_default();
        assert!(files.iter().all(|f| !f.opaque));
        let lines: Vec<(Vec<u32>, Vec<u32>)> =
            files.into_iter().map(|f| (f.added, f.removed)).collect();
        assert_eq!(
            lines,
            [
                (vec![2, 3, 10, 11, 12], vec![9]),
                (vec![1], vec![]),
                (vec![], vec![])
            ]
        );
        assert!(parse_unified_zero(b"diff --git a/x b/x\n@@ -1,2 +1 @@\n-a\n+b\n").is_none());
        assert!(parse_unified_zero(b"@@ -1 +1 @@\n-a\n+b\n").is_none());
        // A NUL byte or a line over the bound: opaque.
        let long = format!("diff --git a/x b/x\n@@ -0,0 +1 @@\n+{}\n", "x".repeat(201));
        for patch in [
            long.as_bytes(),
            b"diff --git a/x b/x\n@@ -0,0 +1 @@\n+a\0b\n",
        ] {
            let files = parse_unified_zero(patch).unwrap_or_default();
            assert!(files.len() == 1 && files[0].opaque);
        }
        let edge = format!("diff --git a/x b/x\n@@ -0,0 +1 @@\n+{}\n", "x".repeat(200));
        assert!(!parse_unified_zero(edge.as_bytes()).unwrap_or_default()[0].opaque);
        assert_eq!(
            parse_numstat_z(b"3\t1\tsrc/a b.rs\0-\t-\tlogo.png\0"),
            Some(vec![
                ("src/a b.rs".to_owned(), Some((3, 1))),
                ("logo.png".to_owned(), None)
            ])
        );
    }
}
