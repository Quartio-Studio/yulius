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
/// before that one are dropped too; once a line is kept, every later line is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinesSince {
    since: Option<Timestamp>,
}

impl LinesSince {
    /// Keeps the lines from the first one at or after `since`; every line with `None`.
    #[must_use]
    pub fn new(since: Option<Timestamp>) -> Self {
        Self { since }
    }

    /// Whether `line` is kept.
    pub fn keep(&mut self, line: &str) -> bool {
        let Some(since) = self.since else {
            return true;
        };
        if claude::record_time(line).is_some_and(|at| at >= since) {
            self.since = None;
            return true;
        }
        false
    }

    /// Whether lines are still being dropped.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.since.is_some()
    }
}

/// Follows the sub-agent transcripts of one session.
#[derive(Debug)]
pub struct SubagentTails {
    dir: PathBuf,
    since: Option<Timestamp>,
    tails: BTreeMap<PathBuf, (TranscriptTail, LinesSince)>,
}

impl SubagentTails {
    /// Follows the sub-agents of the session transcript `transcript`, counting their lines from
    /// `since` (`None`: all of them).
    #[must_use]
    pub fn new(transcript: &Path, since: Option<Timestamp>) -> Self {
        Self {
            dir: subagent_dir(transcript),
            since,
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
            self.tails.insert(path, (tail, LinesSince::new(self.since)));
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
