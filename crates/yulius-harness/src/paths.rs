//! Provider transcript locations and session discovery (SPEC-002 §7.2, SPEC-001 HRN-004).
//!
//! Claude Code: `<CLAUDE_CONFIG_DIR>/projects/<cwd-slug>/<sessionId>.jsonl`. Directory walks
//! are bounded by [`MAX_WALK_ENTRIES`] and never follow symbolic links below the given root.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Maximum directory entries examined by one discovery call.
pub const MAX_WALK_ENTRIES: usize = 200_000;

/// Claude's project directory name for a working directory: every character outside
/// `[A-Za-z0-9-]` becomes `-` (for example `/Users/x/a.b` → `-Users-x-a-b`).
///
/// Claude may shorten very long slugs in ways not modelled here (live-unverified).
#[must_use]
pub fn claude_project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `<config_dir>/projects/<slug of cwd>`.
#[must_use]
pub fn claude_project_dir(config_dir: &Path, cwd: &Path) -> PathBuf {
    config_dir.join("projects").join(claude_project_slug(cwd))
}

/// `<config_dir>/projects/<slug of cwd>/<session_id>.jsonl`.
#[must_use]
pub fn claude_transcript_path(config_dir: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    claude_project_dir(config_dir, cwd).join(format!("{session_id}.jsonl"))
}

pub use yulius_domain::harness::valid_session_id;

fn walk_limit_error() -> io::Error {
    io::Error::other("directory walk limit exceeded")
}

/// Entries of `dir` sorted by name descending; `Ok(empty)` when the directory is missing.
fn sorted_entries(dir: &Path, budget: &mut usize) -> io::Result<Vec<fs::DirEntry>> {
    let reader = match fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for entry in reader {
        if *budget == 0 {
            return Err(walk_limit_error());
        }
        *budget -= 1;
        entries.push(entry?);
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    Ok(entries)
}

fn is_real_file(entry: &fs::DirEntry) -> io::Result<bool> {
    Ok(entry.file_type()?.is_file())
}

/// A Claude session transcript file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSessionFile {
    /// Session id (the file stem).
    pub session_id: String,
    /// Transcript path.
    pub path: PathBuf,
    /// Last modification time.
    pub modified: SystemTime,
}

/// The most recently modified main-session transcript in a Claude project directory, modified
/// at or after `since` (session-id drift detection, HRN-004). Subagent transcripts
/// (`agent-*.jsonl`), subdirectories and symbolic links are ignored.
///
/// # Errors
/// An error when more than [`MAX_WALK_ENTRIES`] entries would be examined; other I/O errors
/// except a missing directory.
pub fn newest_claude_session(
    project_dir: &Path,
    since: SystemTime,
) -> io::Result<Option<ClaudeSessionFile>> {
    let mut budget = MAX_WALK_ENTRIES;
    let mut best: Option<ClaudeSessionFile> = None;
    for entry in sorted_entries(project_dir, &mut budget)? {
        if !is_real_file(&entry)? {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(stem) = name.strip_suffix(".jsonl") else {
            continue;
        };
        if stem.starts_with("agent-") || !valid_session_id(stem) {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if modified < since {
            continue;
        }
        let newer = best.as_ref().is_none_or(|b| {
            modified > b.modified || (modified == b.modified && stem > b.session_id.as_str())
        });
        if newer {
            best = Some(ClaudeSessionFile {
                session_id: stem.to_owned(),
                path: entry.path(),
                modified,
            });
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_everything_outside_alnum_and_dash() {
        assert_eq!(
            claude_project_slug(Path::new("/Users/x/a.b")),
            "-Users-x-a-b"
        );
        assert_eq!(
            claude_project_slug(Path::new("/var/lib/yulius/sandboxes/s_1/worktrees/t-2")),
            "-var-lib-yulius-sandboxes-s-1-worktrees-t-2"
        );
        assert_eq!(claude_project_slug(Path::new("/tmp/żółw x")), "-tmp----w-x");
        assert_eq!(
            claude_project_dir(Path::new("/h/.claude"), Path::new("/w")),
            PathBuf::from("/h/.claude/projects/-w")
        );
    }

    #[test]
    fn session_id_validation() {
        assert!(valid_session_id("3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("-rf"));
        assert!(!valid_session_id("../x"));
        assert!(!valid_session_id(&"a".repeat(129)));
    }
}
