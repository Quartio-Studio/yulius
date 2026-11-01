//! Status panel data (SPEC-001 UI-007, AC-STA-1). Unknown metrics are `null`, never zero.

use serde::{Deserialize, Serialize};

use super::admin::AccountUsageView;

/// VM metrics of the machine running `yulius serve`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VmStatus {
    /// CPU utilization percent (all cores, 0–100).
    pub cpu_percent: Option<f32>,
    /// Logical CPU count.
    pub cpu_count: Option<u32>,
    /// Used memory bytes.
    pub memory_used_bytes: Option<u64>,
    /// Total memory bytes.
    pub memory_total_bytes: Option<u64>,
    /// Used bytes of the volume holding the data directory.
    pub disk_used_bytes: Option<u64>,
    /// Total bytes of that volume.
    pub disk_total_bytes: Option<u64>,
    /// Load averages `[1m, 5m, 15m]`.
    pub load: Option<[f64; 3]>,
    /// One-minute load average (same as `load[0]`).
    pub load_1m: Option<f64>,
    /// Seconds since boot.
    pub uptime_s: Option<u64>,
}

/// Live session capacity.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionCounts {
    /// Live agent sessions (instance-wide).
    pub live: u32,
    /// Configured limit.
    pub limit: u32,
    /// Queued tasks (instance-wide).
    pub queued: u32,
}

/// Per-sandbox status (visible sandboxes only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxStatus {
    /// Sandbox id.
    pub id: String,
    /// Slug.
    pub slug: String,
    /// Display name (repository name).
    pub name: String,
    /// Live agent sessions in this sandbox.
    pub live_sessions: u32,
}

/// An account as shown in the status panel. Everyone sees label, provider, status and usage;
/// the remaining fields are present only for instance administrators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusAccount {
    /// Label.
    pub label: String,
    /// Email of the Claude account behind it (instance administrators only, UI-022; absent while
    /// unknown, SPEC-244 SUB-005).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Provider (`anthropic`).
    pub provider: String,
    /// `ok`, `relogin`, `error`, `limited` or `pending_login`.
    pub status: String,
    /// Usage snapshot (unknown windows are `null`).
    pub usage: AccountUsageView,
    /// Id (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Last error (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Live tasks using it, except Done ones (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_use_by: Option<u32>,
}

/// `GET /api/status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusView {
    /// VM metrics.
    pub vm: VmStatus,
    /// Live sessions vs limit.
    pub sessions: SessionCounts,
    /// Visible sandboxes.
    pub sandboxes: Vec<SandboxStatus>,
    /// Accounts with usage.
    pub accounts: Vec<StatusAccount>,
    /// OS containment active for sandboxes.
    pub contained: bool,
}

/// `GET /api/disk` (SPEC-276 RDA-007): how full the server's data disk is, for the warning every
/// page shows while it runs low.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiskView {
    /// `ok`, `low` (from 85 % used) or `critical` (from 95 %).
    pub level: String,
    /// Used bytes of the data disk; `None` when unknown.
    pub used: Option<u64>,
    /// Its total bytes; `None` when unknown.
    pub total: Option<u64>,
}

/// `GET /api/connections` (SPEC-361 CHR-004): the open connection troubles a person may see.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConnectionsView {
    /// Oldest first.
    pub troubles: Vec<ConnectionTroubleView>,
}

/// A limit refusing connections: a sandbox's proxy share, its relay toward Yulius, or the proxy's
/// own total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTroubleView {
    /// Unique per trouble, also across restarts: one notification per id.
    pub id: String,
    /// `share`, `relay` or `proxy`.
    pub limit: String,
    /// Its sandbox; `None` for the proxy's own total.
    pub sandbox_id: Option<String>,
    /// The sandbox's name.
    pub sandbox_name: Option<String>,
    /// When it began (ms since the epoch).
    pub since_ms: i64,
    /// Connections held at its last refusal.
    pub held: usize,
    /// The limit.
    pub max: usize,
    /// Refusals since it began.
    pub refused: u64,
    /// The busiest holders at its last refusal, busiest first: runs at a share, sandboxes at the
    /// proxy's total, none for a relay.
    pub holders: Vec<ConnectionHolderView>,
}

/// Who holds connections at a limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionHolderView {
    /// `task` (an agent run), `ci` (a CI job), `superagent`, `run` (one that ended), `sandbox`
    /// or `other_sandbox` (one the person cannot view).
    pub kind: String,
    /// How the panel names it: `task #12`, `a CI job`, the sandbox's name.
    pub label: String,
    /// The run's task.
    pub task_id: Option<String>,
    /// Its number.
    pub task_number: Option<u32>,
    /// Its title.
    pub task_title: Option<String>,
    /// The sandbox, at the proxy's total.
    pub sandbox_id: Option<String>,
    /// Connections it holds there.
    pub held: usize,
}
