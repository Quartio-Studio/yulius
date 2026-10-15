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
    /// Harness: `claude` (the default and the only one supported).
    #[serde(default = "default_harness")]
    pub harness: String,
    /// Enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Created tasks are supertasks (SPEC-078, SPEC-071). Not together with `after_task_id`,
    /// `start_rule` or `prepared`; none of the four: created tasks start now (queued).
    #[serde(default)]
    pub supertask: bool,
    /// Created tasks run after this task of the project (SPEC-078, TSK-012); once it has ended,
    /// they wait for the rule `#N done`. Not together with `start_rule`.
    #[serde(default)]
    pub after_task_id: Option<String>,
    /// Number of `after_task_id` (read-only).
    #[serde(default)]
    pub after_task_number: Option<u32>,
    /// Created tasks start when this rule over the project's tasks is true (SPEC-078, SPEC-017),
    /// stored canonically, e.g. `#12 done and #13 merged`.
    #[serde(default)]
    pub start_rule: Option<String>,
    /// Created tasks wait `prepared` until a person presses Start now, "When I decide" (SPEC-155,
    /// SPEC-105). Not together with the other three.
    #[serde(default)]
    pub prepared: bool,
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

/// Body of `POST /api/automations/{id}/run` (SPEC-093 AUT-023): the start choice of this run
/// only, as the editor shows it, checked like [`Automation`]'s; nothing is stored. Without a body
/// the run uses the automation's stored choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAutomation {
    /// The task is a supertask.
    #[serde(default)]
    pub supertask: bool,
    /// The task runs after this task of the project.
    #[serde(default)]
    pub after_task_id: Option<String>,
    /// The task starts when this rule is true.
    #[serde(default)]
    pub start_rule: Option<String>,
    /// The task waits, prepared, until a person starts it (SPEC-155).
    #[serde(default)]
    pub prepared: bool,
}

fn default_harness() -> String {
    "claude".into()
}

fn default_true() -> bool {
    true
}

/// A ready-made automation from Yulius's catalog (SPEC-042 AUT-007/008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTemplate {
    /// Catalog id (the file stem, e.g. `docs-up-to-date`).
    pub id: String,
    /// Name of the automation it creates.
    pub name: String,
    /// What it does, in one line.
    pub description: String,
    /// Five-field cron expression.
    pub cron: String,
    /// Fixed UTC offset used to evaluate the schedule.
    pub utc_offset_minutes: i32,
    /// Prompt of each created task.
    pub prompt: String,
    /// Workflow of created tasks.
    pub workflow_id: Option<String>,
    /// The project's automation with the same name (it counts as added), if any.
    pub automation_id: Option<String>,
}
