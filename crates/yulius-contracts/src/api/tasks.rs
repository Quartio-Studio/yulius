//! Tasks, steps, messages, metrics and feedback.

use serde::{Deserialize, Serialize};

use super::common::UserRef;
use super::git::Delivery;

/// Step progress summary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StepProgress {
    /// Completed steps.
    pub done: u32,
    /// All steps.
    pub total: u32,
    /// Current step title.
    pub current: Option<String>,
}

/// A task in lists and events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// Task id.
    pub id: String,
    /// Sandbox id.
    pub sandbox_id: String,
    /// Number within the sandbox.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Harness wire name (`claude`; Yulius runs Claude Code only).
    pub harness: String,
    /// Status wire name (domain `TaskStatus::as_str`).
    pub status: String,
    /// `working`, `waiting`, `needs_attention`, or null.
    pub activity: Option<String>,
    /// Task branch.
    pub branch: Option<String>,
    /// `ui`, `automation`, `import`, `onboarding` or `agent`.
    pub origin: String,
    /// Creator.
    pub created_by: UserRef,
    /// Creation time.
    pub created_at: String,
    /// Last change.
    pub updated_at: String,
    /// Step progress.
    pub steps: StepProgress,
    /// Unread messages for the task.
    pub unread_messages: u32,
    /// Archived flag.
    pub archived: bool,
    /// Label of the account in use.
    pub account_label: Option<String>,
    /// `goal` or `prompt` (TSK-010).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// While `waiting_capacity` (every account at its usage limit): when the earliest known limit
    /// resets and the task resumes, RFC 3339 (ROT-010). Null otherwise or when unknown.
    #[serde(default)]
    pub resumes_at: Option<String>,
}

fn default_mode() -> String {
    "prompt".into()
}

fn default_harness() -> String {
    "claude".into()
}

/// `POST /api/sandboxes/{sid}/tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTask {
    /// Initial prompt.
    pub prompt: String,
    /// Harness: `claude` (the default and the only one supported).
    #[serde(default = "default_harness")]
    pub harness: String,
    /// Workflow to seed steps/instructions.
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Effort override.
    #[serde(default)]
    pub effort: Option<String>,
    /// Title (derived from the prompt when absent).
    #[serde(default)]
    pub title: Option<String>,
    /// `goal` (default) or `prompt` (TSK-010).
    #[serde(default)]
    pub mode: Option<String>,
}

/// `POST /api/tasks/{tid}/continue` and `/switch`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContinueTask {
    /// Target account (another Claude account); the best one when absent.
    #[serde(default)]
    pub account_id: Option<String>,
}

/// One process run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptView {
    /// Sequence within the task.
    pub seq: u32,
    /// Harness.
    pub harness: String,
    /// Account label.
    pub account_label: Option<String>,
    /// `new` or `resume`.
    pub launch_kind: String,
    /// Attempt state wire name.
    pub state: String,
    /// Start time.
    pub started_at: String,
    /// End time.
    pub ended_at: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Exit reason.
    pub exit_reason: Option<String>,
}

/// A task step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepView {
    /// Step id.
    pub id: String,
    /// Position.
    pub ordinal: u32,
    /// Title.
    pub title: String,
    /// `pending`, `in_progress`, `done`, `skipped` or `blocked`.
    pub status: String,
    /// Note.
    pub note: Option<String>,
    /// Last actor.
    pub updated_by: Option<String>,
    /// Last change time.
    pub updated_at: Option<String>,
}

/// Token and duration metrics; `None` means unknown, never zero.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Wall-clock milliseconds across runs.
    pub wall_ms: u64,
    /// Active (working) milliseconds.
    pub active_ms: Option<u64>,
    /// Input tokens.
    pub input_tokens: Option<u64>,
    /// Output tokens.
    pub output_tokens: Option<u64>,
    /// Cache-read tokens.
    pub cache_read_tokens: Option<u64>,
    /// Cache-write tokens.
    pub cache_write_tokens: Option<u64>,
    /// Models used.
    pub models: Vec<String>,
    /// Account/provider switches.
    pub switches: u32,
    /// Completed turns.
    pub turns: Option<u32>,
}

/// Feedback entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackView {
    /// Author login.
    pub login: String,
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Comment.
    pub comment: String,
    /// Time.
    pub at: String,
}

/// A recorded switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchView {
    /// Time.
    pub at: String,
    /// Reason code.
    pub reason: String,
    /// From (harness/account label).
    pub from: String,
    /// To (harness/account label).
    pub to: String,
}

/// `GET /api/tasks/{tid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetail {
    /// Summary fields.
    #[serde(flatten)]
    pub summary: TaskSummary,
    /// Registered spec path.
    pub spec_path: Option<String>,
    /// Worktree path on the instance.
    pub worktree: Option<String>,
    /// Runs.
    pub attempts: Vec<AttemptView>,
    /// Steps.
    pub steps_list: Vec<StepView>,
    /// Metrics.
    pub metrics: Metrics,
    /// Feedback.
    pub feedback: Vec<FeedbackView>,
    /// Delivery state.
    pub delivery: Delivery,
    /// Switch history.
    pub switches: Vec<SwitchView>,
}

/// `POST /api/tasks/{tid}/feedback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackRequest {
    /// `correct` or `incorrect`.
    pub verdict: String,
    /// Optional comment.
    #[serde(default)]
    pub comment: String,
}

/// Message sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFrom {
    /// Sending task.
    pub task_id: Option<String>,
    /// Sending user.
    pub login: Option<String>,
    /// Sender title (task title or user display name).
    pub title: String,
}

/// An inter-task or user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    /// Message id.
    pub id: String,
    /// Sender.
    pub from: MessageFrom,
    /// Recipient task; null = broadcast.
    pub to_task_id: Option<String>,
    /// `info`, `question`, `request` or `scope`.
    pub kind: String,
    /// Subject.
    pub subject: String,
    /// Body.
    pub body: String,
    /// Creation time.
    pub created_at: String,
    /// Read by the recipient.
    pub read: bool,
}

/// `POST /api/tasks/{tid}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMessage {
    /// Body.
    pub body: String,
}
