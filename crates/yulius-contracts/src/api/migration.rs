//! Settings → Instance, card "Move to another host" (SPEC-470): what the page is told about a
//! move asked from it, and what it asks.

use serde::{Deserialize, Serialize};

use crate::migrate::MoveAction;
use crate::update::HostLine;

/// `GET /api/admin/instance/migration`, and the answer of its `POST` (SPEC-470 MVB-020).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationView {
    /// This instance's public URL.
    pub public_url: String,
    /// Whether the move helper's path unit waits for requests on this host.
    pub helper_installed: bool,
    /// Whether an update or restart runs: nothing is asked of a move meanwhile.
    pub update_running: bool,
    /// The request that was written and not taken by the helper yet.
    pub request: Option<MigrationRequest>,
    /// The helper's last report.
    pub run: Option<MigrationRun>,
    /// The command's checklist.
    #[serde(rename = "move")]
    pub moving: Option<MigrationMove>,
    /// The checklist of the move that brought this instance to this host.
    pub arrived: Option<MigrationArrived>,
    /// What would be accepted now.
    pub can: MigrationCan,
}

/// A request the helper has not taken yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationRequest {
    /// Its id.
    pub id: String,
    /// Plan, move or abort.
    pub action: MoveAction,
    /// `requested`, or `unanswered` when the helper did not take it in time.
    pub state: String,
    /// When it was written (RFC 3339).
    pub requested_at: String,
}

/// The helper's report on a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationRun {
    /// The request's id.
    pub id: String,
    /// Plan, move or abort.
    pub action: MoveAction,
    /// The new host; `null` for an abort and for a refused request.
    pub target: Option<String>,
    /// Its SSH port.
    pub ssh_port: Option<u16>,
    /// The new public URL the person named.
    pub public_url: Option<String>,
    /// The data disks the person named.
    pub data_disks: Vec<String>,
    /// `running`, `ended`, `refused`, or `lost` (a running helper that stopped reporting).
    pub state: String,
    /// The command's exit code once it ended: 0 nothing failed, 1 a step failed, 2 refused
    /// before anything changed; `null` when a signal ended it.
    pub exit_code: Option<i32>,
    /// Why the request was refused, or the command's last words when it ended before its
    /// checklist.
    pub error: Option<String>,
    /// When the helper took the request (RFC 3339).
    pub started_at: String,
    /// Its last report (RFC 3339).
    pub updated_at: String,
    /// When the command ended or the request was refused (RFC 3339).
    pub ended_at: Option<String>,
}

/// The command's status: its phase and checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationMove {
    /// The move's id.
    pub id: String,
    /// The new host.
    pub target: String,
    /// `Plan`, `Prepare`, `Cutover`, `Finish`, `Abort`, `done` or `aborted`.
    pub phase: String,
    /// The checklist so far.
    pub lines: Vec<HostLine>,
    /// The one-time key's public line, while a person has to add it on the new host.
    pub public_key: Option<String>,
    /// The new public URL, once the plan decided it.
    pub new_url: Option<String>,
    /// Its last line (RFC 3339).
    pub updated_at: String,
    /// Written after the helper's last run ended: by a run started on the host.
    pub by_command: bool,
    /// Written before the helper's last run started, which ended without a checklist of its
    /// own (the command refused it): the move that is recorded, not what that run found.
    #[serde(default)]
    pub earlier: bool,
}

/// The checklist of the move that brought the instance to this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationArrived {
    /// The move's id.
    pub id: String,
    /// This host, as the old one named it.
    pub target: String,
    /// The old public URL.
    pub from: Option<String>,
    /// The new one.
    pub to: Option<String>,
    /// When the move finished (RFC 3339).
    pub finished_at: String,
    /// The finished checklist.
    pub lines: Vec<HostLine>,
}

/// What `POST /api/admin/instance/migration` would accept now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationCan {
    /// Show a plan.
    pub plan: bool,
    /// Move, or continue a move, with the values of the last plan.
    #[serde(rename = "move")]
    pub moving: bool,
    /// Abort the recorded move, or stop the run in flight.
    pub abort: bool,
}

/// `POST /api/admin/instance/migration` (SPEC-470 MVB-021).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskMigration {
    /// Plan, move or abort.
    pub action: MoveAction,
    /// The new host (`root@new-host`); not for an abort.
    #[serde(default)]
    pub target: Option<String>,
    /// Its SSH port (22 when absent).
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// The new public URL, when it is not the default.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Blank disks of the new host for the data directory.
    #[serde(default)]
    pub data_disks: Vec<String>,
    /// For a move: the new host's name, typed by the person.
    #[serde(default)]
    pub confirm: Option<String>,
}
