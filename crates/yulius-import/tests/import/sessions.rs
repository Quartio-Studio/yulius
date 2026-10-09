//! Session discovery filters (IMP-003, AC-IMP-3) over synthetic Claude Code stores.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use yulius_contracts::import::ImportHarness;
use yulius_import::{DiscoveredSession, discover_claude_sessions};

use crate::support::{
    OLD, RECENT, RECENT_LATER, TestResult, claude_line, simple_claude_session, since, write,
    write_claude_session,
};

/// Repository layout: `<temp>/work/demo` with a linked worktree at
/// `<temp>/work/demo-wt` and an unrelated sibling `<temp>/work/demo-other`.
struct Layout {
    temp: tempfile::TempDir,
    repo: PathBuf,
    worktree: PathBuf,
    other: PathBuf,
}

impl Layout {
    fn new() -> TestResult<Self> {
        let temp = tempfile::tempdir()?;
        let work = temp.path().join("work");
        let repo = work.join("demo");
        let worktree = work.join("demo-wt");
        let other = work.join("demo-other");
        for dir in [&repo, &worktree, &other, &repo.join("crates").join("app")] {
            fs::create_dir_all(dir)?;
        }
        Ok(Self {
            repo: fs::canonicalize(repo)?,
            worktree: fs::canonicalize(worktree)?,
            other: fs::canonicalize(other)?,
            temp,
        })
    }

    fn paths(&self) -> Vec<PathBuf> {
        vec![self.repo.clone(), self.worktree.clone()]
    }

    fn store(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }
}

fn ids(sessions: &[DiscoveredSession]) -> Vec<&str> {
    sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect()
}

fn find<'a>(sessions: &'a [DiscoveredSession], id: &str) -> TestResult<&'a DiscoveredSession> {
    sessions
        .iter()
        .find(|session| session.session_id == id)
        .ok_or_else(|| format!("session {id} missing").into())
}

#[test]
fn claude_sessions_inside_repo_and_worktrees_within_the_window() -> TestResult {
    let layout = Layout::new()?;
    let claude = layout.store("claude");
    let repo = &layout.repo;

    // Root session: wrappers before the real prompt, a malformed and an oversized line, subagents.
    let long_prompt = format!(
        "Please refactor the checkout flow {}",
        "and keep tests green ".repeat(10)
    );
    let oversized = format!(
        "{{\"type\":\"user\",\"pad\":\"{}\"}}",
        "x".repeat(4 * 1024 * 1024 + 10)
    );
    let root_path = write_claude_session(
        &claude,
        repo,
        "11111111-root",
        &[
            json!({"type": "summary", "summary": "x", "leafUuid": "u"}).to_string(),
            serde_json::to_string(&json!({
                "type": "user", "cwd": repo.to_string_lossy(), "timestamp": OLD,
                "gitBranch": "main", "isMeta": true,
                "message": {"role": "user", "content": "Caveat: meta text"}
            }))?,
            claude_line(
                "user",
                repo,
                RECENT,
                json!("<command-name>/clear</command-name>"),
            ),
            claude_line(
                "user",
                repo,
                RECENT,
                json!([{"type": "text", "text": "<system-reminder>internal</system-reminder>"}]),
            ),
            "{not json".to_owned(),
            oversized,
            claude_line(
                "user",
                repo,
                RECENT,
                json!([{"type": "tool_result", "content": "x"}, {"type": "text", "text": long_prompt}]),
            ),
            claude_line(
                "assistant",
                repo,
                RECENT_LATER,
                json!([{"type": "text", "text": "done"}]),
            ),
        ],
    )?;
    let subagents = root_path.with_extension("").join("subagents");
    write(
        &subagents.join("agent-a1.jsonl"),
        &(claude_line("user", repo, RECENT, json!("sub")) + "\n"),
    )?;
    write(&subagents.join("notes.txt"), "not a transcript")?;

    // Subdirectory (recorded with the non-canonical temp path when it differs).
    let lexical_subdir = layout
        .temp
        .path()
        .join("work")
        .join("demo")
        .join("crates")
        .join("app");
    simple_claude_session(
        &claude,
        &lexical_subdir,
        "22222222-subdir",
        "Fix app crate",
        RECENT,
        RECENT,
    )?;
    // Linked worktree.
    simple_claude_session(
        &claude,
        &layout.worktree,
        "33333333-worktree",
        "Worktree task",
        RECENT,
        RECENT,
    )?;
    // Started before the window, active inside it.
    simple_claude_session(
        &claude,
        repo,
        "44444444-spanning",
        "Long running",
        OLD,
        RECENT,
    )?;
    // Excluded: sibling repository with a common prefix.
    simple_claude_session(
        &claude,
        &layout.other,
        "55555555-other",
        "Other repo",
        RECENT,
        RECENT,
    )?;
    // Excluded: too old (recent mtime, old timestamps).
    simple_claude_session(&claude, repo, "66666666-old", "Old work", OLD, OLD)?;
    // Excluded: too old by modification time (not even parsed).
    let stale = simple_claude_session(&claude, repo, "77777777-stale", "Stale", RECENT, RECENT)?;
    File::options()
        .write(true)
        .open(&stale)?
        .set_modified(since() - Duration::from_secs(3600))?;
    // Excluded: the first recorded directory decides, even if later lines are inside.
    write_claude_session(
        &claude,
        &layout.other,
        "88888888-moved",
        &[
            claude_line("user", &layout.other, RECENT, json!("start elsewhere")),
            claude_line("user", repo, RECENT, json!("then inside")),
        ],
    )?;
    // Excluded: no recorded directory at all.
    write_claude_session(
        &claude,
        repo,
        "99999999-nocwd",
        &[json!({"type": "summary", "summary": "s"}).to_string()],
    )?;

    let sessions = discover_claude_sessions(&claude, &layout.paths(), since())?;

    assert_eq!(
        ids(&sessions),
        [
            "11111111-root",
            "44444444-spanning",
            "22222222-subdir",
            "33333333-worktree"
        ]
    );
    let root = find(&sessions, "11111111-root")?;
    assert_eq!(root.harness, ImportHarness::Claude);
    assert_eq!(root.title.chars().count(), 80);
    assert!(
        root.title
            .starts_with("Please refactor the checkout flow and keep tests green")
    );
    assert_eq!(root.cwd_relative, None);
    assert_eq!(root.git_branch.as_deref(), Some("main"));
    assert_eq!(root.started_at, "2026-08-01T10:00:00.000Z");
    assert_eq!(root.ended_at, RECENT_LATER);
    assert_eq!(root.skipped_lines, 2);
    assert!(!root.truncated);
    let names: Vec<&str> = root.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "sessions/claude/11111111-root.jsonl",
            "sessions/claude/11111111-root/subagents/agent-a1.jsonl"
        ]
    );
    assert_eq!(root.files[0].size, fs::metadata(&root_path)?.len());
    assert_eq!(
        root.total_size(),
        root.files.iter().map(|file| file.size).sum::<u64>()
    );

    let subdir = find(&sessions, "22222222-subdir")?;
    assert_eq!(subdir.cwd_relative.as_deref(), Some("crates/app"));
    assert_eq!(subdir.title, "Fix app crate");
    assert_eq!(find(&sessions, "33333333-worktree")?.cwd_relative, None);
    Ok(())
}

#[test]
fn missing_stores_yield_no_sessions() -> TestResult {
    let layout = Layout::new()?;
    let nowhere = layout.store("does-not-exist");
    assert!(discover_claude_sessions(&nowhere, &layout.paths(), since())?.is_empty());
    Ok(())
}

#[test]
fn duplicate_session_ids_keep_the_latest_copy() -> TestResult {
    let layout = Layout::new()?;
    let claude = layout.store("claude");
    simple_claude_session(&claude, &layout.repo, "dup-1", "Early copy", RECENT, RECENT)?;
    simple_claude_session(
        &claude,
        &layout.repo.join("crates"),
        "dup-1",
        "Later copy",
        RECENT,
        RECENT_LATER,
    )?;
    let sessions = discover_claude_sessions(&claude, &layout.paths(), since())?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "Later copy");
    Ok(())
}

#[test]
fn large_transcripts_stop_at_the_scan_bound_but_keep_the_final_timestamp() -> TestResult {
    let layout = Layout::new()?;
    let claude = layout.store("claude");
    let repo: &Path = &layout.repo;
    let filler = "y".repeat(1024 * 1024);
    let mut lines = vec![claude_line("user", repo, OLD, json!("Big session"))];
    for _ in 0..66 {
        lines.push(claude_line(
            "assistant",
            repo,
            OLD,
            json!([{"type": "text", "text": filler}]),
        ));
    }
    lines.push(claude_line(
        "assistant",
        repo,
        RECENT_LATER,
        json!([{"type": "text", "text": "end"}]),
    ));
    write_claude_session(&claude, repo, "big-1", &lines)?;
    let sessions = discover_claude_sessions(&claude, &layout.paths(), since())?;
    let big = find(&sessions, "big-1")?;
    assert!(big.truncated);
    assert_eq!(big.ended_at, RECENT_LATER);
    assert_eq!(big.title, "Big session");
    Ok(())
}
