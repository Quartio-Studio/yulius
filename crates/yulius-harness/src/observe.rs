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

/// Folds observations of one attempt into usage totals without double counting.
///
/// Usage is summed once per `message.id`; repeated lines of the same message refine it
/// (per-field maximum), and ids already counted earlier are ignored. Unknown usage stays
/// unknown.
#[derive(Debug, Clone, Default)]
pub struct UsageAccumulator {
    finalized: Option<TokenUsage>,
    current: Option<(String, TokenUsage)>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    limit: Option<LimitSignal>,
    model: Option<String>,
    malformed: u64,
    turns_completed: u64,
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
                resets_at, message, ..
            } => {
                self.limit = Some(LimitSignal {
                    resets_at: *resets_at,
                    message: message.clone(),
                });
            }
            Observed::TurnComplete { .. } => self.turns_completed += 1,
            Observed::Malformed => self.malformed += 1,
            _ => {}
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

    /// Number of turn-complete observations seen (may over-count for Claude).
    #[must_use]
    pub fn turn_complete_signals(&self) -> u64 {
        self.turns_completed
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
        assert_eq!(acc.turn_complete_signals(), 1);
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
}
