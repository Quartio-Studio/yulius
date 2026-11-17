//! Transcript location and session discovery contracts (SPEC-002 §7.2, HRN-004).

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use yulius_harness::paths::{
    antigravity_last_conversation, antigravity_transcript_path, claude_project_dir,
    claude_transcript_path, find_codex_rollout, newest_claude_session, recent_codex_rollouts,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const T0: Duration = Duration::from_secs(1_791_364_320);

fn touch(path: &Path, seconds_after_t0: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    file.set_modified(SystemTime::UNIX_EPOCH + T0 + Duration::from_secs(seconds_after_t0))
}

fn at(seconds_after_t0: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + T0 + Duration::from_secs(seconds_after_t0)
}

#[test]
fn claude_paths_follow_the_slug_rule() {
    assert_eq!(
        claude_project_dir(Path::new("/h/.claude"), Path::new("/Users/x/a.b")),
        Path::new("/h/.claude/projects/-Users-x-a-b")
    );
    assert_eq!(
        claude_transcript_path(Path::new("/h/.claude"), Path::new("/w/t_1"), "abc"),
        Path::new("/h/.claude/projects/-w-t-1/abc.jsonl")
    );
}

#[test]
fn newest_claude_session_detects_drift_and_ignores_noise() -> TestResult {
    let dir = tempfile::tempdir()?;
    let project = dir.path().join("projects/-w");
    let original = "3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab";
    let drifted = "7c1d2e3f-bbbb-4ccc-8ddd-0123456789ab";
    touch(&project.join(format!("{original}.jsonl")), 10)?;
    touch(&project.join(format!("{drifted}.jsonl")), 20)?;
    touch(&project.join("agent-1234abcd.jsonl"), 30)?;
    touch(&project.join("notes.txt"), 40)?;
    fs::create_dir_all(project.join(format!("{original}/subagents")))?;
    let found = newest_claude_session(&project, at(5))?.ok_or("no session found")?;
    assert_eq!(found.session_id, drifted);
    assert_eq!(found.path, project.join(format!("{drifted}.jsonl")));
    assert_eq!(found.modified, at(20));
    assert_eq!(newest_claude_session(&project, at(25))?, None);
    assert_eq!(
        newest_claude_session(&dir.path().join("absent"), at(0))?,
        None
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn newest_claude_session_ignores_symlinks() -> TestResult {
    let dir = tempfile::tempdir()?;
    let project = dir.path().join("projects/-w");
    fs::create_dir_all(&project)?;
    let elsewhere = dir.path().join("elsewhere.jsonl");
    touch(&elsewhere, 50)?;
    std::os::unix::fs::symlink(
        &elsewhere,
        project.join("3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab.jsonl"),
    )?;
    assert_eq!(newest_claude_session(&project, at(0))?, None);
    Ok(())
}

/// SPEC-503 CDX-003: a Codex session's rollout is found by its id.
#[test]
fn finds_codex_rollout_by_session_id() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sessions = dir.path().join("sessions");
    let wanted = "0199c1d2-3e4f-7a8b-9c0d-1e2f3a4b5c6d";
    touch(
        &sessions.join(
            "2026/10/06/rollout-2026-10-06T10-00-00-0199aaaa-0000-7000-8000-000000000000.jsonl",
        ),
        0,
    )?;
    let target = sessions.join(format!(
        "2026/10/07/rollout-2026-10-07T09-00-00-{wanted}.jsonl"
    ));
    touch(&target, 1)?;
    touch(
        &sessions.join(format!("2026/10/07/notes-{wanted}.jsonl")),
        2,
    )?;
    assert_eq!(find_codex_rollout(&sessions, wanted)?, Some(target));
    assert_eq!(
        find_codex_rollout(&sessions, "0199bbbb-0000-7000-8000-000000000000")?,
        None
    );
    assert_eq!(
        find_codex_rollout(&dir.path().join("absent"), wanted)?,
        None
    );
    assert!(find_codex_rollout(&sessions, "../../etc/passwd").is_err());
    assert!(find_codex_rollout(&sessions, "").is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn codex_walk_does_not_follow_symlinked_directories() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sessions = dir.path().join("sessions");
    let outside = dir.path().join("outside/10/07");
    let id = "0199c1d2-3e4f-7a8b-9c0d-1e2f3a4b5c6d";
    touch(
        &outside.join(format!("rollout-2026-10-07T09-00-00-{id}.jsonl")),
        0,
    )?;
    fs::create_dir_all(&sessions)?;
    std::os::unix::fs::symlink(dir.path().join("outside"), sessions.join("2026"))?;
    assert_eq!(find_codex_rollout(&sessions, id)?, None);
    Ok(())
}

#[test]
fn recent_codex_rollouts_are_newest_first_and_filtered_by_time() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sessions = dir.path().join("sessions");
    let old = sessions.join("2026/10/06/rollout-2026-10-06T10-00-00-a.jsonl");
    let newer = sessions.join("2026/10/07/rollout-2026-10-07T09-00-00-b.jsonl");
    let newest = sessions.join("2026/10/07/rollout-2026-10-07T09-05-00-c.jsonl");
    touch(&old, 0)?;
    touch(&newer, 100)?;
    touch(&newest, 200)?;
    touch(&sessions.join("2026/10/07/other.txt"), 300)?;
    assert_eq!(
        recent_codex_rollouts(&sessions, at(50))?,
        vec![newest, newer]
    );
    assert!(recent_codex_rollouts(&dir.path().join("absent"), at(0))?.is_empty());
    Ok(())
}

/// SPEC-505 AGY-004: a conversation's transcript lies in its brain folder.
#[test]
fn antigravity_transcripts_lie_in_the_brain_folder() {
    assert_eq!(
        antigravity_transcript_path(Path::new("/s/antigravity/shared/brain"), "c-1"),
        Path::new("/s/antigravity/shared/brain/c-1/.system_generated/logs/transcript.jsonl")
    );
}

/// SPEC-505 AGY-005: the CLI's last conversation of a worktree, from `last_conversations.json`;
/// nothing for another worktree, an unsafe id, a broken, linked or oversized file.
#[test]
fn the_last_conversation_of_a_worktree() -> TestResult {
    let dir = tempfile::tempdir()?;
    let gemini = dir.path().join(".gemini");
    let cache = gemini.join("antigravity-cli/cache");
    fs::create_dir_all(&cache)?;
    let file = cache.join("last_conversations.json");
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    fs::write(
        &file,
        r#"{"/w/t1": "0bb76935-ee61-4895-9082-1aea3f3976ee", "/w/t2": "../x"}"#,
    )?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")).as_deref(),
        Some("0bb76935-ee61-4895-9082-1aea3f3976ee")
    );
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t2")),
        None
    );
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t3")),
        None
    );
    fs::write(&file, "{broken")?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    fs::write(&file, vec![b' '; 2 * 1024 * 1024])?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    fs::remove_file(&file)?;
    let elsewhere = dir.path().join("elsewhere.json");
    fs::write(&elsewhere, r#"{"/w/t1": "abc"}"#)?;
    std::os::unix::fs::symlink(&elsewhere, &file)?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    // A FIFO in its place is not waited on.
    fs::remove_file(&file)?;
    nix::unistd::mkfifo(&file, nix::sys::stat::Mode::S_IRWXU)?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    // A folder on the way that is a link (the agent owns the home) is not followed.
    let other = dir.path().join("other/antigravity-cli/cache");
    fs::create_dir_all(&other)?;
    fs::write(other.join("last_conversations.json"), r#"{"/w/t1": "abc"}"#)?;
    fs::remove_dir_all(gemini.join("antigravity-cli"))?;
    std::os::unix::fs::symlink(
        dir.path().join("other/antigravity-cli"),
        gemini.join("antigravity-cli"),
    )?;
    assert_eq!(
        antigravity_last_conversation(&gemini, Path::new("/w/t1")),
        None
    );
    Ok(())
}
