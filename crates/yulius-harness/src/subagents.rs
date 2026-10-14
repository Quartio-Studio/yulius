//! A session's sub-agent transcripts and the lines of a transcript from a given time
//! (SPEC-087 MET-002, MET-004).
//!
//! Claude Code 2.1.292 writes each sub-agent's conversation (the Task/Agent tool, background
//! agents) to `<session>/subagents/agent-<id>.jsonl` next to the session's `<session>.jsonl`.
//! Every line there is the sub-agent's own (`isSidechain`), and none of its messages is in the
//! session's transcript, so the session's token usage is its transcript's plus these files'.
//! The directory is listed and its files followed with bounds: at most
//! [`MAX_SUBAGENT_FILES`] files, regular files only (no symbolic links), lines of at most
//! [`SUBAGENT_LINE_BYTES`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use yulius_domain::time::Timestamp;

use crate::claude;
use crate::tail::TranscriptTail;

/// Most sub-agent transcripts followed for one session.
pub const MAX_SUBAGENT_FILES: usize = 256;

/// Longest sub-agent transcript line read; longer ones (large tool output) are skipped.
pub const SUBAGENT_LINE_BYTES: usize = 1024 * 1024;

/// `<session>/subagents` for the session transcript `<session>.jsonl`.
#[must_use]
pub fn subagent_dir(transcript: &Path) -> PathBuf {
    transcript.with_extension("").join("subagents")
}

/// Keeps a transcript's lines from the first one recorded at or after a time: a resumed
/// session's file holds its earlier runs' lines before the current run's. Lines without a time
/// before that one are dropped too; once a line is kept, every later line is, up to the window's
/// end when it has one: the first line recorded at or after it and every line after that are
/// dropped (a later run's, SPEC-091 RCT-001).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinesSince {
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    ended: bool,
}

impl LinesSince {
    /// Keeps the lines from the first one at or after `since`; every line with `None`.
    #[must_use]
    pub fn new(since: Option<Timestamp>) -> Self {
        Self::between(since, None)
    }

    /// Keeps the lines from the first one at or after `since` (the first line with `None`) to
    /// the last one before the first line at or after `until` (to the end with `None`).
    #[must_use]
    pub fn between(since: Option<Timestamp>, until: Option<Timestamp>) -> Self {
        Self {
            since,
            until,
            ended: false,
        }
    }

    /// Whether `line` is kept.
    pub fn keep(&mut self, line: &str) -> bool {
        if self.ended {
            return false;
        }
        if self.since.is_none() && self.until.is_none() {
            return true;
        }
        let at = claude::record_time(line);
        if let (Some(until), Some(at)) = (self.until, at)
            && at >= until
        {
            self.ended = true;
            return false;
        }
        let Some(since) = self.since else {
            return true;
        };
        if at.is_some_and(|at| at >= since) {
            self.since = None;
            return true;
        }
        false
    }

    /// Whether lines are still being dropped before the window.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.since.is_some()
    }

    /// Whether the window's end was read: no later line is kept.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.ended
    }
}

/// Follows the sub-agent transcripts of one session.
#[derive(Debug)]
pub struct SubagentTails {
    dir: PathBuf,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    tails: BTreeMap<PathBuf, (TranscriptTail, LinesSince)>,
}

impl SubagentTails {
    /// Follows the sub-agents of the session transcript `transcript`, counting their lines from
    /// `since` (`None`: all of them).
    #[must_use]
    pub fn new(transcript: &Path, since: Option<Timestamp>) -> Self {
        Self::between(transcript, since, None)
    }

    /// Follows them over a run's window: [`LinesSince::between`] `since` and `until` in each file.
    #[must_use]
    pub fn between(transcript: &Path, since: Option<Timestamp>, until: Option<Timestamp>) -> Self {
        Self {
            dir: subagent_dir(transcript),
            since,
            until,
            tails: BTreeMap::new(),
        }
    }

    /// The new lines of every sub-agent transcript: the directory is listed again, and files
    /// that appeared are read from their start.
    ///
    /// # Errors
    /// I/O failures other than a missing directory or file.
    pub fn poll(&mut self) -> io::Result<Vec<String>> {
        self.discover()?;
        let mut lines = Vec::new();
        for (tail, since) in self.tails.values_mut() {
            for line in tail.poll()? {
                if since.keep(&line) {
                    lines.push(line);
                }
            }
        }
        Ok(lines)
    }

    /// Whether the last poll read every followed file to its end or its window's end.
    #[must_use]
    pub fn at_end(&self) -> bool {
        self.tails
            .values()
            .all(|(tail, window)| tail.at_end() || window.ended())
    }

    /// Number of sub-agent transcripts followed.
    #[must_use]
    pub fn files(&self) -> usize {
        self.tails.len()
    }

    fn discover(&mut self) -> io::Result<()> {
        if self.tails.len() >= MAX_SUBAGENT_FILES {
            return Ok(());
        }
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for entry in entries.take(MAX_SUBAGENT_FILES * 4) {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl")
                || self.tails.contains_key(&path)
                || !entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                continue;
            }
            found.push(path);
        }
        found.sort();
        for path in found {
            if self.tails.len() >= MAX_SUBAGENT_FILES {
                break;
            }
            let tail = TranscriptTail::open(&path, SUBAGENT_LINE_BYTES)?;
            self.tails
                .insert(path, (tail, LinesSince::between(self.since, self.until)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(at: &str, id: &str) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":true,"timestamp":"{at}","message":{{"id":"{id}","model":"m","content":[],"usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#
        )
    }

    #[test]
    fn lines_since_keep_the_run_s_own_lines() {
        let mut since = LinesSince::new(Some(Timestamp(1_791_610_681_000)));
        assert!(since.waiting());
        assert!(!since.keep(&line("2026-10-10T05:38:00.000Z", "old")));
        assert!(
            !since.keep(r#"{"type":"last-prompt"}"#),
            "no time before the run's first"
        );
        assert!(since.keep(&line("2026-10-10T05:38:01.000Z", "new")));
        assert!(since.keep(r#"{"type":"mode"}"#), "everything after it");
        assert!(since.keep(&line("2026-10-10T05:37:00.000Z", "late")));
        assert!(!since.waiting());
        assert!(LinesSince::new(None).keep("not even JSON"));
    }

    #[test]
    fn a_window_ends_at_the_first_line_at_or_after_its_end() {
        let mut window = LinesSince::between(
            Some(Timestamp(1_791_610_681_000)),
            Some(Timestamp(1_791_610_690_000)),
        );
        assert!(!window.keep(&line("2026-10-10T05:38:00.000Z", "before")));
        assert!(window.keep(&line("2026-10-10T05:38:01.000Z", "first")));
        assert!(
            window.keep(r#"{"type":"mode"}"#),
            "no time inside the window"
        );
        assert!(window.keep(&line("2026-10-10T05:38:09.999Z", "last")));
        assert!(!window.ended());
        assert!(!window.keep(&line("2026-10-10T05:38:10.000Z", "next run")));
        assert!(window.ended());
        assert!(
            !window.keep(&line("2026-10-10T05:38:05.000Z", "late")),
            "nothing after the end, whatever its time"
        );
        assert!(!window.keep(r#"{"type":"mode"}"#));
        let mut open = LinesSince::between(None, Some(Timestamp(1_791_610_690_000)));
        assert!(open.keep("not even JSON"), "from the first line");
        assert!(!open.keep(&line("2026-10-10T05:38:11.000Z", "after")));
    }

    #[test]
    fn sub_agent_files_are_read_over_the_run_s_window() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("s1.jsonl");
        let sub = subagent_dir(&transcript);
        fs::create_dir_all(&sub)?;
        fs::write(
            sub.join("agent-a.jsonl"),
            format!(
                "{}\n{}\n",
                line("2026-10-10T05:38:02.000Z", "a1"),
                line("2026-10-10T05:38:20.000Z", "later-run")
            ),
        )?;
        let mut tails = SubagentTails::between(
            &transcript,
            Some(Timestamp(1_791_610_681_000)),
            Some(Timestamp(1_791_610_690_000)),
        );
        let lines = tails.poll()?;
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"a1\""));
        assert!(tails.at_end());
        Ok(())
    }

    #[test]
    fn sub_agent_files_are_found_and_read_from_the_run_s_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("s1.jsonl");
        let mut tails = SubagentTails::new(&transcript, Some(Timestamp(1_791_610_681_000)));
        assert!(tails.poll()?.is_empty(), "no directory yet");
        let sub = subagent_dir(&transcript);
        assert_eq!(sub, dir.path().join("s1").join("subagents"));
        fs::create_dir_all(&sub)?;
        fs::write(
            sub.join("agent-a.jsonl"),
            format!(
                "{}\n{}\n",
                line("2026-10-10T05:30:00.000Z", "earlier-run"),
                line("2026-10-10T05:38:02.000Z", "a1")
            ),
        )?;
        fs::write(sub.join("agent-a.meta.json"), "{}")?;
        let lines = tails.poll()?;
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"a1\""));
        fs::write(
            sub.join("agent-b.jsonl"),
            format!("{}\n", line("2026-10-10T05:38:03.000Z", "b1")),
        )?;
        let lines = tails.poll()?;
        assert_eq!((lines.len(), tails.files()), (1, 2));
        assert!(lines[0].contains("\"b1\""));
        assert!(tails.poll()?.is_empty(), "each line once");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_not_followed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let transcript = dir.path().join("s1.jsonl");
        let sub = subagent_dir(&transcript);
        fs::create_dir_all(&sub)?;
        let outside = dir.path().join("outside.jsonl");
        fs::write(
            &outside,
            format!("{}\n", line("2026-10-10T05:38:02.000Z", "x")),
        )?;
        std::os::unix::fs::symlink(&outside, sub.join("agent-link.jsonl"))?;
        let mut tails = SubagentTails::new(&transcript, None);
        assert!(tails.poll()?.is_empty());
        assert_eq!(tails.files(), 0);
        Ok(())
    }
}
