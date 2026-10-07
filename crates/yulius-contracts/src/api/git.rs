//! Git overview and delivery.

use serde::{Deserialize, Serialize};

/// Pull request view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullView {
    /// Number.
    pub number: u64,
    /// URL.
    pub url: String,
    /// Title.
    pub title: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    /// Base branch.
    pub base: String,
    /// Head branch.
    pub head: String,
    /// `none`, `pending`, `success` or `failure`.
    pub checks: String,
}

/// Per-task branch facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBranch {
    /// Task id.
    pub task_id: String,
    /// Task title.
    pub title: String,
    /// Branch.
    pub branch: String,
    /// Commits ahead of integration.
    pub ahead: u32,
    /// Commits behind integration.
    pub behind: u32,
    /// Files changed vs integration.
    pub files_changed: u32,
    /// Inserted lines.
    pub insertions: u32,
    /// Deleted lines.
    pub deletions: u32,
    /// Open PR.
    pub pr: Option<PullView>,
}

/// Commit summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitView {
    /// SHA.
    pub sha: String,
    /// Subject.
    pub subject: String,
    /// Author.
    pub author: String,
    /// Time.
    pub at: String,
}

/// `GET /api/sandboxes/{sid}/git`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitOverview {
    /// Integration branch.
    pub integration_branch: String,
    /// Production branch.
    pub production_branch: String,
    /// Default branch.
    pub default_branch: String,
    /// Task branches.
    pub tasks: Vec<TaskBranch>,
    /// Recent integration commits.
    pub recent_commits: Vec<CommitView>,
    /// Open pull requests.
    pub pulls: Vec<PullView>,
    /// Problems reading Git or GitHub; the lists above may then be partial or empty.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Delivery state of a task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Delivery {
    /// `none`, `pr_open`, `merged_integration`, `env_done`, `promoted`, `production_done`.
    pub stage: String,
    /// The environment that is done when `stage` is `env_done`.
    #[serde(default)]
    pub environment: Option<String>,
    /// PR, if any.
    pub pr: Option<PullView>,
    /// Registered (or discovered) repository-relative spec path.
    #[serde(default)]
    pub spec_path: Option<String>,
    /// Spec validity.
    pub spec_valid: bool,
    /// Spec problems.
    pub spec_problems: Vec<String>,
    /// Open steps.
    pub steps_open: u32,
    /// Allowed next actions (`open_pr`, `merge`, `run_env:<name>`, `confirm_env:<name>`,
    /// `promote`).
    pub next_actions: Vec<String>,
    /// Actions of the current stage that are refused right now, with the reason.
    #[serde(default)]
    pub blocked: Vec<BlockedAction>,
    /// Problems reading Git or GitHub, and policy outcomes (for example a failed automatic
    /// promotion).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A delivery action that applies at the current stage but is refused now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedAction {
    /// Action wire name (as in `next_actions`).
    pub action: String,
    /// Why it is refused.
    pub reason: String,
}

/// `POST /api/tasks/{tid}/delivery/pr`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OpenPr {
    /// Title override.
    #[serde(default)]
    pub title: Option<String>,
    /// Body override.
    #[serde(default)]
    pub body: Option<String>,
}

/// `POST …/environments/{env}/runs` and `POST …/environments/{env}/confirm`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunEnvironment {
    /// Explicit confirmation (deployments are outward-facing); required on every run.
    #[serde(default)]
    pub confirm: bool,
}

/// Environment run status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentRun {
    /// Run id.
    pub run_id: String,
    /// Environment name.
    #[serde(default)]
    pub environment: String,
    /// `command` (the environment command ran) or `manual` (a person confirmed it).
    #[serde(default)]
    pub kind: String,
    /// `running`, `succeeded`, `failed` (non-zero exit, signal or timeout) or `error` (could
    /// not start, or interrupted by a server restart).
    pub state: String,
    /// The command that ran (null for manual confirmations).
    #[serde(default)]
    pub command: Option<String>,
    /// Commit the command ran against.
    #[serde(default)]
    pub commit: Option<String>,
    /// Who requested it (login).
    #[serde(default)]
    pub requested_by: String,
    /// Captured output: the last 1 MiB of combined stdout/stderr plus Yulius notes.
    pub output: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Start time.
    #[serde(default)]
    pub started_at: String,
    /// End time.
    #[serde(default)]
    pub ended_at: Option<String>,
}

/// Promotion result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteResult {
    /// `promoted` or `pr_opened`.
    pub result: String,
    /// Resulting SHA or PR URL.
    pub sha: Option<String>,
}
