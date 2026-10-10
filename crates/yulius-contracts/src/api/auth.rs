//! Setup, sessions, current user and CLI device approval.

use serde::{Deserialize, Serialize};

/// `GET /api/setup/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStatus {
    /// True until the superadmin exists.
    pub setup_required: bool,
}

/// `POST /api/setup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRequest {
    /// One-time setup token from the VM.
    pub token: String,
    /// Chosen superadmin password.
    pub password: String,
    /// Repeated password.
    pub password_confirm: String,
    /// Explicit confirmation that the non-recoverable password was stored safely.
    pub confirm_saved: bool,
}

/// `POST /api/auth/password`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordLogin {
    /// Superadmin password.
    pub password: String,
}

/// `GET /api/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// The signed-in user.
    pub user: MeUser,
    /// Token to send as `X-Yulius-CSRF` on state-changing requests.
    pub csrf_token: String,
    /// Instance facts the UI needs.
    pub instance: InstanceInfo,
}

/// User part of [`Me`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeUser {
    /// User id.
    pub id: String,
    /// GitHub login, or `superadmin`.
    pub login: String,
    /// Display name.
    pub display_name: String,
    /// GitHub avatar URL.
    pub avatar_url: Option<String>,
    /// Whether this is the superadmin.
    pub is_superadmin: bool,
    /// Whether this user administers accounts, instance MCP and skills.
    pub is_instance_admin: bool,
    /// Whether a GitHub identity is linked.
    pub github_linked: bool,
}

/// Instance part of [`Me`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceInfo {
    /// GitHub owner (organization or user) projects belong to.
    pub owner_login: Option<String>,
    /// `org` or `user`.
    pub owner_kind: Option<String>,
    /// Public base URL.
    pub public_url: String,
    /// Whether the GitHub App is configured.
    pub github_configured: bool,
    /// Whether sandboxes run inside OS containment.
    pub contained: bool,
    /// Largest file one upload may carry, in bytes (EDT-006, UI-044; `YULIUS_UPLOAD_MAX_MB`).
    #[serde(default)]
    pub upload_max_bytes: u64,
}

/// `POST /api/device/start` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStart {
    /// Secret code the CLI polls with.
    pub device_code: String,
    /// Short code the user confirms in the browser.
    pub user_code: String,
    /// Page where the user approves.
    pub verification_url: String,
    /// Poll interval in seconds.
    pub interval_s: u32,
    /// Lifetime in seconds.
    pub expires_in_s: u32,
}

/// `POST /api/device/poll` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePollRequest {
    /// Device code from [`DeviceStart`].
    pub device_code: String,
}

/// `POST /api/device/poll` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePoll {
    /// `pending`, `approved`, `denied` or `expired`.
    pub status: String,
    /// API token once approved (returned exactly once).
    pub token: Option<String>,
}

/// `POST /api/device/approve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceApprove {
    /// User code shown by the CLI.
    pub user_code: String,
    /// Approve or deny.
    pub approve: bool,
}

/// `GET /api/auth/recovery`: whether password recovery is possible (ACC-011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatus {
    /// A recovery code exists and recovery is not locked.
    pub available: bool,
    /// Recovery was disabled permanently by wrong codes.
    pub locked: bool,
    /// Wrong codes still accepted before the lock.
    pub attempts_left: u32,
}

/// `POST /api/auth/recover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverRequest {
    /// The 6-digit recovery code.
    pub code: String,
    /// New superadmin password.
    pub password: String,
    /// Repeated new password.
    pub password_confirm: String,
}

/// A freshly issued recovery code, shown once (setup, recovery, regeneration).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCode {
    /// Six digits; never shown again.
    pub recovery_code: String,
}

/// `POST /api/admin/recovery-code`: the current password re-confirms the superadmin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegenerateRecovery {
    /// Current superadmin password.
    pub password: String,
}
