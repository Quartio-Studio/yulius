//! Working-tree status, diff statistics, ahead/behind counts and history.

use std::path::Path;

use yulius_domain::time::Timestamp;

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;

/// Most commits [`Git::log`] returns.
pub const MAX_LOG_ENTRIES: usize = 1000;

/// One entry of `git status --porcelain=v1 -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Index (staged) status code, e.g. `M`, `A`, `R`, `?`.
    pub index: char,
    /// Worktree status code.
    pub worktree: char,
    /// Path relative to the worktree root (lossy UTF-8).
    pub path: String,
    /// Source path for renames and copies.
    pub original_path: Option<String>,
}

impl StatusEntry {
    /// An untracked, non-ignored file (`??`).
    #[must_use]
    pub fn is_untracked(&self) -> bool {
        self.index == '?' && self.worktree == '?'
    }
}

/// Totals of `git diff --numstat` between the merge base of two commits and the second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStat {
    /// Changed files (a rename counts once).
    pub files: u64,
    /// Inserted lines (binary files count zero).
    pub insertions: u64,
    /// Deleted lines (binary files count zero).
    pub deletions: u64,
}

/// Commits unique to each side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AheadBehind {
    /// Commits reachable from the first revision but not the second.
    pub ahead: u64,
    /// Commits reachable from the second revision but not the first.
    pub behind: u64,
}

/// One commit of [`Git::log`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full commit id.
    pub sha: String,
    /// First line of the message.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Author date.
    pub at: Timestamp,
}

impl Git {
    /// Tracked changes and untracked, non-ignored files (`--untracked-files=all`). Uses
    /// `--no-optional-locks` so it never contends with an agent's own Git commands.
    pub fn status_porcelain(&self, worktree: &Path) -> Result<Vec<StatusEntry>, GitError> {
        let output = self.run_ok(Invocation::new(self, "status", Some(worktree)).args([
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ]))?;
        parse_status(&output.stdout)
    }

    /// Whether the worktree has any tracked change or untracked, non-ignored file.
    pub fn has_changes(&self, worktree: &Path) -> Result<bool, GitError> {
        Ok(!self.status_porcelain(worktree)?.is_empty())
    }

    /// Whether `relative` (a path inside `worktree`) is ignored by Git and holds no tracked
    /// file: removing it loses nothing a commit has.
    pub fn is_ignored_and_untracked(
        &self,
        worktree: &Path,
        relative: &str,
    ) -> Result<bool, GitError> {
        let tracked = self.run_ok(
            Invocation::new(self, "ls-files", Some(worktree))
                .args([
                    "--literal-pathspecs",
                    "--no-optional-locks",
                    "ls-files",
                    "-z",
                    "--",
                ])
                .arg(relative),
        )?;
        if !tracked.stdout.is_empty() {
            return Ok(false);
        }
        let output = self.run(
            Invocation::new(self, "check-ignore", Some(worktree))
                .args(["check-ignore", "-q", "--"])
                .arg(relative),
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(crate::runner::failure("check-ignore", &output)),
        }
    }

    /// Whether the worktree holds files Git ignores (build output, local configuration).
    pub fn has_ignored_files(&self, worktree: &Path) -> Result<bool, GitError> {
        let output = self.run_ok(Invocation::new(self, "ls-files", Some(worktree)).args([
            "--no-optional-locks",
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ]))?;
        Ok(!output.stdout.is_empty())
    }

    /// Diff statistics of `base...head` (from their merge base to `head`, as a pull request
    /// shows them). External diff drivers and text conversion are disabled.
    pub fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> Result<DiffStat, GitError> {
        let base = self.rev_parse(repo, base)?;
        let head = self.rev_parse(repo, head)?;
        let output = self.run_ok(
            Invocation::new(self, "diff", Some(repo))
                .args([
                    "diff",
                    "--numstat",
                    "-z",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--end-of-options",
                ])
                .arg(format!("{base}...{head}"))
                .arg("--"),
        )?;
        parse_numstat(&output.stdout)
    }

    /// Commits unique to `a` (`ahead`) and to `b` (`behind`).
    pub fn ahead_behind(&self, repo: &Path, a: &str, b: &str) -> Result<AheadBehind, GitError> {
        let a = self.rev_parse(repo, a)?;
        let b = self.rev_parse(repo, b)?;
        let output = self.run_ok(
            Invocation::new(self, "rev-list", Some(repo))
                .args(["rev-list", "--left-right", "--count", "--end-of-options"])
                .arg(format!("{a}...{b}"))
                .arg("--"),
        )?;
        let text = output.stdout_trimmed();
        let mut parts = text.split_whitespace().map(str::parse::<u64>);
        match (parts.next(), parts.next(), parts.next()) {
            (Some(Ok(ahead)), Some(Ok(behind)), None) => Ok(AheadBehind { ahead, behind }),
            _ => Err(GitError::Parse {
                command: "rev-list",
                detail: format!("unexpected count output {text:?}"),
            }),
        }
    }

    /// Up to `n` (at most [`MAX_LOG_ENTRIES`]) commits reachable from `rev`, newest first.
    pub fn log(&self, repo: &Path, rev: &str, n: usize) -> Result<Vec<Commit>, GitError> {
        let sha = self.rev_parse(repo, rev)?;
        let n = n.min(MAX_LOG_ENTRIES);
        if n == 0 {
            return Ok(Vec::new());
        }
        let output = self.run_ok(
            Invocation::new(self, "log", Some(repo))
                .args(["log", "-z", "--no-show-signature"])
                .arg(format!("--max-count={n}"))
                .arg("--format=%H%x00%at%x00%an%x00%s")
                .arg("--end-of-options")
                .arg(&sha)
                .arg("--"),
        )?;
        parse_log(&output.stdout)
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn parse_status(stdout: &[u8]) -> Result<Vec<StatusEntry>, GitError> {
    let mut entries = Vec::new();
    let mut fields = stdout.split(|byte| *byte == 0).filter(|f| !f.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            return Err(GitError::Parse {
                command: "status",
                detail: format!("malformed entry {:?}", lossy(field)),
            });
        }
        let index = char::from(field[0]);
        let worktree = char::from(field[1]);
        let path = lossy(&field[3..]);
        let original_path = if matches!(index, 'R' | 'C') {
            Some(lossy(fields.next().ok_or_else(|| GitError::Parse {
                command: "status",
                detail: "rename without source path".to_owned(),
            })?))
        } else {
            None
        };
        entries.push(StatusEntry {
            index,
            worktree,
            path,
            original_path,
        });
    }
    Ok(entries)
}

fn parse_count(field: &[u8]) -> Result<u64, GitError> {
    if field == b"-" {
        return Ok(0);
    }
    std::str::from_utf8(field)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| GitError::Parse {
            command: "diff",
            detail: format!("bad count {:?}", lossy(field)),
        })
}

fn parse_numstat(stdout: &[u8]) -> Result<DiffStat, GitError> {
    let mut stat = DiffStat::default();
    let mut fields = stdout.split(|byte| *byte == 0);
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, |byte| *byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(GitError::Parse {
                command: "diff",
                detail: format!("malformed numstat {:?}", lossy(field)),
            });
        };
        if path.is_empty() {
            // Rename or copy: the old and new paths follow as separate fields.
            let _old = fields.next();
            let _new = fields.next();
        }
        stat.files += 1;
        stat.insertions += parse_count(added)?;
        stat.deletions += parse_count(deleted)?;
    }
    Ok(stat)
}

fn parse_log(stdout: &[u8]) -> Result<Vec<Commit>, GitError> {
    let fields: Vec<&[u8]> = stdout.split(|byte| *byte == 0).collect();
    let mut commits = Vec::new();
    let (records, rest) = fields.as_chunks::<4>();
    for chunk in records {
        let seconds: i64 = std::str::from_utf8(chunk[1])
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .ok_or_else(|| GitError::Parse {
                command: "log",
                detail: format!("bad author time {:?}", lossy(chunk[1])),
            })?;
        commits.push(Commit {
            sha: lossy(chunk[0]).trim().to_owned(),
            at: Timestamp(seconds.saturating_mul(1000)),
            author: lossy(chunk[2]),
            subject: lossy(chunk[3]),
        });
    }
    if rest.iter().any(|field| !field.is_empty()) {
        return Err(GitError::Parse {
            command: "log",
            detail: "trailing partial record".to_owned(),
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_with_renames_and_odd_names() -> Result<(), GitError> {
        let raw = b" M src/lib.rs\0R  new name\0old\nname\0?? we\"ird file\0";
        let entries = parse_status(raw)?;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].original_path.as_deref(), Some("old\nname"));
        assert!(entries[2].is_untracked());
        assert_eq!(entries[2].path, "we\"ird file");
        Ok(())
    }

    #[test]
    fn parses_numstat_with_binary_and_rename() -> Result<(), GitError> {
        let raw = b"3\t1\ta.txt\0-\t-\tlogo.png\x002\t0\t\0old.rs\0new.rs\0";
        assert_eq!(
            parse_numstat(raw)?,
            DiffStat {
                files: 3,
                insertions: 5,
                deletions: 1
            }
        );
        Ok(())
    }
}
