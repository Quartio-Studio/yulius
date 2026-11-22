//! Provider transcript locations and session discovery (SPEC-002 §7.2, SPEC-001 HRN-004).
//!
//! Claude Code: `<CLAUDE_CONFIG_DIR>/projects/<cwd-slug>/<sessionId>.jsonl`. Codex (SPEC-503):
//! `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<timestamp>-<sessionId>.jsonl`. Antigravity
//! (SPEC-505): `<HOME>/.gemini/antigravity-cli/brain/<conversation>/.system_generated/logs/
//! transcript.jsonl`. Directory walks are
//! bounded by [`MAX_WALK_ENTRIES`] and never follow symbolic links below the given root.

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

/// The Antigravity CLI's folder of conversations below its `.gemini` folder (1.5.10).
pub const ANTIGRAVITY_BRAIN: &str = "antigravity-cli/brain";
/// Its folder of conversation databases below `.gemini`.
pub const ANTIGRAVITY_CONVERSATIONS: &str = "antigravity-cli/conversations";
/// The file naming each workspace's last conversation, below `.gemini`.
const ANTIGRAVITY_LAST_CONVERSATIONS: &str = "antigravity-cli/cache/last_conversations.json";
/// The largest `last_conversations.json` read.
const ANTIGRAVITY_LAST_CONVERSATIONS_MAX: u64 = 1024 * 1024;

/// `<brain>/<conversation>/.system_generated/logs/transcript.jsonl` (SPEC-505 AGY-004).
#[must_use]
pub fn antigravity_transcript_path(brain: &Path, conversation: &str) -> PathBuf {
    brain
        .join(conversation)
        .join(".system_generated/logs/transcript.jsonl")
}

/// The conversation the Antigravity CLI last ran in `worktree`, from its
/// `cache/last_conversations.json` below `gemini_dir` (SPEC-505 AGY-005): `None` without one, for
/// an id Yulius would not pass on, or for a file larger than 1 MiB or not a JSON object. The
/// agent owns the folders below `gemini_dir`: each is opened from the one above it without
/// following a link, and the file is opened the same way and not waited on when it is no plain
/// file (a FIFO), so nothing is read but the file named.
#[must_use]
pub fn antigravity_last_conversation(gemini_dir: &Path, worktree: &Path) -> Option<String> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    use std::io::Read as _;
    let dir_flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut dir = open(gemini_dir, dir_flags, Mode::empty()).ok()?;
    let mut names = Path::new(ANTIGRAVITY_LAST_CONVERSATIONS)
        .components()
        .peekable();
    let mut file_name = None;
    while let Some(component) = names.next() {
        let name = component.as_os_str();
        if names.peek().is_none() {
            file_name = Some(name);
        } else {
            dir = openat(&dir, name, dir_flags, Mode::empty()).ok()?;
        }
    }
    let file = openat(
        &dir,
        file_name?,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let file = fs::File::from(file);
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > ANTIGRAVITY_LAST_CONVERSATIONS_MAX {
        return None;
    }
    let mut text = String::new();
    file.take(ANTIGRAVITY_LAST_CONVERSATIONS_MAX)
        .read_to_string(&mut text)
        .ok()?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).ok()?;
    map.get(worktree.to_str()?)?
        .as_str()
        .filter(|id| valid_session_id(id))
        .map(str::to_owned)
}

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

fn is_real_dir(entry: &fs::DirEntry) -> io::Result<bool> {
    Ok(entry.file_type()?.is_dir())
}

/// Calls `visit` for every regular file in `<sessions_dir>/*/*/*/` until it returns `Some`.
fn walk_rollouts<T>(
    sessions_dir: &Path,
    mut visit: impl FnMut(&fs::DirEntry) -> io::Result<Option<T>>,
) -> io::Result<Option<T>> {
    let mut budget = MAX_WALK_ENTRIES;
    for year in sorted_entries(sessions_dir, &mut budget)? {
        if !is_real_dir(&year)? {
            continue;
        }
        for month in sorted_entries(&year.path(), &mut budget)? {
            if !is_real_dir(&month)? {
                continue;
            }
            for day in sorted_entries(&month.path(), &mut budget)? {
                if !is_real_dir(&day)? {
                    continue;
                }
                for file in sorted_entries(&day.path(), &mut budget)? {
                    if is_real_file(&file)?
                        && let Some(found) = visit(&file)?
                    {
                        return Ok(Some(found));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// A date folder of a Codex sessions folder, `YYYY/MM/DD`, as numbers.
pub type RolloutDay = (u32, u32, u32);

/// The date folder a rollout `<sessions>/YYYY/MM/DD/rollout-….jsonl` is in.
#[must_use]
pub fn rollout_day(rollout: &Path) -> Option<RolloutDay> {
    let day = rollout.parent()?;
    let month = day.parent()?;
    let year = month.parent()?;
    let number = |dir: &Path| dir.file_name()?.to_str()?.parse::<u32>().ok();
    Some((number(year)?, number(month)?, number(day)?))
}

/// Calls `visit` for every rollout file in the date folders of `sessions_dir` from `first` on
/// (SPEC-619 CSA-6): earlier years, months and days are not listed. Links are not followed.
///
/// # Errors
/// An error when more than [`MAX_WALK_ENTRIES`] entries would be examined; other I/O errors
/// except a missing directory.
pub fn walk_codex_days(
    sessions_dir: &Path,
    first: RolloutDay,
    mut visit: impl FnMut(&fs::DirEntry) -> io::Result<()>,
) -> io::Result<()> {
    let number = |entry: &fs::DirEntry| entry.file_name().to_str()?.parse::<u32>().ok();
    let mut budget = MAX_WALK_ENTRIES;
    for year in sorted_entries(sessions_dir, &mut budget)? {
        let Some(y) = number(&year).filter(|y| *y >= first.0) else {
            continue;
        };
        if !is_real_dir(&year)? {
            continue;
        }
        for month in sorted_entries(&year.path(), &mut budget)? {
            let Some(m) = number(&month).filter(|m| (y, *m) >= (first.0, first.1)) else {
                continue;
            };
            if !is_real_dir(&month)? {
                continue;
            }
            for day in sorted_entries(&month.path(), &mut budget)? {
                if number(&day).is_none_or(|d| (y, m, d) < first) || !is_real_dir(&day)? {
                    continue;
                }
                for file in sorted_entries(&day.path(), &mut budget)? {
                    if is_rollout_name(&file.file_name().to_string_lossy()) && is_real_file(&file)?
                    {
                        visit(&file)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_rollout_name(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

/// Finds the rollout file of a Codex session under `<CODEX_HOME>/sessions`.
///
/// # Errors
/// `InvalidInput` for an invalid session id; an error when more than [`MAX_WALK_ENTRIES`]
/// entries would be examined; other I/O errors except a missing directory.
pub fn find_codex_rollout(sessions_dir: &Path, session_id: &str) -> io::Result<Option<PathBuf>> {
    if !valid_session_id(session_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session id",
        ));
    }
    let suffix = format!("-{session_id}.jsonl");
    walk_rollouts(sessions_dir, |entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        Ok((is_rollout_name(&name) && name.ends_with(&suffix)).then(|| entry.path()))
    })
}

/// Codex rollout files modified at or after `since`, newest first (to discover the id of a new
/// session from its `session_meta` when no hook named it).
///
/// # Errors
/// An error when more than [`MAX_WALK_ENTRIES`] entries would be examined; other I/O errors
/// except a missing directory.
pub fn recent_codex_rollouts(sessions_dir: &Path, since: SystemTime) -> io::Result<Vec<PathBuf>> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    walk_rollouts::<()>(sessions_dir, |entry| {
        let name = entry.file_name();
        if is_rollout_name(&name.to_string_lossy()) {
            let modified = entry.metadata()?.modified()?;
            if modified >= since {
                found.push((modified, entry.path()));
            }
        }
        Ok(None)
    })?;
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    Ok(found.into_iter().map(|(_, path)| path).collect())
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
/// at or after `since` (the cockpit's session; a task's session is named by its hooks, SPEC-281,
/// since any `claude` in the folder writes here). Subagent transcripts
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
