//! A task's changes for review (UI-031): the worktree — committed and uncommitted work,
//! untracked files as added — against its merge base with the integration branch, and the
//! branch's own commits. Bounded: large diffs are cut and say so (`truncated`).

use std::io::Read as _;
use std::path::Path;

use yulius_domain::time::Timestamp;

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;

/// Most files shown with their hunks; later files are listed with counts only.
pub const MAX_REVIEW_FILES: usize = 300;
/// Most diff lines kept over all files.
pub const MAX_REVIEW_LINES: usize = 20_000;
/// Most diff lines kept of one file.
pub const MAX_FILE_LINES: usize = 3_000;
/// Largest untracked file read for its lines; larger ones are listed without lines.
pub const MAX_UNTRACKED_BYTES: u64 = 256 * 1024;
/// Most untracked files read and listed; more set `truncated` (an un-ignored `node_modules/`
/// must not make every review read tens of thousands of files, #68).
pub const MAX_UNTRACKED_FILES: usize = 500;
/// Most commits [`Git::branch_commits`] returns.
pub const MAX_REVIEW_COMMITS: usize = 200;

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// New in the task (also untracked files).
    Added,
    /// Changed in place.
    Modified,
    /// Removed.
    Deleted,
    /// Moved (and maybe changed).
    Renamed,
}

/// One line of a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged context.
    Context,
    /// Added line.
    Added,
    /// Deleted line.
    Deleted,
}

/// A line of a hunk with its numbers in the old and the new file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Context, added or deleted.
    pub kind: LineKind,
    /// Line number in the old file (none for an added line).
    pub old: Option<u32>,
    /// Line number in the new file (none for a deleted line).
    pub new: Option<u32>,
    /// The text, without the leading marker and the line end.
    pub text: String,
}

/// One hunk of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// The `@@ -a,b +c,d @@ …` header.
    pub header: String,
    /// Its lines.
    pub lines: Vec<DiffLine>,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path in the new tree (the old path for a deletion).
    pub path: String,
    /// The old path of a rename.
    pub old_path: Option<String>,
    /// How it changed.
    pub status: FileStatus,
    /// Added lines (binary files count zero).
    pub insertions: u64,
    /// Deleted lines.
    pub deletions: u64,
    /// A binary file: no hunks.
    pub binary: bool,
    /// Hunks (empty for binary files and files past the limits).
    pub hunks: Vec<DiffHunk>,
}

/// A task's changes against its merge base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDiff {
    /// The merge base the changes are shown against.
    pub base_sha: String,
    /// The task's `HEAD`.
    pub head_sha: String,
    /// Changed files: tracked ones in Git's order, then untracked ones.
    pub files: Vec<FileDiff>,
    /// Files with uncommitted changes, untracked files included.
    pub uncommitted: u64,
    /// Some hunks or lines were left out to stay within the limits.
    pub truncated: bool,
}

/// One commit of a branch with its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStat {
    /// Full commit id.
    pub sha: String,
    /// First line of the message.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Author date.
    pub at: Timestamp,
    /// Changed files.
    pub files: u64,
    /// Added lines.
    pub insertions: u64,
    /// Deleted lines.
    pub deletions: u64,
}

impl Git {
    /// The worktree's changes against its merge base with `base` (e.g. `origin/dev`): tracked
    /// files as `git diff <merge-base>` shows them against the working tree (committed, staged
    /// and unstaged), then untracked, non-ignored files as added. Symbolic links are listed but
    /// never followed. External diff drivers and text conversion are disabled.
    pub fn worktree_diff(&self, worktree: &Path, base: &str) -> Result<WorktreeDiff, GitError> {
        let head_sha = self.rev_parse(worktree, "HEAD")?;
        let base_sha = self.merge_base(worktree, base, &head_sha)?;
        let mut budget = Budget::default();
        let mut files = match self.run_ok(diff_invocation(self, worktree, &base_sha)) {
            Ok(output) => parse_unified(&output.stdout, &mut budget),
            Err(GitError::OutputTooLarge { .. }) => {
                budget.truncated = true;
                self.numstat_files(worktree, &base_sha)?
            }
            Err(err) => return Err(err),
        };
        let status = self.status_porcelain(worktree)?;
        let uncommitted = status.len() as u64;
        for (n, entry) in status.iter().filter(|e| e.is_untracked()).enumerate() {
            if n >= MAX_UNTRACKED_FILES {
                budget.truncated = true;
                break;
            }
            files.push(untracked_file(worktree, &entry.path, &mut budget));
        }
        Ok(WorktreeDiff {
            base_sha,
            head_sha,
            files,
            uncommitted,
            truncated: budget.truncated,
        })
    }

    /// Commits reachable from `head` but not from `base`, newest first (at most `n`, bounded by
    /// [`MAX_REVIEW_COMMITS`]), each with its changed files and lines.
    pub fn branch_commits(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
        n: usize,
    ) -> Result<Vec<CommitStat>, GitError> {
        let base = self.rev_parse(repo, base)?;
        let head = self.rev_parse(repo, head)?;
        let n = n.min(MAX_REVIEW_COMMITS);
        if n == 0 {
            return Ok(Vec::new());
        }
        let output = self.run_ok(
            Invocation::new(self, "log", Some(repo))
                .args(["log", "--no-show-signature", "--no-color", "--numstat"])
                .arg(format!("--max-count={n}"))
                .arg("--format=%x1e%H%x1f%at%x1f%an%x1f%s")
                .arg("--end-of-options")
                .arg(format!("{base}..{head}"))
                .arg("--"),
        )?;
        parse_commit_stats(&output.stdout)
    }

    /// The best common ancestor of two revisions.
    fn merge_base(&self, repo: &Path, a: &str, b: &str) -> Result<String, GitError> {
        let a = self.rev_parse(repo, a)?;
        let b = self.rev_parse(repo, b)?;
        let output = self.run_ok(
            Invocation::new(self, "merge-base", Some(repo))
                .args(["merge-base", "--end-of-options"])
                .arg(a)
                .arg(b),
        )?;
        let sha = output.stdout_trimmed();
        if sha.is_empty() {
            return Err(GitError::Parse {
                command: "merge-base",
                detail: "no common ancestor".to_owned(),
            });
        }
        Ok(sha)
    }

    /// Files with counts only, for a diff too large to read whole.
    fn numstat_files(&self, worktree: &Path, base: &str) -> Result<Vec<FileDiff>, GitError> {
        let output = self.run_ok(
            Invocation::new(self, "diff", Some(worktree))
                .args([
                    "-c",
                    "core.quotePath=false",
                    "diff",
                    "--numstat",
                    "-z",
                    "-M",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--end-of-options",
                ])
                .arg(base)
                .arg("--"),
        )?;
        let mut files = Vec::new();
        let mut fields = output.stdout.split(|b| *b == 0).peekable();
        while let Some(field) = fields.next() {
            if field.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(field);
            let mut parts = text.splitn(3, '\t');
            let (Some(ins), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let binary = ins == "-" && del == "-";
            let (path, old_path) = if path.is_empty() {
                // A rename: the old and the new path follow as their own fields.
                let old = fields
                    .next()
                    .map(|f| String::from_utf8_lossy(f).into_owned());
                let new = fields
                    .next()
                    .map(|f| String::from_utf8_lossy(f).into_owned());
                (new.unwrap_or_default(), old)
            } else {
                (path.to_owned(), None)
            };
            files.push(FileDiff {
                status: if old_path.is_some() {
                    FileStatus::Renamed
                } else {
                    FileStatus::Modified
                },
                path,
                old_path,
                insertions: ins.parse().unwrap_or(0),
                deletions: del.parse().unwrap_or(0),
                binary,
                hunks: Vec::new(),
            });
        }
        Ok(files)
    }
}

fn diff_invocation<'a>(git: &Git, worktree: &'a Path, base: &str) -> Invocation<'a> {
    Invocation::new(git, "diff", Some(worktree))
        .args([
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            "-U3",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--end-of-options",
        ])
        .arg(base)
        .arg("--")
}

/// What is left of the line and file limits.
#[derive(Debug, Default)]
struct Budget {
    lines: usize,
    files_with_hunks: usize,
    truncated: bool,
}

impl Budget {
    fn file_open(&mut self) -> bool {
        if self.files_with_hunks >= MAX_REVIEW_FILES || self.lines >= MAX_REVIEW_LINES {
            self.truncated = true;
            return false;
        }
        self.files_with_hunks += 1;
        true
    }

    /// Whether one more line of a file that already kept `kept` lines fits.
    fn line_fits(&mut self, kept: usize) -> bool {
        if kept >= MAX_FILE_LINES || self.lines >= MAX_REVIEW_LINES {
            self.truncated = true;
            return false;
        }
        self.lines += 1;
        true
    }
}

/// A path from a `---`/`+++`/`rename` line: `a/x`, `b/x`, `/dev/null` or Git's C-quoted form.
fn diff_path(raw: &str, prefix: &str) -> Option<String> {
    let raw = raw.trim_end_matches('\t');
    if raw == "/dev/null" {
        return None;
    }
    let unquoted = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        unquote(&raw[1..raw.len() - 1])
    } else {
        raw.to_owned()
    };
    Some(
        unquoted
            .strip_prefix(prefix)
            .map_or(unquoted.clone(), str::to_owned),
    )
}

/// Undoes Git's C-style quoting of unusual path characters.
fn unquote(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut chars = text.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'"') => bytes.push(b'"'),
            Some(b'\\') => bytes.push(b'\\'),
            Some(d @ b'0'..=b'7') => {
                let mut value = u32::from(d - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(n @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(n - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(u8::try_from(value & 0xff).unwrap_or(b'?'));
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `@@ -a[,b] +c[,d] @@`: the first old and new line numbers.
fn hunk_start(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let first = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((first(old)?, first(new)?))
}

/// Parses `git diff` output into files, keeping lines within `budget` while counting all.
fn parse_unified(stdout: &[u8], budget: &mut Budget) -> Vec<FileDiff> {
    let text = String::from_utf8_lossy(stdout);
    let mut files: Vec<FileDiff> = Vec::new();
    let mut keep = false;
    // Between a file's `diff --git` line and its first hunk. Tracked explicitly: a file past the
    // budget keeps no hunks, yet its lines must still be counted, never read as headers (#68).
    let mut in_header = false;
    let mut kept = 0usize;
    let (mut old_line, mut new_line) = (0u32, 0u32);
    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // Paths come from the ---/+++ or rename lines; this one is a fallback for files
            // without them (binary or mode-only changes): `a/x b/x`.
            let fallback = rest
                .rfind(" b/")
                .map(|at| rest[at + 3..].to_owned())
                .unwrap_or_default();
            files.push(FileDiff {
                path: fallback,
                old_path: None,
                status: FileStatus::Modified,
                insertions: 0,
                deletions: 0,
                binary: false,
                hunks: Vec::new(),
            });
            keep = budget.file_open();
            in_header = true;
            kept = 0;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if in_header && !line.starts_with("@@") {
            if line.starts_with("new file mode") {
                file.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                file.status = FileStatus::Deleted;
            } else if let Some(from) = line.strip_prefix("rename from ") {
                file.old_path = diff_path(from, "");
                file.status = FileStatus::Renamed;
            } else if let Some(to) = line.strip_prefix("rename to ") {
                if let Some(path) = diff_path(to, "") {
                    file.path = path;
                }
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                file.binary = true;
            } else if let Some(old) = line.strip_prefix("--- ") {
                if let (Some(path), FileStatus::Deleted) = (diff_path(old, "a/"), file.status) {
                    file.path = path;
                }
            } else if let Some(new) = line.strip_prefix("+++ ")
                && let Some(path) = diff_path(new, "b/")
            {
                file.path = path;
            }
            continue;
        }
        if line.starts_with("@@") {
            let Some((old, new)) = hunk_start(line) else {
                continue;
            };
            (old_line, new_line) = (old, new);
            in_header = false;
            if keep {
                file.hunks.push(DiffHunk {
                    header: line.to_owned(),
                    lines: Vec::new(),
                });
            }
            continue;
        }
        let (kind, rest) = match line.as_bytes().first() {
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Deleted, &line[1..]),
            _ => continue, // "\ No newline at end of file" and the final empty line
        };
        let (old, new) = match kind {
            LineKind::Context => (Some(old_line), Some(new_line)),
            LineKind::Added => (None, Some(new_line)),
            LineKind::Deleted => (Some(old_line), None),
        };
        match kind {
            LineKind::Context => {
                old_line += 1;
                new_line += 1;
            }
            LineKind::Added => {
                file.insertions += 1;
                new_line += 1;
            }
            LineKind::Deleted => {
                file.deletions += 1;
                old_line += 1;
            }
        }
        if keep && budget.line_fits(kept) {
            kept += 1;
            if let Some(hunk) = file.hunks.last_mut() {
                hunk.lines.push(DiffLine {
                    kind,
                    old,
                    new,
                    text: rest.trim_end_matches('\r').to_owned(),
                });
            }
        }
    }
    files
}

/// An untracked file as an added one: its lines when it is a small text file; listed without
/// lines when it is binary, large, unreadable or a symbolic link (never followed).
fn untracked_file(worktree: &Path, path: &str, budget: &mut Budget) -> FileDiff {
    let mut file = FileDiff {
        path: path.to_owned(),
        old_path: None,
        status: FileStatus::Added,
        insertions: 0,
        deletions: 0,
        binary: false,
        hunks: Vec::new(),
    };
    let full = worktree.join(path);
    let Ok(meta) = std::fs::symlink_metadata(&full) else {
        return file;
    };
    if !meta.is_file() {
        return file;
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&full)
        .and_then(|f| f.take(MAX_UNTRACKED_BYTES + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return file;
    }
    if bytes.iter().take(8_000).any(|b| *b == 0) {
        file.binary = true;
        return file;
    }
    let too_large = bytes.len() as u64 > MAX_UNTRACKED_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_UNTRACKED_BYTES as usize)])
        .into_owned();
    let lines: Vec<&str> = text.lines().collect();
    file.insertions = lines.len() as u64;
    if too_large || !budget.file_open() {
        budget.truncated = true;
        return file;
    }
    let mut hunk = DiffHunk {
        header: format!("@@ -0,0 +1,{} @@", lines.len()),
        lines: Vec::new(),
    };
    for (i, line) in lines.iter().enumerate() {
        if !budget.line_fits(i) {
            break;
        }
        hunk.lines.push(DiffLine {
            kind: LineKind::Added,
            old: None,
            new: u32::try_from(i + 1).ok(),
            text: (*line).to_owned(),
        });
    }
    file.hunks.push(hunk);
    file
}

/// `log --numstat` records: `RS sha US at US author US subject`, then `ins TAB del TAB path`.
fn parse_commit_stats(stdout: &[u8]) -> Result<Vec<CommitStat>, GitError> {
    let text = String::from_utf8_lossy(stdout);
    let mut commits = Vec::new();
    for record in text.split('\u{1e}').filter(|r| !r.trim().is_empty()) {
        let mut lines = record.lines();
        let head = lines.next().unwrap_or_default();
        let mut fields = head.splitn(4, '\u{1f}');
        let (Some(sha), Some(at), Some(author), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(GitError::Parse {
                command: "log",
                detail: format!("malformed commit record {head:?}"),
            });
        };
        let seconds: i64 = at.trim().parse().map_err(|_| GitError::Parse {
            command: "log",
            detail: format!("bad author date {at:?}"),
        })?;
        let mut stat = CommitStat {
            sha: sha.to_owned(),
            subject: subject.to_owned(),
            author: author.to_owned(),
            at: Timestamp(seconds.saturating_mul(1000)),
            files: 0,
            insertions: 0,
            deletions: 0,
        };
        for line in lines.filter(|l| !l.is_empty()) {
            let mut parts = line.splitn(3, '\t');
            let (Some(ins), Some(del), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            stat.files += 1;
            stat.insertions += ins.parse::<u64>().unwrap_or(0);
            stat.deletions += del.parse::<u64>().unwrap_or(0);
        }
        commits.push(stat);
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift generator for randomized tests (no extra dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        /// Text made of `pieces`, random bytes (lossy UTF-8) and multi-byte characters.
        fn text(&mut self, pieces: &[&str], max_parts: usize) -> String {
            let mut out = String::new();
            for _ in 0..self.below(max_parts + 1) {
                match self.below(10) {
                    0 => {
                        let bytes: Vec<u8> = (0..self.below(8))
                            .map(|_| (self.next() & 0xff) as u8)
                            .collect();
                        out.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    1 => out
                        .push_str(["é", "😀", "\u{0}", "\r", "\t", "ﬀ", "\u{200b}"][self.below(7)]),
                    _ => out.push_str(pieces[self.below(pieces.len())]),
                }
            }
            out
        }
    }

    /// Randomized diffs (#76): the parser never panics, kept lines never exceed the limits,
    /// and a file's kept added lines never exceed its counted insertions.
    #[test]
    fn random_diffs_never_panic_and_stay_consistent() {
        let pieces = [
            "diff --git a/x b/x\n",
            "diff --git a/x\n",
            "--- a/x\n",
            "+++ b/x\n",
            "--- /dev/null\n",
            "+++ /dev/null\n",
            "@@ -1,2 +1,3 @@\n",
            "@@ -0,0 +1 @@ fn\n",
            "@@ garbage\n",
            "@@\n",
            "+added\n",
            "-removed\n",
            " context\n",
            "\\ No newline at end of file\n",
            "new file mode 100644\n",
            "deleted file mode 100644\n",
            "rename from a b\n",
            "rename to \"c\\303\\251\"\n",
            "Binary files a/x and b/x differ\n",
            "GIT binary patch\n",
            "+++ \"b/caf\\303\\251\"\n",
            "\n",
            "+",
            "-",
            " ",
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..4_000 {
            let diff = rng.text(&pieces, 40);
            let mut budget = Budget::default();
            let files = parse_unified(diff.as_bytes(), &mut budget);
            for file in &files {
                let kept_added = file
                    .hunks
                    .iter()
                    .flat_map(|h| &h.lines)
                    .filter(|l| l.kind == LineKind::Added)
                    .count() as u64;
                assert!(kept_added <= file.insertions, "{diff:?}");
                let kept: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
                assert!(kept <= MAX_FILE_LINES, "{diff:?}");
            }
        }
    }

    #[test]
    fn parses_added_modified_deleted_renamed_and_binary_files() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,4 @@ fn main\n keep\n-old\n+new\n+more\n keep2\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\\ No newline at end of file\ndiff --git a/old name.md b/new name.md\nsimilarity index 90%\nrename from old name.md\nrename to new name.md\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
        let mut budget = Budget::default();
        let files = parse_unified(diff.as_bytes(), &mut budget);
        assert_eq!(files.len(), 5);
        let a = &files[0];
        assert_eq!(
            (a.path.as_str(), a.status),
            ("src/a.rs", FileStatus::Modified)
        );
        assert_eq!((a.insertions, a.deletions), (2, 1));
        assert_eq!(a.hunks[0].header, "@@ -1,3 +1,4 @@ fn main");
        let lines = &a.hunks[0].lines;
        assert_eq!(
            lines[0],
            DiffLine {
                kind: LineKind::Context,
                old: Some(1),
                new: Some(1),
                text: "keep".into()
            }
        );
        assert_eq!(
            lines[1],
            DiffLine {
                kind: LineKind::Deleted,
                old: Some(2),
                new: None,
                text: "old".into()
            }
        );
        assert_eq!(
            lines[2],
            DiffLine {
                kind: LineKind::Added,
                old: None,
                new: Some(2),
                text: "new".into()
            }
        );
        assert_eq!(
            lines[4],
            DiffLine {
                kind: LineKind::Context,
                old: Some(3),
                new: Some(4),
                text: "keep2".into()
            }
        );
        assert_eq!(
            (files[1].path.as_str(), files[1].status),
            ("new.txt", FileStatus::Added)
        );
        assert_eq!(
            (files[2].path.as_str(), files[2].status, files[2].deletions),
            ("gone.txt", FileStatus::Deleted, 1)
        );
        assert_eq!(files[3].status, FileStatus::Renamed);
        assert_eq!(
            (files[3].path.as_str(), files[3].old_path.as_deref()),
            ("new name.md", Some("old name.md"))
        );
        assert!(files[4].binary && files[4].hunks.is_empty());
        assert_eq!(files[4].path, "logo.png");
        assert!(!budget.truncated);
    }

    #[test]
    fn long_files_are_cut_and_say_so() {
        let mut diff = String::from(
            "diff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -0,0 +1,5000 @@\n",
        );
        for i in 0..5_000 {
            diff.push_str(&format!("+line {i}\n"));
        }
        let mut budget = Budget::default();
        let files = parse_unified(diff.as_bytes(), &mut budget);
        assert_eq!(files[0].insertions, 5_000, "counts stay exact");
        assert_eq!(files[0].hunks[0].lines.len(), MAX_FILE_LINES);
        assert!(budget.truncated);
    }

    #[test]
    fn files_past_the_budget_keep_exact_counts_and_their_paths() {
        // #68: a file the budget no longer keeps hunks for is still counted, and its content
        // lines (even ones that look like headers) never change its path.
        let diff = "diff --git a/late.txt b/late.txt\n--- a/late.txt\n+++ b/late.txt\n@@ -1,2 +1,3 @@\n keep\n-old\n+++ b/elsewhere\n+new\n";
        let mut budget = Budget {
            files_with_hunks: MAX_REVIEW_FILES,
            ..Budget::default()
        };
        let files = parse_unified(diff.as_bytes(), &mut budget);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "late.txt");
        assert_eq!((files[0].insertions, files[0].deletions), (2, 1));
        assert!(files[0].hunks.is_empty());
        assert!(budget.truncated);
    }

    #[test]
    fn quoted_paths_and_commit_records() -> Result<(), GitError> {
        assert_eq!(
            diff_path("\"b/caf\\303\\251 \\\"x\\\".txt\"", "b/").as_deref(),
            Some("café \"x\".txt")
        );
        assert_eq!(diff_path("/dev/null", "a/"), None);
        let log = "\u{1e}abc\u{1f}1760000000\u{1f}Ann\u{1f}Add checkout\n\n3\t1\tsrc/a.rs\n-\t-\tlogo.png\n\u{1e}def\u{1f}1759990000\u{1f}Bob\u{1f}Spec\n\n10\t0\tdocs/spec.md\n";
        let commits = parse_commit_stats(log.as_bytes())?;
        assert_eq!(commits.len(), 2);
        assert_eq!(
            (
                commits[0].files,
                commits[0].insertions,
                commits[0].deletions
            ),
            (2, 3, 1)
        );
        assert_eq!(commits[0].at, Timestamp(1_760_000_000_000));
        assert_eq!(commits[1].subject, "Spec");
        Ok(())
    }
}
