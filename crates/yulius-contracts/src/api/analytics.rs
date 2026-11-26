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
    /// Count per status wire name, with `done` for a running task whose goal is achieved and
    /// whose turn ended (TSK-014): its session stays open, but it is finished, not running.
    pub by_status: BTreeMap<String, u32>,
    /// Count per current task harness.
    pub by_harness: BTreeMap<String, u32>,
    /// Count per origin (`ui`, `automation`, `import`, `onboarding`, `agent`, `telegram`, `handoff`).
    pub by_origin: BTreeMap<String, u32>,
}

/// One UTC day of the range: tasks created, and the cohort's runs and switches that day
/// (SPEC-110 TUS-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayCount {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Tasks created that day.
    pub count: u32,
    /// Agent working time (ms) of the runs started that day; `null` when none reported it.
    #[serde(default)]
    pub active_ms: Option<u64>,
    /// Median agent working time (ms) of the tasks created that day whose runs reported it.
    #[serde(default)]
    pub median_active_ms: Option<u64>,
    /// Input tokens of the runs started that day; `null` when none reported them.
    #[serde(default)]
    pub input: Option<u64>,
    /// Output tokens of the runs started that day.
    #[serde(default)]
    pub output: Option<u64>,
    /// Subscription switches made that day.
    #[serde(default)]
    pub switches: u32,
}

/// Durations of the cohort, computed from attempts (wall: a session open, idle or not) and
/// metrics (active: the agent working, SPEC-087 MET-001).
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
    /// Median per-task active agent time (ms) over tasks whose runs reported it (SPEC-087
    /// MET-005); `null` when none did.
    #[serde(default)]
    pub median_active: Option<u64>,
    /// 90th percentile (nearest rank) of per-task active agent time (ms).
    #[serde(default)]
    pub p90_active: Option<u64>,
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

/// Usage and elapsed time of the runs executed by one harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessTokens {
    /// Executing harness.
    pub harness: String,
    /// Number of ended runs in the elapsed-time distribution.
    #[serde(default)]
    pub finished_runs: u32,
    /// Median wall milliseconds per ended run, including idle time.
    pub median_wall_ms: Option<u64>,
    /// Nearest-rank p90 wall milliseconds per ended run.
    pub p90_wall_ms: Option<u64>,
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
    /// Email of its Claude account (instance administrators only, UI-022; absent when unknown,
    /// deleted or none, SPEC-244).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
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
    /// `random` when the task's harness was drawn at creation, else `chosen` (SPEC-505 RND-003,
    /// SPEC-520 RHM-003).
    #[serde(default)]
    pub harness_choice: String,
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
    "harness_choice",
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

/// `GET /api/analytics/reviews`: the independent review's numbers over the pull requests merged
/// in the range (SPEC-493 REV-006).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReviewAnalytics {
    /// The range read.
    pub range: AnalyticsRange,
    /// Pull requests merged in it with at least one recorded review round.
    pub pull_requests: u32,
    /// Of those, the ones whose first round asked for changes.
    pub first_rounds_requesting_changes: u32,
    /// Of those, the ones whose first round never reported (cut off by a usage limit).
    pub first_rounds_without_report: u32,
    /// The median of the rounds per pull request; `null` without any.
    pub median_rounds: Option<f64>,
    /// Rounds by the model that ran them, most first.
    pub rounds_by_model: Vec<ModelCount>,
    /// Rounds run on Opus 5.5 because the subscription could not run Fable 5.2.
    pub no_fable_rounds: u32,
    /// Rounds with a verdict whose previous round asked for changes (SPEC-506 FST-007).
    #[serde(default)]
    pub re_reviews_after_changes: u32,
    /// Of those, the ones that asked for changes again.
    #[serde(default)]
    pub re_reviews_asking_again: u32,
    /// Rounds with a verdict whose previous round approved (re-approvals unless they asked).
    #[serde(default)]
    pub rounds_after_approval: u32,
    /// Of those, the ones that asked for changes.
    #[serde(default)]
    pub rounds_after_approval_asking: u32,
}

/// `GET /api/analytics/merge-queue`: the merge queue's candidates over the range, per day, and the
/// minutes from arming a merge to the merge (SPEC-493 MQC-005).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MergeQueueAnalytics {
    /// The range read.
    pub range: AnalyticsRange,
    /// Per UTC day, oldest first; days with nothing are left out.
    pub days: Vec<MergeQueueDay>,
    /// Candidates formed, merged and dissolved over the range.
    pub formed: u32,
    /// See `formed`.
    pub merged: u32,
    /// See `formed`.
    pub dissolved: u32,
    /// Dissolved candidates by why (`member_left`, `member_failed`, `bisected`, `base_moved`,
    /// `errored`, `too_long`, `gate_red`), over the range.
    pub dissolved_by: std::collections::BTreeMap<String, u32>,
    /// The last ten candidates dissolved in the range, newest first, with why and the whole
    /// reason (SPEC-497 MQW-004).
    #[serde(default)]
    pub dissolved_recent: Vec<MergeQueueDissolved>,
    /// Pull requests merged after an armed merge.
    pub merges: u32,
    /// The median minutes from a merge's last arming to the merge; `null` without any.
    pub median_arm_to_merge_minutes: Option<f64>,
    /// CI minutes over the range by how the runs ended (SPEC-497 OFC-003).
    #[serde(default)]
    pub ci_minutes: CiMinutes,
    /// The same per UTC day of the runs' start, oldest first; days with no run are left out.
    #[serde(default)]
    pub ci_minutes_by_day: Vec<CiMinutesDay>,
    /// Minutes over the range in which a sandbox's CI ran nothing because of its configuration
    /// (an invalid `yulius.toml` with no valid one before it; SPEC-504 CFG-004), summed over the
    /// sandboxes read.
    #[serde(default)]
    pub config_blocked_minutes: u64,
}

/// A dissolved merge candidate (SPEC-497 MQW-004).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MergeQueueDissolved {
    /// When (RFC 3339).
    pub at: String,
    /// The sandbox's slug.
    pub sandbox: String,
    /// Why, as `dissolved_by` names it.
    pub why: String,
    /// The reason as the queue recorded it (the candidate's pull requests, its run, why).
    pub reason: String,
}

/// CI minutes by how the runs ended (SPEC-497 OFC-003): run minutes (start to end) of runs that
/// passed, failed, failed only for the infrastructure and were cancelled; and job minutes thrown
/// away by infrastructure failures that were run again.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CiMinutes {
    /// Runs that passed.
    pub passed: f64,
    /// Runs that failed for the change (or a reason not told apart).
    pub failed: f64,
    /// Runs that failed only for the infrastructure.
    pub failed_outside: f64,
    /// Runs that were cancelled.
    pub cancelled: f64,
    /// Job minutes of runs thrown away for the infrastructure (inside runs of any outcome).
    pub rerun: f64,
    /// `(failed_outside + cancelled) / all four`; `null` without any run. The target: 0.15.
    pub outside_share: Option<f64>,
}

/// `GET /api/analytics/ci-load`: the host's load from CI per UTC hour (SPEC-497): job-minutes CI
/// jobs waited to start, the host's CPUs busy, and the CPUs running jobs reserved and measured.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CiLoadAnalytics {
    /// The range read.
    pub range: AnalyticsRange,
    /// Per hour, oldest first; hours with no reading are left out.
    pub hours: Vec<CiLoadHour>,
    /// Job-minutes waited over the range.
    pub wait_job_minutes: u32,
    /// The most job-minutes waited in one hour.
    pub peak_wait_job_minutes: u32,
    /// The host's CPUs busy, percent, averaged over the hours; `null` without a reading.
    pub busy_percent: Option<f64>,
}

/// One hour of [`CiLoadAnalytics`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CiLoadHour {
    /// `YYYY-MM-DDTHH` (UTC).
    pub hour: String,
    /// Minutes read.
    pub minutes: u32,
    /// Job-minutes CI jobs waited to start.
    pub wait_job_minutes: u32,
    /// The host's CPUs busy, percent; `null` when not read.
    pub busy_percent: Option<f64>,
    /// CPUs the running CI jobs reserved, averaged.
    pub held_cpus: f64,
    /// CPUs their sandboxes' CI and engine groups used, averaged; `null` when not measured.
    pub measured_cpus: Option<f64>,
}

/// One day's [`CiMinutes`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CiMinutesDay {
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Its minutes.
    pub minutes: CiMinutes,
}

/// `GET /api/analytics/docker`: what the sandboxes' Docker fronts needed over the range, per week
/// (SPEC-494 DFJ-013).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DockerAnalytics {
    /// The range read.
    pub range: AnalyticsRange,
    /// Per week (from its Monday, UTC), oldest first; weeks with nothing are left out.
    pub weeks: Vec<DockerWeek>,
    /// Fronts restarted (by a repair, a person's ask or an update) over the range.
    pub restarts: u64,
    /// Connections that gave way to a new one at a front's limit over the range.
    pub gave_way: u64,
    /// Needs you naming the sandbox's Docker over the range.
    pub needs_you: u64,
    /// Containers CI jobs left that Yulius removed over the range (SPEC-504 CLB-006).
    #[serde(default)]
    pub ci_containers: u64,
    /// Networks CI jobs left that Yulius removed over the range.
    #[serde(default)]
    pub ci_networks: u64,
    /// Of those containers and networks, the ones the upkeep's sweep found (after a restart, or
    /// with the engine stopped at the job's end) rather than the job's end.
    #[serde(default)]
    pub ci_swept: u64,
}

/// One week of [`DockerAnalytics`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DockerWeek {
    /// Its Monday, `YYYY-MM-DD` (UTC).
    pub week: String,
    /// See [`DockerAnalytics::restarts`].
    pub restarts: u64,
    /// See [`DockerAnalytics::gave_way`].
    pub gave_way: u64,
    /// See [`DockerAnalytics::needs_you`].
    pub needs_you: u64,
    /// See [`DockerAnalytics::ci_containers`].
    #[serde(default)]
    pub ci_containers: u64,
    /// See [`DockerAnalytics::ci_networks`].
    #[serde(default)]
    pub ci_networks: u64,
    /// See [`DockerAnalytics::ci_swept`].
    #[serde(default)]
    pub ci_swept: u64,
}

/// One day of [`MergeQueueAnalytics`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MergeQueueDay {
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Candidates formed.
    pub formed: u32,
    /// Candidates whose members all merged.
    pub merged: u32,
    /// Candidates dissolved, by why.
    pub dissolved: std::collections::BTreeMap<String, u32>,
}

/// A model and how many review rounds it ran.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelCount {
    /// The model's id.
    pub model: String,
    /// Rounds.
    pub rounds: u32,
}
