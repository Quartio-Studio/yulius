//! UI event stream messages (`WS /api/events`, SPEC-002 §7.8).

use serde::{Deserialize, Serialize};

use crate::api::admin::AccountView;
use crate::api::notes::NoteSelection;
use crate::api::sandboxes::Sandbox;
use crate::api::tasks::TaskSummary;

/// One server-pushed event. Delivery is best effort; on `Resync` clients refetch lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UiEvent {
    /// A task changed.
    #[serde(rename = "task.updated")]
    TaskUpdated {
        /// New summary (boxed: the largest variant by far; the wire is the same).
        task: Box<TaskSummary>,
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
    /// A notepad was saved (SPEC-041): open notepads with nothing unsaved fetch the note again.
    /// A sandbox note's event reaches only the people who can view that sandbox, a personal
    /// note's only its owner (SPEC-103).
    #[serde(rename = "note.updated")]
    NoteUpdated {
        /// `team`, `sandbox` or `me`.
        scope: String,
        /// The sandbox of a sandbox note.
        sandbox_id: Option<String>,
        /// The stored revision.
        revision: u64,
        /// Who saved it.
        updated_by: String,
    },
    /// Someone's selection in a shared notepad (SPEC-112), relayed as they sent it to the people
    /// who may read that notepad: team notes to everyone signed in, a sandbox's to its viewers.
    /// `selection: null` means they left it; `hello` asks the others to send theirs.
    #[serde(rename = "note.presence")]
    NotePresence {
        /// `team` or `sandbox`.
        scope: String,
        /// The sandbox of a sandbox note.
        sandbox_id: Option<String>,
        /// Who: their user id (the browser leaves out its own).
        user_id: String,
        /// Their browser tab, as they sent it (one person may have several).
        client: String,
        /// Their name: the GitHub login, else the display name.
        name: String,
        /// The revision whose text the offsets are in.
        revision: u64,
        /// Their selection, or `None` when they left.
        selection: Option<NoteSelection>,
        /// They just opened it: the others answer with their own.
        hello: bool,
        /// Their tab's update number, as they sent it: an older one arriving late is dropped
        /// (SPEC-170 TKS-003). 0: not numbered.
        seq: u64,
    },
    /// Someone's selection in a file open in the editor (SPEC-127), relayed as they sent it to the
    /// people who may read the sandbox's files (its writers). `selection: null` means they left it;
    /// `hello` asks the others to send theirs.
    #[serde(rename = "file.presence")]
    FilePresence {
        /// The sandbox.
        sandbox_id: String,
        /// `sandbox` or `task:<task id>`.
        root: String,
        /// The file, relative to the root (normalized).
        path: String,
        /// Who: their user id (the browser leaves out its own).
        user_id: String,
        /// Their browser tab, as they sent it.
        client: String,
        /// Their name: the GitHub login, else the display name.
        name: String,
        /// SHA-256 hex of the saved text the offsets are in.
        sha256: String,
        /// Their selection, or `None` when they left.
        selection: Option<NoteSelection>,
        /// They just opened it: the others answer with their own.
        hello: bool,
        /// Their tab's update number, as they sent it: an older one arriving late is dropped
        /// (SPEC-170 TKS-003). 0: not numbered.
        seq: u64,
    },
    /// The sandbox's pull requests may have changed (a delivery stage moved, a Yulius CI run was
    /// queued, started or ended): watching clients read the Git overview again (SPEC-108).
    #[serde(rename = "pulls.updated")]
    PullsUpdated {
        /// Sandbox id.
        sandbox_id: String,
    },
    /// Events were lost; refetch.
    #[serde(rename = "resync")]
    Resync,
}
