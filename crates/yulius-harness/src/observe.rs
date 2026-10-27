//! Normalized observations from transcript lines and the usage accumulator (SPEC-002 §7.2,
//! EVT-001/002).
//!
//! [`crate::claude::observe_line`] turns one transcript line into zero or more [`Observed`]
//! values. Unknown record types yield nothing; lines that are not
//! JSON objects or lack required fields yield [`Observed::Malformed`].

use std::collections::{HashSet, VecDeque};

use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::TokenUsage;

/// Maximum number of recent Claude message ids remembered for de-duplication.
pub const MESSAGE_ID_MEMORY: usize = 65_536;

/// One normalized fact from a transcript line.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    /// Session identity (every genuine user prompt record).
    SessionMeta {
        /// Provider session id.
        session_id: String,
        /// Working directory, if recorded.
        cwd: Option<String>,
        /// Model, if recorded on this record.
        model: Option<String>,
        /// CLI version, if recorded.
        version: Option<String>,
    },
    /// A genuine user prompt started a turn (not tool results or injected context).
    UserTurn {
        /// When the prompt was recorded (the record's `timestamp`), if parseable.
        at: Option<Timestamp>,
        /// Claude Code woke the session itself (a `<task-notification>` prompt: background work
        /// ended or reported), no person or Yulius typed it.
        wake: bool,
    },
    /// The person stopped the agent's turn (Esc, or Ctrl+C while it works): Claude Code records
    /// a user line "[Request interrupted by user]" (or "… for tool use") and waits for input.
    /// No Stop hook runs for such a turn, so this is its only end.
    TurnInterrupted {
        /// When it was recorded, if parseable.
        at: Option<Timestamp>,
    },
    /// One Claude API message (repeated once per transcript line of that message).
    AssistantMessage {
        /// Provider message id (`message.id`), the de-duplication key.
        message_id: String,
        /// Model that produced it.
        model: Option<String>,
        /// Usage reported with the message; unknown fields stay `None`.
        usage: TokenUsage,
    },
    /// The provider reported that a usage limit was reached.
    LimitReached {
        /// When the limit resets, if the provider said so in parseable form.
        resets_at: Option<Timestamp>,
        /// Provider text describing the limit (shortened).
        message: String,
        /// Length of the limited window in minutes — 300 for the five-hour session, 10080 for
        /// the weekly one — when the provider said which.
        window_minutes: Option<u32>,
        /// When Claude Code recorded it (the record's `timestamp`), if parseable: tells a new
        /// limit from one in a conversation's history (SPEC-074 SUP-002).
        at: Option<Timestamp>,
    },
    /// The agent finished a turn. Claude emits this for each line of an `end_turn` message, so
    /// consumers treat it idempotently; the Stop hook remains the authoritative turn boundary.
    TurnComplete {
        /// Turn duration in milliseconds, if reported.
        duration_ms: Option<u64>,
        /// When the session's own turn ended (the record's `timestamp`); `None` for a
        /// sub-agent's turn or an unparseable time.
        at: Option<Timestamp>,
    },
    /// Claude Code recorded the session goal's state (`goal_status` attachment, TSK-014).
    Goal {
        /// What was recorded.
        outcome: GoalOutcome,
    },
    /// Claude Code closed a turn (the `turn_duration` system record; `stop_hook_summary` is also
    /// written when a Stop hook blocks and the turn goes on, so it is not this). Not
    /// authoritative while Yulius runs (the Stop hook is); used to reduce a replayed transcript
    /// to its final state and to learn what the agent still waits for.
    TurnEnded {
        /// Background agents Claude Code still waits for (`pendingBackgroundAgentCount`); the
        /// session wakes by itself when they finish. `None` when the CLI does not say.
        background_agents: Option<u32>,
        /// When the turn closed (the record's `timestamp`), if parseable.
        at: Option<Timestamp>,
        /// How long Claude Code says the turn ran (`durationMs`, "Brewed for 51s"). A turn that
        /// waited for its background agents counts from the prompt that started them, so the
        /// spans of consecutive turns can overlap.
        duration_ms: Option<u64>,
    },
    /// The agent works: an assistant line that is not the end of a turn (a tool call, partial
    /// output). After a recorded turn end it shows the turn goes on (a Stop hook continued it).
    AgentWork {
        /// When it was recorded, if parseable.
        at: Option<Timestamp>,
    },
    /// The session started background work it is woken by (SPEC-079 BGW-002): a Monitor stream
    /// (its tool result's `toolUseResult` is `{taskId, timeoutMs, …}`) or a background agent
    /// (`{status: "async_launched", agentId, …}`).
    BackgroundStarted {
        /// Claude Code's task id, which the `<task-notification>` that ends it names.
        id: String,
        /// A background agent; otherwise a Monitor stream.
        agent: bool,
        /// How long a Monitor lives at most (`timeoutMs`, capped); `None` for an agent.
        expires_in_ms: Option<u64>,
        /// When it was recorded, if parseable.
        at: Option<Timestamp>,
    },
    /// A `<task-notification>` said the background work `id` ended: it carries a `<status>`, or
    /// it is a Monitor's expiry event.
    BackgroundEnded {
        /// Claude Code's task id.
        id: String,
    },
    /// A `<task-notification>` said the background agent `id` stopped with background work of its
    /// own still running (its `<note>`): it resumes by itself when that work ends, and the same id
    /// notifies again; the result it carries may be interim (SPEC-079 BGW-006).
    BackgroundResumable {
        /// Claude Code's task id.
        id: String,
    },
    /// Claude Code's goal mode answered a turn an error ended (SPEC-333): it retries the turn
    /// itself (`Goal still active · <cause> · retrying in 1 min (1/3) · …`), or it stopped: the
    /// retries are spent (`Goal paused after 3 automatic retries · <cause> · …`) or the API error
    /// is one it does not retry (`Goal paused · the API rejected the last request · …`).
    GoalRetry {
        /// A retry is armed: Claude Code continues the goal by itself.
        armed: bool,
        /// What Claude Code said, without its closing advice ("send a message to …").
        notice: String,
    },
    /// The API rejected the session's prompt as too long: its context is full, and every later
    /// prompt gets the same answer until it is compacted (#213). No Stop hook ends such a turn.
    ContextFull,
    /// Claude Code compacted the conversation (a `compact_boundary` record; `/compact` or its
    /// automatic compaction).
    Compacted,
    /// A compaction failed ("Error during compaction" in the command's error output).
    CompactFailed,
    /// The line is not a JSON object or lacks a required field.
    Malformed,
}

/// The latest limit signal seen by a [`UsageAccumulator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitSignal {
    /// When the limit resets, if known.
    pub resets_at: Option<Timestamp>,
    /// Provider text.
    pub message: String,
}

/// Most separate turn spans an [`ActiveTime`] remembers; older ones are settled into its total.
pub const ACTIVE_SPAN_MEMORY: usize = 1_024;

/// A session's active agent time, from its transcript alone (SPEC-087 MET-001): the union of its
/// turns' spans, in the times the transcript records. A turn runs from its prompt — or from the
/// agent's first work after a turn's end, when a Stop hook continued it — to its end: the
/// session's `end_turn` answer, an interrupt, a usage limit or Claude Code's turn record. A turn
/// record also covers the span its `durationMs` names, which includes the time the turn waited
/// for its background agents. A turn still open counts up to the newest time read, so the figure
/// never depends on a clock other than the transcript's and is the same when the transcript is
/// read again after a restart.
#[derive(Debug, Clone, Default)]
pub struct ActiveTime {
    /// Closed spans (ms), disjoint and sorted by start.
    spans: Vec<(i64, i64)>,
    /// Spans dropped from `spans` beyond [`ACTIVE_SPAN_MEMORY`]: their total and their end.
    settled_ms: u64,
    settled_until: i64,
    open_since: Option<i64>,
    latest: Option<i64>,
}

fn span_ms(start: i64, end: i64) -> u64 {
    u64::try_from(end.saturating_sub(start)).unwrap_or(0)
}

impl ActiveTime {
    /// A time read in the transcript.
    pub fn seen(&mut self, at: Timestamp) {
        self.latest = Some(self.latest.map_or(at.0, |latest| latest.max(at.0)));
    }

    /// A turn is in progress from `at`, unless one is already.
    pub fn open(&mut self, at: Timestamp) {
        self.seen(at);
        self.open_since.get_or_insert(at.0);
    }

    /// The turn in progress, if any, ended at `at`.
    pub fn close(&mut self, at: Timestamp) {
        self.seen(at);
        if let Some(start) = self.open_since.take() {
            self.add(start, at.0);
        }
    }

    /// Adds the span `start..end` (ms); overlaps with known spans count once.
    pub fn add(&mut self, start: i64, end: i64) {
        let (mut start, mut end) = (start.max(self.settled_until), end);
        if end <= start {
            return;
        }
        self.spans.retain(|&(a, b)| {
            let apart = b < start || a > end;
            if !apart {
                start = start.min(a);
                end = end.max(b);
            }
            apart
        });
        let index = self.spans.partition_point(|&(a, _)| a < start);
        self.spans.insert(index, (start, end));
        while self.spans.len() > ACTIVE_SPAN_MEMORY {
            let (a, b) = self.spans.remove(0);
            self.settled_ms = self.settled_ms.saturating_add(span_ms(a, b));
            self.settled_until = b;
        }
    }

    /// When the turn in progress started, if one is.
    #[must_use]
    pub fn open_since(&self) -> Option<Timestamp> {
        self.open_since.map(Timestamp)
    }

    /// Active time so far (ms): the closed spans and the open turn up to the newest time read.
    /// `None` before any turn.
    #[must_use]
    pub fn total_ms(&self) -> Option<u64> {
        if self.spans.is_empty() && self.settled_ms == 0 && self.open_since.is_none() {
            return None;
        }
        let closed = self.spans.iter().fold(self.settled_ms, |sum, &(a, b)| {
            sum.saturating_add(span_ms(a, b))
        });
        let open = match (self.open_since, self.latest) {
            (Some(since), Some(latest)) if latest > since => {
                // The part of the open turn no closed span covers.
                let covered = self
                    .spans
                    .iter()
                    .map(|&(a, b)| span_ms(a.max(since), b.min(latest)))
                    .fold(0u64, u64::saturating_add);
                span_ms(since, latest).saturating_sub(covered)
            }
            _ => 0,
        };
        Some(closed.saturating_add(open))
    }
}

/// Folds observations of one attempt into usage totals without double counting.
///
/// Usage is summed once per `message.id`; repeated lines of the same message refine it
/// (per-field maximum), and ids already counted earlier are ignored. Unknown usage stays
/// unknown. The session's turns and active time come from the same lines (SPEC-087).
#[derive(Debug, Clone, Default)]
pub struct UsageAccumulator {
    finalized: Option<TokenUsage>,
    current: Option<(String, TokenUsage)>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    limit: Option<LimitSignal>,
    model: Option<String>,
    malformed: u64,
    /// Claude Code's turn records (`turn_duration`): one per turn of the session.
    turn_records: u64,
    /// The session's `end_turn` answers (a goal continuation answers more than once per turn).
    turn_answers: u64,
    active: ActiveTime,
}

fn max_field(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

fn add_known(total: Option<TokenUsage>, usage: TokenUsage) -> Option<TokenUsage> {
    Some(total.map_or(usage, |t| t.checked_add(usage)))
}

impl UsageAccumulator {
    /// An empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one observation.
    pub fn observe(&mut self, observed: &Observed) {
        match observed {
            Observed::AssistantMessage {
                message_id,
                model,
                usage,
            } => {
                if let Some(model) = model {
                    self.model = Some(model.clone());
                }
                self.observe_message(message_id, *usage);
            }
            Observed::LimitReached {
                resets_at,
                message,
                at,
                ..
            } => {
                self.limit = Some(LimitSignal {
                    resets_at: *resets_at,
                    message: message.clone(),
                });
                // A usage limit ends the turn.
                if let Some(at) = at {
                    self.active.close(*at);
                }
            }
            // A sub-agent's answer (no time) is not the session's.
            Observed::TurnComplete { at: Some(at), .. } => {
                self.turn_answers += 1;
                self.active.close(*at);
            }
            Observed::TurnEnded {
                at, duration_ms, ..
            } => {
                self.turn_records += 1;
                if let Some(at) = at {
                    self.active.close(*at);
                    if let Some(ms) = duration_ms.and_then(|ms| i64::try_from(ms).ok()) {
                        self.active.add(at.0.saturating_sub(ms), at.0);
                    }
                }
            }
            Observed::UserTurn { at: Some(at), .. } | Observed::AgentWork { at: Some(at) } => {
                self.active.open(*at);
            }
            Observed::TurnInterrupted { at: Some(at) } => self.active.close(*at),
            Observed::BackgroundStarted { at: Some(at), .. } => self.active.seen(*at),
            Observed::Malformed => self.malformed += 1,
            _ => {}
        }
    }

    /// Folds a line of a sub-agent's transcript (`<session>/subagents/*.jsonl`): only its token
    /// usage counts. Its model, limits and turns are the sub-agent's, not the session's.
    pub fn observe_side(&mut self, observed: &Observed) {
        if let Observed::AssistantMessage {
            message_id, usage, ..
        } = observed
        {
            self.observe_message(message_id, *usage);
        }
    }

    /// Folds several observations in order.
    pub fn observe_all<'a>(&mut self, observed: impl IntoIterator<Item = &'a Observed>) {
        for item in observed {
            self.observe(item);
        }
    }

    fn observe_message(&mut self, id: &str, usage: TokenUsage) {
        if let Some((current_id, current_usage)) = &mut self.current
            && current_id == id
        {
            *current_usage = TokenUsage {
                input: max_field(current_usage.input, usage.input),
                output: max_field(current_usage.output, usage.output),
                cache_read: max_field(current_usage.cache_read, usage.cache_read),
                cache_write: max_field(current_usage.cache_write, usage.cache_write),
            };
            return;
        }
        if self.seen.contains(id) {
            return;
        }
        if let Some((_, previous)) = self.current.take() {
            self.finalized = add_known(self.finalized, previous);
        }
        if self.seen_order.len() >= MESSAGE_ID_MEMORY
            && let Some(oldest) = self.seen_order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.seen.insert(id.to_owned());
        self.seen_order.push_back(id.to_owned());
        self.current = Some((id.to_owned(), usage));
    }

    /// Usage of the observed lines: the sum of distinct messages. All fields are `None` when
    /// nothing was observed.
    #[must_use]
    pub fn usage(&self) -> TokenUsage {
        let mut total = self.finalized;
        if let Some((_, current)) = &self.current {
            total = add_known(total, *current);
        }
        total.unwrap_or(TokenUsage::UNKNOWN)
    }

    /// Latest limit signal, if any.
    #[must_use]
    pub fn last_limit(&self) -> Option<&LimitSignal> {
        self.limit.as_ref()
    }

    /// Latest model seen, if any.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Number of malformed lines seen (a diagnostic; never fatal).
    #[must_use]
    pub fn malformed_lines(&self) -> u64 {
        self.malformed
    }

    /// The session's turns: Claude Code's turn records, or, from a CLI that writes none, its
    /// `end_turn` answers.
    #[must_use]
    pub fn turns(&self) -> u64 {
        if self.turn_records > 0 {
            self.turn_records
        } else {
            self.turn_answers
        }
    }

    /// The session's active agent time so far.
    #[must_use]
    pub fn active(&self) -> &ActiveTime {
        &self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, input: Option<u64>, output: Option<u64>) -> Observed {
        Observed::AssistantMessage {
            message_id: id.into(),
            model: Some("claude-test".into()),
            usage: TokenUsage {
                input,
                output,
                cache_read: Some(0),
                cache_write: Some(0),
            },
        }
    }

    #[test]
    fn empty_accumulator_reports_unknown() {
        let acc = UsageAccumulator::new();
        assert!(acc.usage().is_unknown());
        assert_eq!(acc.last_limit(), None);
    }

    #[test]
    fn repeated_message_lines_count_once_and_refine() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[
            message("m1", Some(10), Some(1)),
            message("m1", Some(10), Some(7)),
            message("m2", Some(5), Some(2)),
            // A replay of an earlier message far later is ignored.
            message("m1", Some(10), Some(7)),
        ]);
        let usage = acc.usage();
        assert_eq!(usage.input, Some(15));
        assert_eq!(usage.output, Some(9));
        assert_eq!(acc.model(), Some("claude-test"));
    }

    #[test]
    fn absent_usage_makes_total_unknown() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[message("m1", Some(10), Some(1)), message("m2", None, None)]);
        assert_eq!(acc.usage().input, None);
        assert_eq!(acc.usage().cache_read, Some(0));
    }

    #[test]
    fn counts_malformed_lines_turns_and_keeps_the_latest_limit() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[
            Observed::Malformed,
            Observed::TurnComplete {
                duration_ms: Some(5),
                at: None,
            },
            Observed::LimitReached {
                resets_at: None,
                message: "first".into(),
                window_minutes: None,
                at: None,
            },
            Observed::LimitReached {
                resets_at: Some(Timestamp(5)),
                message: "limit".into(),
                window_minutes: Some(300),
                at: None,
            },
        ]);
        assert_eq!(acc.malformed_lines(), 1);
        assert_eq!(acc.turns(), 0, "an answer without a time is a sub-agent's");
        assert_eq!(
            acc.last_limit(),
            Some(&LimitSignal {
                resets_at: Some(Timestamp(5)),
                message: "limit".into()
            })
        );
    }

    #[test]
    fn message_id_memory_is_bounded() {
        let mut acc = UsageAccumulator::new();
        for i in 0..(MESSAGE_ID_MEMORY + 10) {
            acc.observe(&message(&format!("m{i}"), Some(1), Some(1)));
        }
        assert!(acc.seen.len() <= MESSAGE_ID_MEMORY);
        assert_eq!(acc.usage().input, Some((MESSAGE_ID_MEMORY + 10) as u64));
    }

    const S: i64 = 1_000;

    fn prompt(at: i64, wake: bool) -> Observed {
        Observed::UserTurn {
            at: Some(Timestamp(at * S)),
            wake,
        }
    }

    fn work(at: i64) -> Observed {
        Observed::AgentWork {
            at: Some(Timestamp(at * S)),
        }
    }

    fn answer(at: Option<i64>) -> Observed {
        Observed::TurnComplete {
            duration_ms: None,
            at: at.map(|at| Timestamp(at * S)),
        }
    }

    fn record(at: i64, seconds: u64, agents: u32) -> Observed {
        Observed::TurnEnded {
            background_agents: Some(agents),
            at: Some(Timestamp(at * S)),
            duration_ms: Some(seconds * 1_000),
        }
    }

    fn active_seconds(acc: &UsageAccumulator) -> Option<u64> {
        acc.active().total_ms().map(|ms| ms / 1_000)
    }

    /// SPEC-087 MET-001: a turn that waited for its background agents is one span from the
    /// prompt that started them; Claude Code's records of the turns in between overlap it and
    /// count once (the shape of task #59's transcript, 2026-10-13).
    #[test]
    fn active_time_is_the_union_of_the_turns_spans() {
        let mut acc = UsageAccumulator::new();
        assert_eq!(active_seconds(&acc), None, "no turn yet: unknown, not zero");
        acc.observe_all(&[
            prompt(0, false),
            work(5),
            answer(Some(100)),
            record(100, 100, 1),
            // Idle while a background agent runs, then woken by it.
            prompt(160, true),
            answer(Some(170)),
            record(170, 10, 1),
            prompt(200, true),
            answer(Some(230)),
            // The last record covers the whole wait, from the first prompt.
            record(230, 230, 0),
        ]);
        assert_eq!(active_seconds(&acc), Some(230));
        assert_eq!(acc.active().open_since(), None);
        assert_eq!(acc.turns(), 3);
    }

    /// SPEC-087 MET-001: idle time between a turn's end and a person's next prompt is not active.
    #[test]
    fn idle_time_between_turns_is_not_active() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[
            prompt(0, false),
            answer(Some(30)),
            record(30, 30, 0),
            prompt(3_600, false),
            answer(Some(3_620)),
            record(3_620, 20, 0),
        ]);
        assert_eq!(active_seconds(&acc), Some(50));
    }

    /// SPEC-087 MET-001: the turn in progress counts up to the newest time the transcript shows;
    /// an interrupt or a usage limit ends it.
    #[test]
    fn an_open_turn_counts_to_the_newest_line_and_an_interrupt_or_limit_ends_it() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[prompt(0, false), work(40)]);
        assert_eq!(active_seconds(&acc), Some(40));
        assert_eq!(acc.active().open_since(), Some(Timestamp(0)));
        acc.observe(&Observed::TurnInterrupted {
            at: Some(Timestamp(45 * S)),
        });
        assert_eq!(acc.active().open_since(), None);
        acc.observe_all(&[prompt(100, false), work(110)]);
        acc.observe(&Observed::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            at: Some(Timestamp(120 * S)),
        });
        // Lines after the limit do not extend the closed turn.
        acc.observe(&Observed::BackgroundStarted {
            id: "b1".into(),
            agent: false,
            expires_in_ms: None,
            at: Some(Timestamp(500 * S)),
        });
        assert_eq!(active_seconds(&acc), Some(65));
    }

    /// SPEC-087 MET-001, MET-003: a Stop hook that continues the turn (a goal not met yet) keeps it
    /// active; the session's turns are Claude Code's turn records, not its answers or a
    /// sub-agent's (task #8 counted 537 turns where its transcript has 39).
    #[test]
    fn a_continued_turn_stays_active_and_counts_once() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[
            prompt(0, false),
            answer(None),
            answer(Some(10)),
            // The goal evaluator sent the agent back to work.
            work(20),
            answer(None),
            answer(Some(30)),
        ]);
        assert_eq!(active_seconds(&acc), Some(20), "the gap is not known yet");
        assert_eq!(
            acc.turns(),
            2,
            "without turn records: the session's answers"
        );
        acc.observe(&record(31, 31, 0));
        assert_eq!(active_seconds(&acc), Some(31));
        assert_eq!(acc.turns(), 1);
    }

    #[test]
    fn active_span_memory_is_bounded() {
        let mut time = ActiveTime::default();
        let count = i64::try_from(ACTIVE_SPAN_MEMORY).unwrap_or(i64::MAX) + 5;
        for i in 0..count {
            time.add(i * 10, i * 10 + 3);
        }
        assert!(time.spans.len() <= ACTIVE_SPAN_MEMORY);
        assert_eq!(time.total_ms(), u64::try_from(count * 3).ok());
        // A span reaching back into what was settled counts only its new part.
        time.add(0, 2);
        assert_eq!(time.total_ms(), u64::try_from(count * 3).ok());
    }

    /// SPEC-087 MET-004: a sub-agent's transcript adds its tokens, nothing else.
    #[test]
    fn a_sub_agents_lines_add_only_their_tokens() {
        let mut acc = UsageAccumulator::new();
        acc.observe_all(&[prompt(0, false), message("m1", Some(10), Some(5))]);
        let mut side = message("s1", Some(3), Some(2));
        if let Observed::AssistantMessage { model, .. } = &mut side {
            *model = Some("claude-haiku".into());
        }
        acc.observe_side(&side);
        acc.observe_side(&answer(Some(50)));
        acc.observe_side(&Observed::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            at: Some(Timestamp(60 * S)),
        });
        assert_eq!((acc.usage().input, acc.usage().output), (Some(13), Some(7)));
        assert_eq!(acc.model(), Some("claude-test"));
        assert_eq!(acc.last_limit(), None);
        assert_eq!(acc.active().open_since(), Some(Timestamp(0)));
    }
}
