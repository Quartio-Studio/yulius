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
}

/// Delivery state of a task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Delivery {
    /// `none`, `pr_open`, `merged_integration`, `env_done`, `promoted`, `production_done`.
    pub stage: String,
    /// PR, if any.
    pub pr: Option<PullView>,
    /// Spec validity.
    pub spec_valid: bool,
    /// Spec problems.
    pub spec_problems: Vec<String>,
    /// Open steps.
    pub steps_open: u32,
    /// Allowed next actions (`open_pr`, `merge`, `run_env:<name>`, `promote`).
    pub next_actions: Vec<String>,
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

/// `POST …/environments/{env}/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEnvironment {
    /// Explicit confirmation (deployments are outward-facing).
    pub confirm: bool,
}

/// Environment run status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentRun {
    /// Run id.
    pub run_id: String,
    /// `running`, `succeeded`, `failed`.
    pub state: String,
    /// Captured output (bounded).
    pub output: String,
    /// Exit code.
    pub exit_code: Option<i32>,
}

/// Promotion result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteResult {
    /// `promoted` or `pr_opened`.
    pub result: String,
    /// Resulting SHA or PR URL.
    pub sha: Option<String>,
}
