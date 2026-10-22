//! Leftovers on the server's disk (SPEC-215, ADR-0170): `GET /api/admin/leftovers`,
//! `POST /api/admin/leftovers/scan` and `POST /api/admin/leftovers/delete` (superadmin), and the
//! same over the loopback listener for `yulius leftovers` (`/internal/leftovers*`).

use serde::{Deserialize, Serialize};

/// One leftover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftoverView {
    /// Stable id (a hash of the path).
    pub id: String,
    /// Kind (`removed-worktrees`, `task-cache`, … ; SPEC-215 inventory).
    pub kind: String,
    /// Path on the server.
    pub path: String,
    /// Allocated bytes; `None` when it could not be measured.
    pub bytes: Option<u64>,
    /// The sandbox it belongs to, by its slug.
    pub sandbox: Option<String>,
    /// The task it belongs to, by its number.
    pub task: Option<u64>,
    /// Why it is a leftover.
    pub reason: String,
    /// The automatic rounds delete it (else only a person does).
    pub auto: bool,
    /// Yulius may delete it at all (the host's own data is only reported).
    pub deletable: bool,
}

/// A running or finished scan or deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeftoverJobView {
    /// `scan` or `delete`.
    pub action: String,
    /// It still runs.
    pub running: bool,
    /// Who started it (`yulius` for the automatic rounds).
    pub actor: String,
    /// Items it has to handle.
    pub total: usize,
    /// Items handled so far.
    pub done: usize,
    /// Items deleted.
    pub deleted: usize,
    /// Bytes freed.
    pub freed: u64,
    /// Items kept because they were in use or no longer leftovers at deletion: "path: why".
    pub kept: Vec<String>,
    /// Failures: "path: error".
    pub failed: Vec<String>,
    /// Started (RFC 3339).
    pub started_at: String,
    /// Ended (RFC 3339), when it has.
    pub ended_at: Option<String>,
}

/// `GET /api/admin/leftovers`: the last scan and the disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LeftoversView {
    /// The last scan's items; empty before the first scan.
    pub items: Vec<LeftoverView>,
    /// When the last scan ended (RFC 3339).
    pub scanned_at: Option<String>,
    /// Bytes of every deletable item.
    pub total_bytes: u64,
    /// Bytes of the items the automatic rounds delete.
    pub safe_bytes: u64,
    /// Used bytes of the volume that holds the data directory.
    pub disk_used: Option<u64>,
    /// Total bytes of that volume.
    pub disk_total: Option<u64>,
    /// The data directory.
    pub data_dir: String,
    /// The running or last scan or deletion.
    pub job: Option<LeftoverJobView>,
    /// When the next automatic round runs (RFC 3339).
    pub next_auto_at: Option<String>,
    /// The last automatic round since the start, as it ended (SPEC-276 RDA-009): what it deleted,
    /// freed, kept and failed.
    #[serde(default)]
    pub last_auto: Option<LeftoverJobView>,
    /// Removed sandboxes whose files were kept, as the last scan found them (SPEC-276 RDA-012).
    #[serde(default)]
    pub kept: Vec<KeptSandboxView>,
}

/// A removed sandbox whose files stay on the server (SPEC-276 RDA-012).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeptSandboxView {
    /// Sandbox id (`POST /api/sandboxes/{id}/purge` deletes its files).
    pub id: String,
    /// Its slug.
    pub slug: String,
    /// When it was removed (RFC 3339).
    pub removed_at: Option<String>,
    /// Bytes of everything of it; `None` when unreadable.
    pub bytes: Option<u64>,
    /// Its files were asked to go and are deleted once nothing of it runs.
    pub purge_requested: bool,
}

/// `POST /api/admin/leftovers/delete`: the items to delete, or every safe one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeleteLeftoversRequest {
    /// Ids from the last scan.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Every item the automatic rounds would delete.
    #[serde(default)]
    pub all_safe: bool,
}
