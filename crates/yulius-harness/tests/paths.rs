//! Transcript location and session discovery contracts (SPEC-002 §7.2, HRN-004).

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use yulius_harness::paths::{claude_project_dir, claude_transcript_path, newest_claude_session};

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
