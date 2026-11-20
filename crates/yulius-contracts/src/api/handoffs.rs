//! Tasks hand work to another sandbox (SPEC-512 part 5, #1106, ADR-0395).

use serde::{Deserialize, Serialize};

/// `GET`/`PUT /api/admin/handoffs` (HOF-001): off by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffSetting {
    /// `off`, `ask` or `automatic`.
    pub mode: String,
    /// How deep a chain of handoffs goes (1 to 5).
    pub max_depth: u32,
    /// Handoffs per owner in 24 hours (1 to 100).
    pub per_day: u32,
}

impl Default for HandoffSetting {
    fn default() -> Self {
        Self {
            mode: "off".into(),
            max_depth: 3,
            per_day: 20,
        }
    }
}

/// `GET`/`PUT /api/sandboxes/{sid}/handoffs` (HOF-002): on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxHandoffs {
    /// Whether tasks of other sandboxes may hand work to this one.
    pub accept: bool,
}

/// One handoff, as its source task's owner sees it (HOF-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffView {
    /// Its id.
    pub id: String,
    /// The target sandbox's id and name.
    pub target_sandbox_id: String,
    /// The target sandbox's name.
    pub target_sandbox: String,
    /// The task's title.
    pub title: String,
    /// The task's prompt, exactly as it would be created.
    pub prompt: String,
    /// `task_create`'s options, as asked.
    pub harness: Option<String>,
    /// The model asked for.
    pub model: Option<String>,
    /// The effort asked for.
    pub effort: Option<String>,
    /// `goal` or `prompt`.
    pub mode: Option<String>,
    /// `requested`, `created`, `declined` or `refused`.
    pub state: String,
    /// Why it was refused or declined.
    pub reason: Option<String>,
    /// The task it made, when it did.
    pub target_number: Option<u32>,
    /// That task's page.
    pub target_link: Option<String>,
    /// When it was asked (RFC 3339).
    pub created_at: String,
}

/// Where a handed-off task came from (HOF-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffFrom {
    /// The source sandbox's name.
    pub sandbox: String,
    /// The source task's number.
    pub number: u32,
    /// The source task's page.
    pub link: String,
}

/// `GET /api/tasks/{tid}/handoffs`: what a task handed off and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskHandoffs {
    /// Where it was handed off from, if it was.
    pub from: Option<HandoffFrom>,
    /// Its own handoffs, newest first (the owner sees requests to answer).
    pub handoffs: Vec<HandoffView>,
}

/// `POST /api/tasks/{tid}/handoffs/{hid}` (HOF-004): the owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerHandoff {
    /// Approve (create it) or decline.
    pub approve: bool,
}
