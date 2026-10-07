//! Transcript tailing contracts: complete lines only, truncation, bounds and cursors
//! (SPEC-002 §7.2 "Transcript tailing", §9 transcript line bound).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use yulius_harness::tail::{DEFAULT_MAX_LINE_BYTES, TranscriptTail};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(bytes)
}

#[test]
fn returns_only_complete_lines_appended_since_last_poll() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    let mut tail = TranscriptTail::open(&path, DEFAULT_MAX_LINE_BYTES)?;
    assert_eq!(
        tail.poll()?,
        Vec::<String>::new(),
        "missing file yields nothing"
    );

    append(&path, b"{\"a\":1}\n{\"b\":")?;
    assert_eq!(tail.poll()?, vec!["{\"a\":1}"]);
    assert!(tail.has_partial_line());
    assert_eq!(tail.offset(), 8, "cursor excludes the partial line");
    assert_eq!(tail.poll()?, Vec::<String>::new());

    append(&path, b"2}\r\n\n  \n{\"c\":3}\n")?;
    assert_eq!(tail.poll()?, vec!["{\"b\":2}", "{\"c\":3}"]);
    assert!(!tail.has_partial_line());
    assert_eq!(tail.offset(), std::fs::metadata(&path)?.len());
    Ok(())
}

#[test]
fn truncated_file_restarts_from_the_beginning() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    append(&path, b"first\nsecond\n")?;
    let mut tail = TranscriptTail::open(&path, 1024)?;
    assert_eq!(tail.poll()?, vec!["first", "second"]);
    File::create(&path)?.write_all(b"new\n")?;
    assert_eq!(tail.poll()?, vec!["new"]);
    assert_eq!(tail.truncations(), 1);
    Ok(())
}

#[test]
fn overlong_lines_are_skipped_and_counted_even_across_polls() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    let mut tail = TranscriptTail::open(&path, 10)?;
    append(&path, b"short\n0123456789ABCDEF\nok\n")?;
    assert_eq!(tail.poll()?, vec!["short", "ok"]);
    assert_eq!(tail.skipped_lines(), 1);

    // A long line arriving in pieces is dropped without buffering it.
    append(&path, b"0123456")?;
    assert_eq!(tail.poll()?, Vec::<String>::new());
    append(&path, b"789ABC")?;
    assert_eq!(tail.poll()?, Vec::<String>::new());
    append(&path, b"DEF\nafter\n")?;
    assert_eq!(tail.poll()?, vec!["after"]);
    assert_eq!(tail.skipped_lines(), 2);
    // Exactly at the bound is accepted.
    append(&path, b"0123456789\n")?;
    assert_eq!(tail.poll()?, vec!["0123456789"]);
    Ok(())
}

#[test]
fn open_at_end_and_resume_cursor() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    append(&path, b"old-1\nold-2\n")?;
    let mut at_end = TranscriptTail::open_at_end(&path, 1024)?;
    append(&path, b"new-1\n")?;
    assert_eq!(at_end.poll()?, vec!["new-1"]);

    let cursor = at_end.offset();
    append(&path, b"new-2\n")?;
    let mut resumed = TranscriptTail::open_at(&path, 1024, cursor)?;
    assert_eq!(resumed.poll()?, vec!["new-2"]);
    assert_eq!(resumed.path(), path.as_path());

    let mut missing = TranscriptTail::open_at_end(dir.path().join("absent.jsonl"), 1024)?;
    assert_eq!(missing.offset(), 0);
    assert_eq!(missing.poll()?, Vec::<String>::new());
    Ok(())
}

#[test]
fn invalid_utf8_is_replaced_and_zero_bound_is_rejected() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    append(&path, b"ok \xff\n")?;
    let mut tail = TranscriptTail::open(&path, 1024)?;
    assert_eq!(tail.poll()?, vec!["ok \u{fffd}"]);
    assert!(TranscriptTail::open(&path, 0).is_err());
    Ok(())
}

#[test]
fn large_appends_are_read_completely_over_polls() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("s.jsonl");
    let line = format!("{}\n", "x".repeat(1000));
    let mut data = String::new();
    for _ in 0..20_000 {
        data.push_str(&line);
    }
    append(&path, data.as_bytes())?;
    let mut tail = TranscriptTail::open(&path, DEFAULT_MAX_LINE_BYTES)?;
    let mut total = 0;
    for _ in 0..4 {
        total += tail.poll()?.len();
    }
    assert_eq!(total, 20_000);
    Ok(())
}
