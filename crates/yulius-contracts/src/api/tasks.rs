//! Tasks, steps, messages, metrics and feedback.

use serde::{Deserialize, Serialize};

use super::common::UserRef;
use super::git::Delivery;

/// Step progress summary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StepProgress {
    /// Finished steps (done or skipped).
    pub done: u32,
    /// All steps.
    pub total: u32,
    /// Current step title.
    pub current: Option<String>,
    /// Finished share of the plan's effort, 0-100, rounded down (STP-006): 9 of 10 equal steps
    /// is 90.
    #[serde(default)]
    pub percent: u8,
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
    /// `working`, `waiting`, `needs_attention`, `done` (the goal is achieved and the turn ended,
    /// TSK-014), or null.
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
    /// While the agent is idle (`waiting` or `done`): background shells it left running, as last
    /// probed (TSK-014). Null when not known or not idle.
    #[serde(default)]
    pub background_shells: Option<u32>,
    /// Since when those shells have been running, RFC 3339 (complaint 1: a wait that never
    /// ends shows its age). Null when none run or not known.
    #[serde(default)]
    pub background_shells_since: Option<String>,
    /// The task this one runs after (SPEC-001 TSK-012): while `chained` it waits for it; the
    /// link stays after the task started.
    #[serde(default)]
    pub after: Option<Box<ChainLink>>,
    /// Tasks chained to run after this one.
    #[serde(default)]
    pub next: Vec<ChainLink>,
    /// A prompt typed into the session that Yulius held for a decision (TSK-013).
    #[serde(default)]
    pub held_prompt: Option<Box<HeldPrompt>>,
    /// How "Auto" chose the task's mode and workflow when it was created (TSK-015); null when the
    /// person picked both.
    #[serde(default)]
    pub auto: Option<AutoChoice>,
}

/// How "Auto" chose a new task's mode and workflow (SPEC-001 TSK-015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoChoice {
    /// How the mode was chosen: `jev` (Jev was confident), `default` (Goal: Jev not configured,
    /// failing or unsure), or null when the person picked it.
    pub mode: Option<String>,
    /// How the workflow was chosen: `jev`, `default` ("Feature" or none), or null when the person
    /// picked it.
    pub workflow: Option<String>,
    /// The task's workflow when it was created; null without one.
    pub workflow_name: Option<String>,
}

/// One end of a chain (SPEC-001 TSK-012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLink {
    /// Task id; null when the predecessor was deleted.
    pub task_id: Option<String>,
    /// Task number.
    pub number: u32,
    /// Title; null when deleted.
    pub title: Option<String>,
    /// Status wire name; null when deleted.
    pub status: Option<String>,
    /// On a waiting chained task: why it needs a person instead of starting by itself (its
    /// predecessor stopped, failed, was cancelled, closed before it was done or was deleted).
    #[serde(default)]
    pub blocked: Option<String>,
}

/// A prompt held by the scope check (SPEC-001 TSK-013).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldPrompt {
    /// Hold id.
    pub id: String,
    /// `new_scope`, `drops_scope` or `redirect`.
    pub class: String,
    /// The prompt (at most 2,000 characters; the full text stays on the server).
    pub excerpt: String,
    /// Jev's probability of the class, in percent.
    pub percent: u32,
    /// Login of the person who typed it, when known.
    pub typed_by: Option<String>,
    /// When it was held.
    pub created_at: String,
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
    /// Workflow to seed steps/instructions; `auto` lets Jev choose one or none (TSK-015).
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
    /// `goal` (default), `prompt` (TSK-010) or `auto`: Jev chooses (TSK-015).
    #[serde(default)]
    pub mode: Option<String>,
    /// Run after this task of the same sandbox (SPEC-001 TSK-012): the new task waits as
    /// `chained` until that task is Done.
    #[serde(default)]
    pub after_task_id: Option<String>,
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
    /// Declared effort, 1-3 (STP-006).
    #[serde(default = "one")]
    pub effort: u8,
}

fn one() -> u8 {
    1
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

/// A task's message: between tasks, an agent's message to people (`to="people"`, null
/// `to_task_id`), or a person's message stored before 2026-10-10 (people now talk to the agent in
/// its terminal).
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
