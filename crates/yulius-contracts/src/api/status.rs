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
    /// Provider (`anthropic`).
    pub provider: String,
    /// `ok`, `relogin`, `error`, `limited` or `pending_login`.
    pub status: String,
    /// Enabled for rotation.
    pub enabled: bool,
    /// Usage snapshot (unknown windows are `null`).
    pub usage: AccountUsageView,
    /// Id (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Priority (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    /// Last error (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Live tasks using it (instance administrators only).
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
