//! Analytics aggregates and per-task export rows (SPEC-001 ANL-001..003, docs/api.md).
//!
//! Unknown usage is never zero: token fields are `null` when no run in the group reported them,
//! and runs without usage are counted in `unknown_usage_runs` instead of being added as zero.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The filter a summary was computed for (echo of the query).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AnalyticsRange {
    /// Sandbox id, or `null` for all visible sandboxes.
    pub sandbox: Option<String>,
    /// First day (`YYYY-MM-DD`, UTC, inclusive).
    pub from: Option<String>,
    /// Last day (`YYYY-MM-DD`, UTC, inclusive).
    pub to: Option<String>,
}

/// Task counts of the cohort (tasks created in range).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskCounts {
    /// Tasks created in range.
    pub total: u32,
    /// Of those, tasks that are closed now.
    pub closed: u32,
    /// Count per status wire name.
    pub by_status: BTreeMap<String, u32>,
    /// Count per current harness (`claude`).
    pub by_harness: BTreeMap<String, u32>,
    /// Count per origin (`ui`, `automation`, `import`, `onboarding`, `agent`).
    pub by_origin: BTreeMap<String, u32>,
}

/// Tasks created on one UTC day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayCount {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Tasks created that day.
    pub count: u32,
}

/// Durations of the cohort, computed from attempts (wall) and metrics (active).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DurationStats {
    /// Median per-task wall time (ms) over tasks with at least one attempt.
    pub median: Option<u64>,
    /// 90th percentile (nearest rank) of per-task wall time (ms).
    pub p90: Option<u64>,
    /// Total wall time of all attempts (ms).
    pub total_wall: u64,
    /// Total active agent time (ms) of runs that reported it; `null` when none did.
    pub total_active: Option<u64>,
}

/// Token totals over a group of runs (attempts).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenTotals {
    /// Runs in the group.
    pub runs: u32,
    /// Distinct tasks in the group.
    pub tasks: u32,
    /// Sum of known uncached input tokens.
    pub input: Option<u64>,
    /// Sum of known output tokens.
    pub output: Option<u64>,
    /// Sum of known cache-read tokens.
    pub cache_read: Option<u64>,
    /// Sum of known cache-write tokens.
    pub cache_write: Option<u64>,
    /// Runs that reported no token usage at all.
    pub unknown_usage_runs: u32,
}

/// Tokens of the runs that used one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokens {
    /// Model name (`unknown` when the run reported none).
    pub model: String,
    /// Totals.
    #[serde(flatten)]
    pub tokens: TokenTotals,
}

/// Tokens of the runs of one harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessTokens {
    /// Harness (`claude`).
    pub harness: String,
    /// Totals.
    #[serde(flatten)]
    pub tokens: TokenTotals,
}

/// Tokens of the runs that used one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountTokens {
    /// Account id (`null` for runs without an account).
    pub account_id: Option<String>,
    /// Account label (`null` when the account was deleted or the run had none).
    pub account_label: Option<String>,
    /// Totals.
    #[serde(flatten)]
    pub tokens: TokenTotals,
}

/// Account switches of the cohort.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SwitchCounts {
    /// Recorded switch decisions (each moves a task to another account).
    pub total: u32,
}

/// Feedback verdicts of the cohort.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FeedbackCounts {
    /// `correct` verdicts.
    pub correct: u32,
    /// `incorrect` verdicts.
    pub incorrect: u32,
}

/// `GET /api/analytics/summary`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AnalyticsSummary {
    /// The filter.
    pub range: AnalyticsRange,
    /// Task counts.
    pub tasks: TaskCounts,
    /// Tasks created per day.
    pub tasks_per_day: Vec<DayCount>,
    /// Durations.
    pub duration_ms: DurationStats,
    /// Token totals over all runs.
    pub tokens: TokenTotals,
    /// Per model.
    pub tokens_by_model: Vec<ModelTokens>,
    /// Per harness.
    pub tokens_by_harness: Vec<HarnessTokens>,
    /// Per account.
    pub tokens_by_account: Vec<AccountTokens>,
    /// Switches.
    pub switches: SwitchCounts,
    /// Feedback.
    pub feedback: FeedbackCounts,
    /// `true` when the cohort exceeded the row bound and was cut (newest tasks kept).
    pub truncated: bool,
}

/// One analytics row per task (`/api/analytics/tasks?format=json`; CSV uses the same columns).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    /// Task id.
    pub task_id: String,
    /// Sandbox slug.
    pub sandbox: String,
    /// Number.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Status wire name.
    pub status: String,
    /// Origin.
    pub origin: String,
    /// Harness of the first run (current harness when the task never ran).
    pub harness: String,
    /// Harnesses used, in order of first use, `;`-separated.
    pub harnesses: String,
    /// Models used, `;`-separated.
    pub models: String,
    /// Account labels used, `;`-separated.
    pub accounts: String,
    /// Created.
    pub created_at: String,
    /// Closed.
    pub closed_at: Option<String>,
    /// Attempts (runs).
    pub attempts: u32,
    /// Wall ms over all attempts.
    pub wall_ms: u64,
    /// Active ms (known runs only).
    pub active_ms: Option<u64>,
    /// Input tokens (known runs only).
    pub input_tokens: Option<u64>,
    /// Output tokens (known runs only).
    pub output_tokens: Option<u64>,
    /// Cache-read tokens (known runs only).
    pub cache_read_tokens: Option<u64>,
    /// Cache-write tokens (known runs only).
    pub cache_write_tokens: Option<u64>,
    /// Runs without any usage.
    pub unknown_usage_runs: u32,
    /// Switches.
    pub switches: u32,
    /// Latest feedback verdict.
    pub feedback: Option<String>,
    /// `correct` verdicts.
    pub feedback_correct: u32,
    /// `incorrect` verdicts.
    pub feedback_incorrect: u32,
}

/// CSV header matching [`TaskRow`] field order.
pub const TASK_ROW_COLUMNS: &[&str] = &[
    "task_id",
    "sandbox",
    "number",
    "title",
    "status",
    "origin",
    "harness",
    "harnesses",
    "models",
    "accounts",
    "created_at",
    "closed_at",
    "attempts",
    "wall_ms",
    "active_ms",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "unknown_usage_runs",
    "switches",
    "feedback",
    "feedback_correct",
    "feedback_incorrect",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattened_breakdowns_and_null_unknowns() -> Result<(), serde_json::Error> {
        let row = ModelTokens {
            model: "m".into(),
            tokens: TokenTotals {
                runs: 2,
                tasks: 1,
                input: Some(5),
                unknown_usage_runs: 1,
                ..TokenTotals::default()
            },
        };
        let v = serde_json::to_value(&row)?;
        assert_eq!(v["model"], "m");
        assert_eq!(v["input"], 5);
        assert!(v["output"].is_null());
        assert_eq!(v["unknown_usage_runs"], 1);
        let back: ModelTokens = serde_json::from_value(v)?;
        assert_eq!(back, row);
        Ok(())
    }
}
