//! Counting an ended run's figures again from its transcript (SPEC-091 RCT-001, RCT-002).
//!
//! A run's lines are its window of the session transcript (see [`LinesSince::between`]) and its
//! sub-agent transcripts over the same window. They are folded with the [`UsageAccumulator`] the
//! live watch uses, so a run recounted later gets the figures a run watched today gets.

use std::fs;
use std::io;
use std::path::Path;

use yulius_domain::time::Timestamp;

use crate::claude;
use crate::observe::UsageAccumulator;
use crate::subagents::{LinesSince, SubagentTails};
use crate::tail::{DEFAULT_MAX_LINE_BYTES, TranscriptTail};

/// Folds the lines of `transcript` from `since` (the first line when `None`) to `until` (the end
/// of the file when `None`), with its sub-agents' usage over the same window. `None` when the
/// transcript does not exist: a run whose transcript is gone has nothing to count, which is not
/// zero. A sub-agent transcript that cannot be read leaves the session's own figures.
///
/// # Errors
/// I/O errors reading the session transcript.
pub fn count_run(
    transcript: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<UsageAccumulator>> {
    match fs::symlink_metadata(transcript) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut acc = UsageAccumulator::new();
    let mut tail = TranscriptTail::open(transcript, DEFAULT_MAX_LINE_BYTES)?;
    let mut window = LinesSince::between(since, until);
    // Each poll reads at most `MAX_POLL_BYTES`; the window's end stops the read early.
    while !window.ended() {
        for line in tail.poll()? {
            if window.keep(&line) {
                acc.observe_all(claude::observe_line(&line).iter());
            }
        }
        if tail.at_end() {
            break;
        }
    }
    let mut side = SubagentTails::between(transcript, since, until);
    // A sub-agent transcript that cannot be read ends the sub-agents' count there.
    while let Ok(lines) = side.poll() {
        for line in lines {
            for observed in claude::observe_line(&line) {
                acc.observe_side(&observed);
            }
        }
        if side.at_end() {
            break;
        }
    }
    Ok(Some(acc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn at(rfc3339: &str) -> Timestamp {
        crate::time::parse_rfc3339(rfc3339).unwrap_or(Timestamp(0))
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/resumed.jsonl")
    }

    /// The fixture (`tests/fixtures/claude/resumed.jsonl`, Claude Code 2.1.292 shapes): a session
    /// whose first run (09:00–09:30) had a goal continuation, a turn that waited for a background
    /// agent and an interrupted turn; a second run resumed it on another subscription
    /// (10:00–10:05) with a sub-agent; then a person's terminal resumed it at 11:00.
    #[test]
    fn each_run_of_a_resumed_session_counts_its_own_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let first =
            count_run(&fixture(), None, Some(at("2026-10-10T09:30:00Z")))?.ok_or("transcript")?;
        let usage = first.usage();
        // Its own answers (310) and its background agent's (1,200); not the later runs'.
        assert_eq!((usage.output, usage.input), (Some(1_510), Some(85)));
        assert_eq!(
            first.turns(),
            3,
            "Claude Code's turn records, not its 7 answers"
        );
        assert_eq!(
            first.active().total_ms(),
            Some(60_100 + 130_100 + 30_000),
            "the union of the turns' spans, the wait for the background agent and the \
             interrupted turn included"
        );
        assert_eq!(first.model(), Some("claude-opus-5-5"));

        let second = count_run(
            &fixture(),
            Some(at("2026-10-10T10:00:00Z")),
            Some(at("2026-10-10T10:05:00Z")),
        )?
        .ok_or("transcript")?;
        let usage = second.usage();
        assert_eq!(
            (
                usage.output,
                usage.input,
                usage.cache_read,
                usage.cache_write
            ),
            (Some(400), Some(44), Some(7_000), Some(60)),
            "its lines and its sub-agent's, not the first run's or the terminal's"
        );
        assert_eq!(second.turns(), 1);
        assert_eq!(second.active().total_ms(), Some(39_100));

        let to_the_end =
            count_run(&fixture(), Some(at("2026-10-10T10:00:00Z")), None)?.ok_or("transcript")?;
        assert_eq!(
            (to_the_end.usage().output, to_the_end.turns()),
            (Some(1_399), 2),
            "without an end the terminal's turn would count"
        );
        Ok(())
    }

    #[test]
    fn a_missing_transcript_is_not_zero() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        assert!(count_run(&dir.path().join("gone.jsonl"), None, None)?.is_none());
        assert!(count_run(dir.path(), None, None)?.is_none(), "a directory");
        let empty = dir.path().join("empty.jsonl");
        fs::write(&empty, "")?;
        let acc = count_run(&empty, None, None)?.ok_or("an empty file is a transcript")?;
        assert!(acc.usage().is_unknown());
        assert_eq!((acc.turns(), acc.active().total_ms()), (0, None));
        Ok(())
    }
}
