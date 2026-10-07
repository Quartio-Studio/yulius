//! Status panel data.

use serde::{Deserialize, Serialize};

use super::admin::AccountView;

/// VM metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VmStatus {
    /// CPU utilization percent.
    pub cpu_percent: f32,
    /// Used memory bytes.
    pub mem_used: u64,
    /// Total memory bytes.
    pub mem_total: u64,
    /// Used disk bytes (data directory volume).
    pub disk_used: u64,
    /// Total disk bytes.
    pub disk_total: u64,
    /// One-minute load average.
    pub load_1m: f64,
}

/// Per-sandbox status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxStatus {
    /// Sandbox id.
    pub id: String,
    /// Slug.
    pub slug: String,
    /// Live sessions.
    pub sessions: u32,
}

/// `GET /api/status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusView {
    /// VM metrics.
    pub vm: VmStatus,
    /// Visible sandboxes.
    pub sandboxes: Vec<SandboxStatus>,
    /// Accounts (instance admins see all; others see label/usage only).
    pub accounts: Vec<AccountView>,
    /// Containment active.
    pub contained: bool,
    /// Live sessions / limit.
    pub live_sessions: u32,
    /// Configured limit.
    pub max_live_sessions: u32,
}
