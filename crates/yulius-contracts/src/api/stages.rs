//! Stages of tasks, CI runs and CD runs (SPEC-137, docs/api.md "Stages").
//!
//! A span is one measured stage of a subject. Durations are milliseconds; a span still running
//! has no end and no duration. Unknown figures are `null`, never zero.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One span.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanView {
    /// `task`, `ci_run` or `cd_run`.
    pub subject_kind: String,
    /// The task's, CI run's or CD run's id.
    pub subject_id: String,
    /// Unique within the subject; a child's key starts with its parent's and `/`.
    pub key: String,
    /// The parent span's key.
    pub parent_key: Option<String>,
    /// What sort of stage: `state`, `step`, `run`, `pause`, `delivery`, `wait`, `update`,
    /// `queue`, `prepare`, `job`, `stage`, `promote`, `deploy`, `verify`.
    pub kind: String,
    /// Its name (a state, a step's title, a job's name, a stage a job printed).
    pub name: String,
    /// `dev` or `prod` (CD).
    pub environment: Option<String>,
    /// The CI job's definition name.
    pub job: Option<String>,
    /// Start (RFC 3339).
    pub started_at: String,
    /// End (RFC 3339), `null` while it runs.
    pub ended_at: Option<String>,
    /// End minus start.
    pub duration_ms: Option<i64>,
    /// How it ended (`passed`, `failed`, a task's next status, `requeued`…), when known.
    pub outcome: Option<String>,
    /// `recorded` as it happened, or `backfilled` from earlier records.
    pub source: String,
    /// Figures and facts: tokens, tests, reason, commit, trigger.
    pub detail: Value,
}

/// One subject's spans in start order (`GET …/stages/timeline`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineView {
    /// The subject's kind.
    pub subject_kind: String,
    /// The subject's id.
    pub subject_id: String,
    /// Its spans.
    pub spans: Vec<SpanView>,
    /// For a task: the CI runs of its pull request (its time waiting for CI).
    pub related: Vec<SpanView>,
    /// The subject has more spans than are read: these are the newest.
    pub truncated: bool,
}

/// Figures of one stage over the range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageRow {
    /// Subject kind.
    pub subject_kind: String,
    /// Span kind.
    pub kind: String,
    /// Span name (for a queue, also `queue · waiting for disk` and `queue · waiting for a lane`).
    pub name: String,
    /// The CI job, when the stage belongs to one.
    pub job: Option<String>,
    /// The CD environment, when the stage belongs to one.
    pub environment: Option<String>,
    /// Ended spans.
    pub count: u64,
    /// Of those, failed or errored.
    pub failed: u64,
    /// Spans still running.
    pub running: u64,
    /// Of the ended spans, backfilled from earlier records.
    pub backfilled: u64,
    /// Median duration.
    pub p50_ms: Option<i64>,
    /// 90th percentile.
    pub p90_ms: Option<i64>,
    /// Longest.
    pub max_ms: Option<i64>,
    /// Sum.
    pub total_ms: i64,
    /// Median CPU time of the ended spans that measured it (CI jobs, CD runs; SPEC-154).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_p50_ms: Option<i64>,
    /// 90th percentile of the CPU time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_p90_ms: Option<i64>,
    /// Median peak memory (MiB) of the ended spans that measured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_peak_p50_mb: Option<i64>,
    /// 90th percentile of the peak memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_peak_p90_mb: Option<i64>,
}

/// One UTC day of a stage's trend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Spans that ended that day.
    pub count: u64,
    /// Their median duration.
    pub p50_ms: Option<i64>,
    /// Their 90th percentile.
    pub p90_ms: Option<i64>,
}

/// The trend of the chosen stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageTrend {
    /// Subject kind of the stage.
    pub subject_kind: String,
    /// Span kind.
    pub kind: String,
    /// Span name.
    pub name: String,
    /// Every day of the range, oldest first.
    pub days: Vec<StageDay>,
}

/// An environment's delivery figures (deployment frequency, lead time, change failure rate,
/// time to restore).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryFiguresView {
    /// `dev` or `prod`.
    pub environment: String,
    /// Finished deployments in the range.
    pub deployments: u64,
    /// Of those, passed.
    pub passed: u64,
    /// Passed deployments per week.
    pub per_week: Option<f64>,
    /// From a merge into the integration branch to the end of the first passed deployment that
    /// started after it: median.
    pub lead_time_p50_ms: Option<i64>,
    /// Its 90th percentile.
    pub lead_time_p90_ms: Option<i64>,
    /// Merges measured.
    pub lead_time_count: u64,
    /// Failed over finished deployments.
    pub change_failure_rate: Option<f64>,
    /// From a failed deployment to the end of the next passed one: median.
    pub time_to_restore_p50_ms: Option<i64>,
    /// Restores measured.
    pub restores: u64,
}

/// What the filters can be set to (the values seen in the range).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StageOptions {
    /// CI jobs.
    pub jobs: Vec<String>,
    /// CD environments.
    pub environments: Vec<String>,
    /// Stage kinds.
    pub kinds: Vec<String>,
}

/// `GET …/stages/summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageSummary {
    /// First day (`YYYY-MM-DD`, UTC, inclusive).
    pub from: String,
    /// Last day (inclusive).
    pub to: String,
    /// Each stage's figures, most time first.
    pub stages: Vec<StageRow>,
    /// The chosen stage's daily p50 and p90 (the CI run by default).
    pub trend: StageTrend,
    /// Dev and production.
    pub delivery: Vec<DeliveryFiguresView>,
    /// Filter values.
    pub options: StageOptions,
    /// CI results reused from an identical tree: not runs, never in the figures.
    pub reused_runs: u64,
    /// The range has more spans than are read: the figures describe the newest.
    pub truncated: bool,
}
