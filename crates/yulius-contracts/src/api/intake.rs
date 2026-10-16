//! Task intake: split checks and proposals for new tasks, decisions on held prompts
//! (SPEC-001 TSK-011, TSK-013; docs/api.md "Task intake").

use serde::{Deserialize, Serialize};

use super::tasks::TaskSummary;

/// `POST /api/sandboxes/{sid}/intake/check` and `/intake/proposal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeRequest {
    /// The new task's prompt.
    pub prompt: String,
}

/// Answer of `/intake/check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeVerdict {
    /// `one` (create the task) or `several` (ask for a proposal).
    pub verdict: String,
    /// Whether Jev answered: false for prompts under 3 words, without a Jev key, or when Jev
    /// failed; the verdict is then `one` and `supertask` false. The split question needs 40
    /// words, so a shorter prompt is checked for breadth only.
    pub checked: bool,
    /// Whether to propose running the task as a supertask (SPEC-188): Jev is sure it is broad
    /// work, and the verdict is not `several`.
    pub supertask: bool,
}

/// One proposed task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedTaskView {
    /// Title, `Area: what`.
    pub title: String,
    /// Prompt carrying its items of the request word for word.
    pub prompt: String,
    /// Whether it must run after the previous task.
    pub after_previous: bool,
}

/// Answer of `/intake/proposal`: two to four tasks, or none ("keep it as one task").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitProposal {
    /// The proposed tasks; empty means keep one task.
    pub tasks: Vec<ProposedTaskView>,
}

/// `POST /api/tasks/{tid}/held-prompts/{hid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveHeldPrompt {
    /// `parallel` (start a new task now), `after` (a new task chained after this one), `send`
    /// (deliver it unchanged into the session), `skip_after` (mark `skip_step_ids` skipped and
    /// chain the new direction), `revert` (cancel and revert this task, start a new one; needs
    /// `confirm`) or `dismiss`.
    pub action: String,
    /// Steps to mark skipped (`skip_after`).
    #[serde(default)]
    pub skip_step_ids: Vec<String>,
    /// Explicit confirmation of the destructive `revert`.
    #[serde(default)]
    pub confirm: bool,
}

/// Result of a decision on a held prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldPromptOutcome {
    /// The task the prompt was typed into, after the decision.
    pub task: TaskSummary,
    /// The task created for the prompt, if any.
    pub created: Option<TaskSummary>,
    /// What happened, in one sentence (for example where reverted work is kept).
    pub note: String,
}
