//! The rollouts of a Codex session's subagents (SPEC-619).
//!
//! Codex 0.170 writes each subagent's thread to a rollout of its own, in the same shared
//! `sessions/YYYY/MM/DD` folders as every other session of the sandbox. Its first line,
//! `session_meta`, names the thread (`id`), the thread that spawned it (`parent_thread_id`, also
//! under `source.subagent.thread_spawn`) and the session's root thread (`session_id`). Each of
//! its `token_usage_record` lines is one response of the thread its `thread_id` names; a forked
//! subagent's file starts with a copy of its parent's history. A response is answered by the
//! model of the thread's last `turn_context`.
//!
//! [`CodexChildren`] finds the descendants of one session, at any depth, and follows their
//! rollouts over a run's window ([`LinesSince`]). Discovery is bounded (CSA-6): only date
//! folders from the day before the session's own are listed, only files changed since the
//! window began are examined, each file's first line is read once and at most
//! [`SESSION_META_BYTES`] of it, at most [`MAX_EXAMINED`] files are examined and
//! [`MAX_CHILDREN`] followed, and the folders are listed at most every [`LIST_INTERVAL`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use yulius_domain::harness::valid_session_id;
use yulius_domain::time::Timestamp;

use crate::json::{obj_of, owned_str, parse_object, str_of};
use crate::observe::Observed;
use crate::paths::{self, RolloutDay};
use crate::subagents::LinesSince;
use crate::tail::{self, TranscriptTail};

/// Most rollouts examined (their first line read) for one session.
pub const MAX_EXAMINED: usize = 4096;

/// Most subagent rollouts followed for one session.
pub const MAX_CHILDREN: usize = 1024;

/// Longest first line read to tell whose a rollout is (Codex 0.170 writes about 30 KiB).
pub const SESSION_META_BYTES: u64 = 1024 * 1024;

/// Longest subagent rollout line read; longer ones (large tool output) are skipped. Token and
/// model lines are short.
pub const CHILD_LINE_BYTES: usize = 1024 * 1024;

/// How often a live watch lists the sessions folders again.
pub const LIST_INTERVAL: Duration = Duration::from_secs(2);

/// What a rollout's first line says of its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    thread: String,
    parent: Option<String>,
    session: Option<String>,
}

/// A followed subagent rollout.
#[derive(Debug)]
struct Child {
    thread: String,
    tail: TranscriptTail,
    window: LinesSince,
    /// The model of the thread's last `turn_context`, read also before the window.
    model: Option<String>,
}

/// Follows the subagent rollouts of one Codex session over a run's window.
#[derive(Debug)]
pub struct CodexChildren {
    sessions: PathBuf,
    rollout: PathBuf,
    root: Option<String>,
    first_day: Option<RolloutDay>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    /// Each examined file's link (`None`: not a session's first line), read once.
    examined: BTreeMap<PathBuf, Option<Link>>,
    /// The session's root thread and its descendants found so far.
    family: BTreeSet<String>,
    children: BTreeMap<PathBuf, Child>,
    listed_at: Option<Instant>,
    interval: Duration,
    max_examined: usize,
    max_children: usize,
}

impl CodexChildren {
    /// Follows the subagents of the session whose rollout is `rollout`
    /// (`<sessions>/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`), counting their lines over
    /// [`LinesSince::between`] `since` and `until`.
    #[must_use]
    pub fn between(rollout: &Path, since: Option<Timestamp>, until: Option<Timestamp>) -> Self {
        let sessions = rollout
            .ancestors()
            .nth(4)
            .map_or_else(PathBuf::new, Path::to_path_buf);
        let root = rollout_thread(rollout);
        Self {
            sessions,
            rollout: rollout.to_path_buf(),
            family: root.iter().cloned().collect(),
            root,
            first_day: paths::rollout_day(rollout).map(day_before),
            since,
            until,
            examined: BTreeMap::new(),
            children: BTreeMap::new(),
            listed_at: None,
            interval: LIST_INTERVAL,
            max_examined: MAX_EXAMINED,
            max_children: MAX_CHILDREN,
        }
    }

    /// Lists the folders on every poll: an ended run's recount reads everything at once.
    #[must_use]
    pub fn listing_every_poll(mut self) -> Self {
        self.interval = Duration::ZERO;
        self
    }

    /// The responses the subagents' new lines count in the window, each with the model that
    /// answered it. The folders are listed again when [`LIST_INTERVAL`] has passed; new
    /// subagents' rollouts are read from their start.
    ///
    /// # Errors
    /// A listing that fails or exceeds its bound; a rollout that cannot be read, when no other
    /// gave a line (its tail did not move: the failure shows again at the next poll).
    pub fn poll(&mut self) -> io::Result<Vec<Observed>> {
        if self
            .listed_at
            .is_none_or(|at| at.elapsed() >= self.interval)
        {
            self.discover()?;
            self.listed_at = Some(Instant::now());
        }
        let mut out = Vec::new();
        let mut failed = None;
        for child in self.children.values_mut() {
            match child.tail.poll() {
                Ok(lines) => {
                    for line in lines {
                        out.extend(child.read(&line));
                    }
                }
                Err(error) => failed = failed.or(Some(error)),
            }
        }
        match failed {
            Some(error) if out.is_empty() => Err(error),
            _ => Ok(out),
        }
    }

    /// Whether the last poll read every followed rollout to its end or its window's end.
    #[must_use]
    pub fn at_end(&self) -> bool {
        self.children
            .values()
            .all(|child| child.tail.at_end() || child.window.ended())
    }

    /// Number of subagent rollouts followed.
    #[must_use]
    pub fn files(&self) -> usize {
        self.children.len()
    }

    fn discover(&mut self) -> io::Result<()> {
        let (Some(root), Some(first_day)) = (self.root.clone(), self.first_day) else {
            return Ok(());
        };
        // A file not changed since the window began holds none of its lines.
        let changed_since = self.since.map(|since| {
            SystemTime::UNIX_EPOCH + Duration::from_millis(since.0.max(0).unsigned_abs())
        });
        let mut found = Vec::new();
        paths::walk_codex_days(&self.sessions, first_day, |entry| {
            let path = entry.path();
            if self.examined.contains_key(&path) || path == self.rollout {
                return Ok(());
            }
            if let Some(since) = changed_since
                && entry.metadata()?.modified()? < since
            {
                return Ok(());
            }
            found.push(path);
            Ok(())
        })?;
        found.sort();
        for path in found {
            if self.examined.len() >= self.max_examined {
                break;
            }
            match read_link(&path) {
                // Its first line is still being written: examined again next time.
                Err(()) => {}
                Ok(link) => {
                    self.examined.insert(path, link);
                }
            }
        }
        // A grandchild may be listed before its parent: until nothing more joins.
        loop {
            let mut joined = false;
            for (path, link) in &self.examined {
                let Some(link) = link else { continue };
                let member = link.parent.as_ref().is_some_and(|parent| {
                    self.family.contains(parent) || link.session.as_deref() == Some(root.as_str())
                });
                if !member || self.family.contains(&link.thread) {
                    continue;
                }
                if self.children.len() >= self.max_children {
                    return Ok(());
                }
                self.family.insert(link.thread.clone());
                self.children.insert(
                    path.clone(),
                    Child {
                        thread: link.thread.clone(),
                        tail: TranscriptTail::open(path, CHILD_LINE_BYTES)?,
                        window: LinesSince::between(self.since, self.until),
                        model: None,
                    },
                );
                joined = true;
            }
            if !joined {
                return Ok(());
            }
        }
    }
}

impl Child {
    /// The response a line of this subagent's rollout counts, if any.
    fn read(&mut self, line: &str) -> Option<Observed> {
        let kept = self.window.keep(line);
        let record = parse_object(line)?;
        let payload = obj_of(&record, "payload")?;
        match str_of(&record, "type")? {
            // The model answers the turns that follow, in the window or not.
            "turn_context" if !self.window.ended() => {
                if let Some(model) = owned_str(payload, "model") {
                    self.model = Some(model);
                }
                None
            }
            // Only the thread's own responses: a fork's copied history is its parent's.
            "token_usage_record"
                if kept && str_of(payload, "thread_id").is_none_or(|t| t == self.thread) =>
            {
                crate::codex::response_usage(payload, self.model.clone())
            }
            _ => None,
        }
    }
}

/// The thread of `rollout-<time>-<thread>.jsonl` (the time is `YYYY-MM-DDThh-mm-ss`).
fn rollout_thread(rollout: &Path) -> Option<String> {
    let name = rollout.file_name()?.to_str()?;
    let rest = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let thread = rest.get(20..)?;
    valid_session_id(thread).then(|| thread.to_owned())
}

/// The date folder a day before `day` or earlier: a child started after its session, in the
/// same time zone, but a day of slack costs little. A month's last day is not computed: any
/// day from the 28th of the month before is listed.
fn day_before((year, month, day): RolloutDay) -> RolloutDay {
    match (month, day) {
        (_, 2..) => (year, month, day - 1),
        (2.., _) => (year, month - 1, 28),
        _ => (year.saturating_sub(1), 12, 31),
    }
}

/// What the first line of `path` says of its thread: `Ok(None)` for a line that is no
/// `session_meta` of a valid thread, or longer than [`SESSION_META_BYTES`]; `Err` while the
/// line is incomplete or the file cannot be read now.
fn read_link(path: &Path) -> Result<Option<Link>, ()> {
    let file = tail::open_regular(path).map_err(|_| ())?;
    let mut first = Vec::new();
    std::io::BufReader::new(file.take(SESSION_META_BYTES))
        .read_until(b'\n', &mut first)
        .map_err(|_| ())?;
    if first.last() != Some(&b'\n') {
        return if u64::try_from(first.len()).unwrap_or(u64::MAX) >= SESSION_META_BYTES {
            Ok(None)
        } else {
            Err(())
        };
    }
    let Some(record) = std::str::from_utf8(&first).ok().and_then(parse_object) else {
        return Ok(None);
    };
    if str_of(&record, "type") != Some("session_meta") {
        return Ok(None);
    }
    let Some(payload) = obj_of(&record, "payload") else {
        return Ok(None);
    };
    let id = |value: Option<&str>| value.filter(|v| valid_session_id(v)).map(str::to_owned);
    let Some(thread) = id(str_of(payload, "id")) else {
        return Ok(None);
    };
    let spawned = obj_of(payload, "source")
        .and_then(|source| obj_of(source, "subagent"))
        .and_then(|subagent| obj_of(subagent, "thread_spawn"))
        .and_then(|spawn| str_of(spawn, "parent_thread_id"));
    Ok(Some(Link {
        thread,
        parent: id(str_of(payload, "parent_thread_id").or(spawned)),
        session: id(str_of(payload, "session_id")),
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    const ROOT: &str = "01a1f882-35c2-7413-b525-14d9ee43e464";

    fn at(rfc3339: &str) -> Timestamp {
        yulius_domain::time::parse_rfc3339(rfc3339).unwrap_or(Timestamp(0))
    }

    /// A rollout in Codex 0.170's shapes (synthetic ids, times and texts).
    pub(crate) struct Rollout {
        pub(crate) path: PathBuf,
        lines: Vec<String>,
        thread: String,
    }

    impl Rollout {
        /// `day` is `YYYY/MM/DD`; `parent` makes it a subagent (`source.subagent.thread_spawn`).
        pub(crate) fn new(
            sessions: &Path,
            day: &str,
            start: &str,
            thread: &str,
            parent: Option<&str>,
        ) -> Self {
            let time = start.get(..19).unwrap_or(start).replace(':', "-");
            let path = sessions
                .join(day)
                .join(format!("rollout-{time}-{thread}.jsonl"));
            let mut meta = serde_json::json!({
                "session_id": ROOT, "id": thread, "timestamp": start, "cwd": "/w",
                "cli_version": "0.170.0", "source": "cli", "thread_source": "user",
                "base_instructions": {"text": "Synthetic."}, "history_mode": "paginated"
            });
            if let Some(parent) = parent {
                meta["parent_thread_id"] = parent.into();
                meta["source"] = serde_json::json!(
                    {"subagent": {"thread_spawn": {"parent_thread_id": parent, "depth": 1}}}
                );
                meta["thread_source"] = "subagent".into();
            }
            let first = serde_json::json!(
                {"timestamp": start, "ordinal": 0, "type": "session_meta", "payload": meta}
            );
            Self {
                path,
                lines: vec![first.to_string()],
                thread: thread.to_owned(),
            }
        }

        /// Names `session` as its root thread instead of the test's.
        pub(crate) fn session(mut self, session: &str) -> Self {
            self.lines[0] = self.lines[0].replace(
                &format!("\"session_id\":\"{ROOT}\""),
                &format!("\"session_id\":\"{session}\""),
            );
            self
        }

        pub(crate) fn line(mut self, value: serde_json::Value) -> Self {
            self.lines.push(value.to_string());
            self
        }

        pub(crate) fn model(self, time: &str, model: &str) -> Self {
            self.line(
                serde_json::json!({"timestamp": time, "type": "turn_context",
                "payload": {"turn_id": "t", "model": model}}),
            )
        }

        /// A response of `thread` (the rollout's own when `None`) with `input` uncached input
        /// tokens, as many cached and `output` output tokens.
        pub(crate) fn response_of(
            self,
            thread: Option<&str>,
            time: &str,
            id: &str,
            input: u64,
            output: u64,
        ) -> Self {
            let thread = thread.map_or_else(|| self.thread.clone(), str::to_owned);
            self.line(
                serde_json::json!({"timestamp": time, "type": "token_usage_record",
                "payload": {"thread_id": thread, "session_id": ROOT, "response_id": id,
                "usage": {"input_tokens": input * 2, "cached_input_tokens": input,
                "cache_write_input_tokens": 0, "output_tokens": output}}}),
            )
        }

        pub(crate) fn response(self, time: &str, id: &str, input: u64, output: u64) -> Self {
            self.response_of(None, time, id, input, output)
        }

        pub(crate) fn write(self) -> io::Result<PathBuf> {
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&self.path, format!("{}\n", self.lines.join("\n")))?;
            Ok(self.path)
        }
    }

    fn drain(children: &mut CodexChildren) -> io::Result<Vec<Observed>> {
        let mut out = Vec::new();
        loop {
            out.extend(children.poll()?);
            if children.at_end() {
                return Ok(out);
            }
        }
    }

    /// (response id, model, output tokens) of each counted response, sorted.
    fn counted(observed: &[Observed]) -> Vec<(String, Option<String>, Option<u64>)> {
        let mut out: Vec<_> = observed
            .iter()
            .filter_map(|o| match o {
                Observed::AssistantMessage {
                    message_id,
                    model,
                    usage,
                } => Some((message_id.clone(), model.clone(), usage.output)),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    fn entry(id: &str, model: &str, output: u64) -> (String, Option<String>, Option<u64>) {
        (format!("codex-{id}"), Some(model.to_owned()), Some(output))
    }

    /// AC-1: the session's descendants at any depth count; a grandchild listed before its
    /// parent, a fork's copied history, an unrelated session, its child and a person's fork of
    /// the session do not, or count once.
    #[test]
    fn a_session_follows_its_descendants_only() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let sessions = dir.path();
        let day = "2026/11/21";
        let root = Rollout::new(sessions, day, "2026-11-21T09:00:00.000Z", ROOT, None)
            .model("2026-11-21T09:00:01.000Z", "gpt-6.2-sol")
            .response("2026-11-21T09:00:02.000Z", "resp_root", 10, 1)
            .write()?;
        let child = "01a1f900-0000-7000-8000-000000000001";
        Rollout::new(sessions, day, "2026-11-21T09:01:00.000Z", child, Some(ROOT))
            .model("2026-11-21T09:01:01.000Z", "gpt-6.2-astra")
            .response("2026-11-21T09:01:02.000Z", "resp_child", 10, 2)
            .response("2026-11-21T09:01:02.000Z", "resp_child", 10, 2)
            .write()?;
        // Its child, nested, starts with a name that sorts before its parent's.
        let grandchild = "01a1f800-0000-7000-8000-000000000002";
        let mut nested = Rollout::new(
            sessions,
            day,
            "2026-11-21T08:59:59.000Z",
            grandchild,
            Some(child),
        );
        nested.lines[0] = nested.lines[0].replace(ROOT, "01a1ffff-0000-7000-8000-00000000ffff");
        nested
            .model("2026-11-21T09:02:01.000Z", "gpt-6-luna")
            .response("2026-11-21T09:02:02.000Z", "resp_grandchild", 10, 3)
            .write()?;
        // A forked child repeats the root's meta and a response of the root.
        let fork = "01a1f900-0000-7000-8000-000000000003";
        Rollout::new(sessions, day, "2026-11-21T09:03:00.000Z", fork, Some(ROOT))
            .line(serde_json::json!({"timestamp": "2026-11-21T09:03:00.000Z",
                "type": "session_meta", "payload": {"id": ROOT, "session_id": ROOT}}))
            .model("2026-11-21T09:03:00.000Z", "gpt-6.2-sol")
            .response_of(
                Some(ROOT),
                "2026-11-21T09:03:00.000Z",
                "resp_copied",
                10,
                99,
            )
            .model("2026-11-21T09:03:01.000Z", "gpt-6.2-astra")
            .response("2026-11-21T09:03:02.000Z", "resp_fork", 10, 4)
            .write()?;
        // Another task's session and its child, and a person's fork of the session.
        let other = "01a1f700-0000-7000-8000-000000000004";
        let mut unrelated = Rollout::new(sessions, day, "2026-11-21T09:00:00.000Z", other, None);
        unrelated.lines[0] = unrelated.lines[0].replace(ROOT, other);
        unrelated
            .response("2026-11-21T09:00:05.000Z", "resp_other", 10, 50)
            .write()?;
        let mut their_child = Rollout::new(
            sessions,
            day,
            "2026-11-21T09:00:06.000Z",
            "01a1f700-0000-7000-8000-000000000005",
            Some(other),
        );
        their_child.lines[0] = their_child.lines[0].replace(ROOT, other);
        their_child
            .response("2026-11-21T09:00:07.000Z", "resp_other_child", 10, 51)
            .write()?;
        let mut forked = Rollout::new(
            sessions,
            day,
            "2026-11-21T09:04:00.000Z",
            "01a1f900-0000-7000-8000-000000000006",
            None,
        )
        .response("2026-11-21T09:04:01.000Z", "resp_person", 10, 52);
        forked.lines[0] = forked.lines[0].replace(
            "\"source\":\"cli\"",
            &format!("\"forked_from_id\":\"{ROOT}\",\"source\":\"cli\""),
        );
        forked.write()?;
        fs::write(sessions.join(day).join("notes.txt"), "not a rollout")?;

        let mut children = CodexChildren::between(&root, None, None);
        let observed = drain(&mut children)?;
        assert_eq!(
            counted(&observed),
            vec![
                entry("resp_child", "gpt-6.2-astra", 2),
                entry("resp_child", "gpt-6.2-astra", 2),
                entry("resp_fork", "gpt-6.2-astra", 4),
                entry("resp_grandchild", "gpt-6-luna", 3),
            ],
            "a repeated line is the accumulator's to count once"
        );
        assert_eq!(children.files(), 3);
        Ok(())
    }

    /// AC-2: a child that answers in two runs of a resumed session counts each response in its
    /// own run's window, at the model its earlier `turn_context` names.
    #[test]
    fn a_child_s_responses_count_in_the_window_that_holds_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = Rollout::new(
            dir.path(),
            "2026/11/20",
            "2026-11-20T23:00:00.000Z",
            ROOT,
            None,
        )
        .write()?;
        let child = "01a1f900-0000-7000-8000-000000000001";
        // Started the next day (another date folder), answered in both runs.
        Rollout::new(
            dir.path(),
            "2026/11/21",
            "2026-11-21T00:10:00.000Z",
            child,
            Some(ROOT),
        )
        .model("2026-11-21T00:10:01.000Z", "gpt-6.2-astra")
        .response("2026-11-21T00:10:02.000Z", "resp_1", 10, 1)
        .response("2026-11-21T01:30:00.000Z", "resp_2", 10, 2)
        .write()?;
        let resumed = at("2026-11-21T01:00:00.000Z");
        let first = drain(&mut CodexChildren::between(&root, None, Some(resumed)))?;
        assert_eq!(counted(&first), vec![entry("resp_1", "gpt-6.2-astra", 1)]);
        let second = drain(&mut CodexChildren::between(&root, Some(resumed), None))?;
        assert_eq!(
            counted(&second),
            vec![entry("resp_2", "gpt-6.2-astra", 2)],
            "the model line before the window still names the model"
        );
        Ok(())
    }

    /// AC-5: date folders before the session's (and the day before it), and files not changed
    /// since the window began, are not read; a first line still being written is read again.
    #[test]
    fn discovery_reads_only_what_the_window_can_hold() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = Rollout::new(
            dir.path(),
            "2026/11/03",
            "2026-11-03T09:00:00.000Z",
            ROOT,
            None,
        )
        .write()?;
        let child = |n: u32| format!("01a1f900-0000-7000-8000-00000000000{n}");
        // Two days earlier: a child cannot be there (a copied rollout, a clock gone wrong).
        Rollout::new(
            dir.path(),
            "2026/11/01",
            "2026-11-01T09:00:00.000Z",
            &child(1),
            Some(ROOT),
        )
        .response("2026-11-03T10:00:00.000Z", "resp_early", 10, 1)
        .write()?;
        Rollout::new(
            dir.path(),
            "2026/11/02",
            "2026-11-02T23:59:00.000Z",
            &child(2),
            Some(ROOT),
        )
        .response("2026-11-03T10:00:00.000Z", "resp_slack", 10, 2)
        .write()?;
        let complete = Rollout::new(
            dir.path(),
            "2026/11/03",
            "2026-11-03T09:30:00.000Z",
            &child(3),
            Some(ROOT),
        )
        .response("2026-11-03T10:00:00.000Z", "resp_late", 10, 3);
        let partial = complete.path.clone();
        let full = format!("{}\n", complete.lines.join("\n"));
        fs::write(&partial, &full[..20])?;
        let mut children = CodexChildren::between(&root, None, None).listing_every_poll();
        assert_eq!(
            counted(&drain(&mut children)?),
            vec![entry_unknown("resp_slack", 2)]
        );
        fs::write(&partial, &full)?;
        assert_eq!(
            counted(&drain(&mut children)?),
            vec![entry_unknown("resp_late", 3)]
        );

        // A window that began after every file was written examines none of them.
        let future = Timestamp(
            i64::try_from(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_millis(),
            )? + 60_000,
        );
        let mut later = CodexChildren::between(&root, Some(future), None);
        assert!(drain(&mut later)?.is_empty());
        assert!(later.examined.is_empty(), "{:?}", later.examined.keys());
        Ok(())
    }

    fn entry_unknown(id: &str, output: u64) -> (String, Option<String>, Option<u64>) {
        (format!("codex-{id}"), None, Some(output))
    }

    /// CSA-6: at most so many files are examined and so many children followed; a live watch
    /// lists the folders again only after the interval.
    #[test]
    fn discovery_stops_at_its_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let day = "2026/11/21";
        let root = Rollout::new(dir.path(), day, "2026-11-21T09:00:00.000Z", ROOT, None).write()?;
        for n in 0..6 {
            let thread = format!("01a1f900-0000-7000-8000-00000000001{n}");
            Rollout::new(
                dir.path(),
                day,
                "2026-11-21T09:01:00.000Z",
                &thread,
                Some(ROOT),
            )
            .response("2026-11-21T09:01:02.000Z", &format!("resp_{n}"), 1, 1)
            .write()?;
        }
        let mut few = CodexChildren::between(&root, None, None);
        few.max_examined = 4;
        few.max_children = 2;
        assert_eq!(drain(&mut few)?.len(), 2);
        assert_eq!((few.examined.len(), few.files()), (4, 2));

        let mut live = CodexChildren::between(&root, None, None);
        assert_eq!(drain(&mut live)?.len(), 6);
        Rollout::new(
            dir.path(),
            day,
            "2026-11-21T09:05:00.000Z",
            "01a1f900-0000-7000-8000-000000000020",
            Some(ROOT),
        )
        .response("2026-11-21T09:05:02.000Z", "resp_new", 1, 1)
        .write()?;
        assert!(
            live.poll()?.is_empty(),
            "not listed again within the interval"
        );
        live.listed_at = Instant::now().checked_sub(LIST_INTERVAL);
        assert_eq!(counted(&live.poll()?), vec![entry_unknown("resp_new", 1)]);
        Ok(())
    }

    #[test]
    fn a_day_before_crosses_months_and_years() {
        assert_eq!(day_before((2026, 11, 21)), (2026, 11, 20));
        assert_eq!(day_before((2026, 11, 1)), (2026, 10, 28));
        assert_eq!(day_before((2026, 1, 1)), (2025, 12, 31));
        assert_eq!(
            rollout_thread(Path::new(&format!(
                "/s/rollout-2026-11-21T09-05-30-{ROOT}.jsonl"
            )))
            .as_deref(),
            Some(ROOT)
        );
        assert_eq!(rollout_thread(Path::new("/s/rollout-x.jsonl")), None);
    }
}
