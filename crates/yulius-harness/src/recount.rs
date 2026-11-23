//! Counting an ended run's figures again from its transcript (SPEC-091 RCT-001, RCT-002).
//!
//! A run's lines are its window of the session transcript (see [`LinesSince::between`]) and its
//! sub-agent transcripts over the same window. They are folded with the [`UsageAccumulator`] the
//! live watch uses, so a run recounted later gets the figures a run watched today gets.

use std::fs;
use std::io;
use std::path::Path;

use yulius_domain::time::Timestamp;

use crate::codex_children::CodexChildren;
use crate::observe::{Observed, UsageAccumulator};
use crate::subagents::{LinesSince, SubagentTails};
use crate::tail::{DEFAULT_MAX_LINE_BYTES, TranscriptTail};
use crate::{antigravity, claude, codex};

/// Folds the lines of `transcript` from `since` (the first line when `None`) to `until` (the end
/// of the file when `None`), with its sub-agents' usage over the same window. `None` when the
/// transcript does not exist: a run whose transcript is gone has nothing to count, which is not
/// zero.
///
/// # Errors
/// I/O errors reading the session transcript or its sub-agents' (the count would be partial).
pub fn count_run(
    transcript: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<UsageAccumulator>> {
    count_with(transcript, since, until, claude::observe_versioned, true)
}

/// [`count_run`] for a Codex rollout (SPEC-503), with the responses its session's subagents'
/// rollouts hold over the same window (SPEC-619), as the live watch counts them.
///
/// # Errors
/// I/O errors reading the rollout, listing the sessions folders or reading a subagent's rollout
/// (the count would be partial).
pub fn count_codex_run(
    rollout: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<UsageAccumulator>> {
    let Some(mut acc) = count_with(
        rollout,
        since,
        until,
        |line| (codex::observe_line(line), None),
        false,
    )?
    else {
        return Ok(None);
    };
    let mut children = CodexChildren::between(rollout, since, until);
    loop {
        for observed in children.poll()? {
            acc.observe_child(&observed);
        }
        if children.at_end() {
            return Ok(Some(acc));
        }
    }
}

/// [`count_run`] for an Antigravity transcript (SPEC-505 AGY-004): its own lines only; the CLI
/// keeps its subagents' steps in their own conversations.
///
/// # Errors
/// I/O errors reading the transcript.
pub fn count_antigravity_run(
    transcript: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> io::Result<Option<UsageAccumulator>> {
    count_with(
        transcript,
        since,
        until,
        |line| (antigravity::observe_line(line), None),
        false,
    )
}

fn count_with(
    transcript: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    observe: fn(&str) -> (Vec<Observed>, Option<String>),
    subagents: bool,
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
                let (observed, version) = observe(&line);
                if let Some(version) = version {
                    acc.note_cli_version(&version);
                }
                acc.observe_all(observed.iter());
            }
        }
        if tail.at_end() {
            break;
        }
    }
    if !subagents {
        return Ok(Some(acc));
    }
    let mut side = SubagentTails::between(transcript, since, until);
    // A sub-agent transcript that cannot be read fails the count: partial tokens are not the run's.
    loop {
        for line in side.poll()? {
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
        yulius_domain::time::parse_rfc3339(rfc3339).unwrap_or(Timestamp(0))
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

    /// SPEC-619 AC-1, AC-2: a Codex run counts its session's subagents and their nested
    /// children once each, in the window that holds each response, keeping its own model and
    /// storing the other models' parts; another session in the folder does not count.
    #[test]
    fn a_codex_run_counts_its_subagents_once_by_model() -> Result<(), Box<dyn std::error::Error>> {
        use crate::codex_children::tests::Rollout;
        use yulius_domain::usage::{ModelTokens, TokenUsage};
        const ROOT: &str = "01a1f882-35c2-7413-b525-14d9ee43e464";
        let child = "01a1f900-0000-7000-8000-000000000001";
        let grandchild = "01a1f900-0000-7000-8000-000000000002";
        let dir = tempfile::tempdir()?;
        let day = "2026/11/21";
        let root = Rollout::new(dir.path(), day, "2026-11-21T09:00:00.000Z", ROOT, None)
            .model("2026-11-21T09:00:01.000Z", "gpt-6.2-sol")
            .response("2026-11-21T09:00:02.000Z", "resp_root", 100, 1)
            .response("2026-11-21T10:30:00.000Z", "resp_root_2", 100, 10)
            .write()?;
        Rollout::new(
            dir.path(),
            day,
            "2026-11-21T09:01:00.000Z",
            child,
            Some(ROOT),
        )
        .model("2026-11-21T09:01:01.000Z", "gpt-6.2-astra")
        .response("2026-11-21T09:01:02.000Z", "resp_child", 20, 2)
        .response("2026-11-21T09:01:02.000Z", "resp_child", 20, 2)
        .response("2026-11-21T10:31:00.000Z", "resp_child_2", 20, 20)
        .write()?;
        Rollout::new(
            dir.path(),
            day,
            "2026-11-21T09:02:00.000Z",
            grandchild,
            Some(child),
        )
        .model("2026-11-21T09:02:01.000Z", "gpt-6.2-sol")
        .response("2026-11-21T09:02:02.000Z", "resp_grandchild", 3, 3)
        .write()?;
        let other = "01a1f700-0000-7000-8000-000000000004";
        Rollout::new(dir.path(), day, "2026-11-21T09:00:00.000Z", other, None)
            .session(other)
            .response("2026-11-21T09:00:05.000Z", "resp_other", 1_000, 1_000)
            .write()?;
        let tokens = |input: u64, output: u64| TokenUsage {
            input: Some(input),
            output: Some(output),
            cache_read: Some(input),
            cache_write: Some(0),
        };

        let whole = count_codex_run(&root, None, None)?.ok_or("rollout")?;
        assert_eq!(
            whole.usage(),
            tokens(100 + 100 + 20 + 20 + 3, 1 + 10 + 2 + 20 + 3)
        );
        assert_eq!(whole.model(), Some("gpt-6.2-sol"), "the run's own thread's");
        assert_eq!(
            whole.model_parts(),
            vec![ModelTokens {
                model: "gpt-6.2-astra".into(),
                usage: tokens(40, 22),
            }],
            "the grandchild ran the session's model"
        );

        // Resumed at 10:00, on another subscription: each response counts in its own run.
        let resumed = at("2026-11-21T10:00:00Z");
        let first = count_codex_run(&root, None, Some(resumed))?.ok_or("rollout")?;
        assert_eq!(first.usage(), tokens(100 + 20 + 3, 1 + 2 + 3));
        let second = count_codex_run(&root, Some(resumed), None)?.ok_or("rollout")?;
        assert_eq!(second.usage(), tokens(100 + 20, 10 + 20));
        assert_eq!(
            second.model_parts(),
            vec![ModelTokens {
                model: "gpt-6.2-astra".into(),
                usage: tokens(20, 20),
            }],
            "the child's model line is before the resumed run's window"
        );
        Ok(())
    }

    #[test]
    fn a_sub_agent_transcript_that_cannot_be_read_fails_the_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("s1.jsonl");
        fs::copy(fixture(), &transcript)?;
        fs::create_dir_all(dir.path().join("s1"))?;
        // `subagents` is a file: listing it fails (not "not found"), whoever runs the test.
        fs::write(
            crate::subagents::subagent_dir(&transcript),
            "not a directory",
        )?;
        assert!(
            count_run(&transcript, None, None).is_err(),
            "partial tokens are not a count: the run stays queued"
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
        assert_eq!(acc.usage(), yulius_domain::usage::TokenUsage::UNKNOWN);
        assert_eq!((acc.turns(), acc.active().total_ms()), (0, None));
        Ok(())
    }
}
