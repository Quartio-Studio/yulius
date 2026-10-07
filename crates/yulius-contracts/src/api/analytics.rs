//! Analytics aggregates (SPEC-001 ANL-001..003).

use serde::{Deserialize, Serialize};

/// Aggregate by a key (model, harness, account).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakdown {
    /// Key.
    pub key: String,
    /// Tasks.
    pub tasks: u32,
    /// Input tokens (known only).
    pub input_tokens: u64,
    /// Output tokens (known only).
    pub output_tokens: u64,
    /// Attempts whose usage was unknown.
    pub unknown_usage_runs: u32,
}

/// `GET /api/analytics/summary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsSummary {
    /// Tasks created in range.
    pub tasks_created: u32,
    /// Tasks closed in range.
    pub tasks_closed: u32,
    /// Median wall duration of closed tasks (ms).
    pub median_wall_ms: Option<u64>,
    /// Median active duration (ms).
    pub median_active_ms: Option<u64>,
    /// Switches in range.
    pub switches: u32,
    /// Feedback counts.
    pub feedback_correct: u32,
    /// Feedback counts.
    pub feedback_incorrect: u32,
    /// By model.
    pub by_model: Vec<Breakdown>,
    /// By harness.
    pub by_harness: Vec<Breakdown>,
    /// Tasks per day (`YYYY-MM-DD`, count).
    pub per_day: Vec<(String, u32)>,
}

/// One analytics row per task (`format=json`).
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
    /// Harness of the first run.
    pub harness: String,
    /// Models used.
    pub models: String,
    /// Created.
    pub created_at: String,
    /// Closed.
    pub closed_at: Option<String>,
    /// Wall ms.
    pub wall_ms: u64,
    /// Active ms.
    pub active_ms: Option<u64>,
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Switches.
    pub switches: u32,
    /// Latest feedback verdict.
    pub feedback: Option<String>,
    /// Origin.
    pub origin: String,
}
