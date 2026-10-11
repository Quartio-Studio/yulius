//! UI event stream messages (`WS /api/events`, SPEC-002 §7.8).

use serde::{Deserialize, Serialize};

use crate::api::admin::AccountView;
use crate::api::sandboxes::Sandbox;
use crate::api::tasks::{MessageView, TaskSummary};

/// One server-pushed event. Delivery is best effort; on `Resync` clients refetch lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UiEvent {
    /// A task changed.
    #[serde(rename = "task.updated")]
    TaskUpdated {
        /// New summary.
        task: TaskSummary,
    },
    /// A task disappeared from the visible set.
    #[serde(rename = "task.removed")]
    TaskRemoved {
        /// Task id.
        task_id: String,
    },
    /// Steps changed.
    #[serde(rename = "steps.updated")]
    StepsUpdated {
        /// Task id.
        task_id: String,
    },
    /// A message was posted to a task.
    #[serde(rename = "message")]
    Message {
        /// Recipient task (or sender for broadcasts).
        task_id: String,
        /// The message.
        message: MessageView,
    },
    /// A sandbox changed.
    #[serde(rename = "sandbox.updated")]
    SandboxUpdated {
        /// New sandbox view.
        sandbox: Sandbox,
    },
    /// An agent asked a person to act in the browser.
    #[serde(rename = "browser.action_requested")]
    BrowserActionRequested {
        /// Sandbox id.
        sandbox_id: String,
        /// Tab id.
        tab_id: String,
        /// Reason text.
        reason: String,
        /// Requesting task.
        task_id: Option<String>,
    },
    /// An automation ran.
    #[serde(rename = "automation.run")]
    AutomationRun {
        /// Automation id.
        automation_id: String,
        /// Run state.
        state: String,
        /// Created task.
        task_id: Option<String>,
    },
    /// An account changed.
    #[serde(rename = "account.updated")]
    AccountUpdated {
        /// New account view.
        account: AccountView,
    },
    /// An instance update changed (a new `main` was found, or a run moved): clients refetch
    /// `GET /api/update` (SPEC-002 ADR-18). `phase` is the running update's phase, if any.
    #[serde(rename = "update.updated")]
    UpdateChanged {
        /// `building`, `pausing`, `restarting`, `resuming`, or `None` when no update runs.
        phase: Option<String>,
    },
    /// Events were lost; refetch.
    #[serde(rename = "resync")]
    Resync,
}
