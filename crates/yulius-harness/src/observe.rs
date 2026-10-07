//! Normalized observations from transcript lines and the usage accumulator (SPEC-002 §7.2,
//! EVT-001/002).
//!
//! [`crate::claude::observe_line`] and [`crate::codex::observe_line`] turn one transcript line
//! into zero or more [`Observed`] values. Unknown record types yield nothing; lines that are not
//! JSON objects or lack required fields yield [`Observed::Malformed`].

use std::collections::{HashSet, VecDeque};

use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, TokenUsage};

/// Maximum number of recent Claude message ids remembered for de-duplication.
pub const MESSAGE_ID_MEMORY: usize = 65_536;

/// One normalized fact from a transcript line.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    /// Session identity (Claude: every genuine user prompt record; Codex: `session_meta`).
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
    /// Model settings of the following turns (Codex `turn_context`).
    TurnContext {
        /// Working directory, if recorded.
        cwd: Option<String>,
        /// Model, if recorded.
        model: Option<String>,
        /// Reasoning effort, if recorded.
        effort: Option<String>,
    },
    /// A genuine user prompt started a turn (not tool results or injected context).
    UserTurn,
    /// One Claude API message (repeated once per transcript line of that message).
    AssistantMessage {
        /// Provider message id (`message.id`), the de-duplication key.
        message_id: String,
        /// Model that produced it.
        model: Option<String>,
        /// Usage reported with the message; unknown fields stay `None`.
        usage: TokenUsage,
    },
    /// Cumulative session usage (Codex `token_count.info.total_token_usage`); last value wins.
    CumulativeUsage(TokenUsage),
    /// Provider rate-limit snapshot (Codex `token_count.rate_limits`).
    RateLimits(AccountUsage),
    /// The provider reported that a usage limit was reached.
    LimitReached {
        /// When the limit resets, if the provider said so in parseable form.
        resets_at: Option<Timestamp>,
        /// Provider text describing the limit (shortened).
        message: String,
    },
    /// The agent finished a turn. Claude emits this for each line of an `end_turn` message, so
    /// consumers treat it idempotently; the Stop hook remains the authoritative turn boundary.
    TurnComplete {
        /// Turn duration in milliseconds, if reported.
        duration_ms: Option<u64>,
    },
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
/// Claude usage is summed once per `message.id`; repeated lines of the same message refine it
/// (per-field maximum), and ids already counted earlier are ignored. Codex usage is cumulative:
/// the last `CumulativeUsage` wins. Unknown usage stays unknown.
#[derive(Debug, Clone, Default)]
pub struct UsageAccumulator {
    finalized: Option<TokenUsage>,
    current: Option<(String, TokenUsage)>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    cumulative: Option<TokenUsage>,
    rate_limits: Option<AccountUsage>,
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
            Observed::CumulativeUsage(usage) => self.cumulative = Some(*usage),
            Observed::RateLimits(usage) => self.rate_limits = Some(*usage),
            Observed::LimitReached { resets_at, message } => {
                self.limit = Some(LimitSignal {
                    resets_at: *resets_at,
                    message: message.clone(),
                });
            }
            Observed::TurnContext {
                model: Some(model), ..
            } => self.model = Some(model.clone()),
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

    /// Usage of the observed lines: the sum of distinct Claude messages and/or the last Codex
    /// cumulative value. All fields are `None` when nothing was observed.
    #[must_use]
    pub fn usage(&self) -> TokenUsage {
        let mut claude = self.finalized;
        if let Some((_, current)) = &self.current {
            claude = add_known(claude, *current);
        }
        match (claude, self.cumulative) {
            (Some(a), Some(b)) => a.checked_add(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => TokenUsage::UNKNOWN,
        }
    }

    /// Latest rate-limit snapshot, if any.
    #[must_use]
    pub fn rate_limits(&self) -> Option<&AccountUsage> {
        self.rate_limits.as_ref()
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
        assert_eq!(acc.rate_limits(), None);
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
    fn codex_cumulative_last_value_wins() {
        let mut acc = UsageAccumulator::new();
        let first = TokenUsage {
            input: Some(100),
            output: Some(10),
            cache_read: Some(0),
            cache_write: None,
        };
        let second = TokenUsage {
            input: Some(300),
            output: Some(40),
            ..first
        };
        acc.observe_all(&[
            Observed::CumulativeUsage(first),
            Observed::CumulativeUsage(second),
            Observed::Malformed,
            Observed::TurnComplete {
                duration_ms: Some(5),
            },
        ]);
        assert_eq!(acc.usage(), second);
        assert_eq!(acc.malformed_lines(), 1);
        assert_eq!(acc.turn_complete_signals(), 1);
    }

    #[test]
    fn keeps_latest_limit_and_rate_limits() {
        let mut acc = UsageAccumulator::new();
        let snapshot = AccountUsage {
            limit_reached: true,
            ..AccountUsage::default()
        };
        acc.observe(&Observed::RateLimits(snapshot));
        acc.observe(&Observed::LimitReached {
            resets_at: Some(Timestamp(5)),
            message: "limit".into(),
        });
        assert_eq!(acc.rate_limits(), Some(&snapshot));
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
