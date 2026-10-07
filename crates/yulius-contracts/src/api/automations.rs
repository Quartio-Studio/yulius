//! Automations (scheduled task creation).

use serde::{Deserialize, Serialize};

/// Last run summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRunView {
    /// Scheduled time.
    pub scheduled_for: String,
    /// `task_created`, `skipped_previous_active`, `skipped_disabled` or `failed`.
    pub state: String,
    /// Created task.
    pub task_id: Option<String>,
    /// Error text.
    pub error: Option<String>,
}

/// An automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    /// Id (ignored on create).
    #[serde(default)]
    pub id: String,
    /// Name.
    pub name: String,
    /// Five-field cron expression.
    pub cron: String,
    /// Fixed UTC offset used to evaluate the schedule.
    #[serde(default)]
    pub utc_offset_minutes: i32,
    /// Prompt of each created task.
    pub prompt: String,
    /// Workflow for created tasks.
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// `claude` or `codex`.
    pub harness: String,
    /// Enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `ui` or `agent` (read-only).
    #[serde(default)]
    pub created_via: String,
    /// Next scheduled run (read-only).
    #[serde(default)]
    pub next_run_at: Option<String>,
    /// Last run (read-only).
    #[serde(default)]
    pub last_run: Option<AutomationRunView>,
}

fn default_true() -> bool {
    true
}
