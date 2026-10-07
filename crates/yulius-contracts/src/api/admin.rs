//! Instance administration: users, accounts and GitHub connection.

use serde::{Deserialize, Serialize};

/// A user as listed for the superadmin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUser {
    /// Id.
    pub id: String,
    /// GitHub login.
    pub github_login: String,
    /// Display name.
    pub display_name: String,
    /// Instance administrator flag.
    pub is_instance_admin: bool,
    /// Disabled flag.
    pub disabled: bool,
    /// Has signed in with GitHub at least once.
    pub github_linked: bool,
}

/// `POST /api/admin/users`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmitUser {
    /// GitHub login to admit.
    pub github_login: String,
    /// Display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Instance administrator flag.
    #[serde(default)]
    pub is_instance_admin: bool,
}

/// `PATCH /api/admin/users/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateUser {
    /// Disable or enable.
    #[serde(default)]
    pub disabled: Option<bool>,
    /// Instance administrator flag.
    #[serde(default)]
    pub is_instance_admin: Option<bool>,
}

/// A usage window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindowView {
    /// Percent used (0–100).
    pub used_percent: Option<f64>,
    /// Reset time.
    pub resets_at: Option<String>,
    /// Window length in minutes.
    pub window_minutes: Option<u32>,
}

/// Account usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AccountUsageView {
    /// Primary (short) window.
    pub primary: Option<UsageWindowView>,
    /// Secondary (weekly) window.
    pub secondary: Option<UsageWindowView>,
    /// When the snapshot was observed.
    pub observed_at: Option<String>,
}

/// A connected provider account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountView {
    /// Id.
    pub id: String,
    /// `anthropic` or `openai`.
    pub provider: String,
    /// Label.
    pub label: String,
    /// Enabled.
    pub enabled: bool,
    /// Lower is preferred.
    pub priority: i32,
    /// `ok`, `relogin`, `error`, `limited` or `pending_login`.
    pub status: String,
    /// Usage snapshot.
    pub usage: AccountUsageView,
    /// Last error (no secrets).
    pub last_error: Option<String>,
    /// Live tasks using it.
    pub in_use_by: u32,
}

/// `POST /api/admin/accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAccount {
    /// `anthropic` or `openai`.
    pub provider: String,
    /// Label.
    pub label: String,
}

/// Response to account creation or relogin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountLoginStarted {
    /// The account.
    pub account: AccountView,
    /// Login terminal id (`/api/admin/accounts/login/{login_id}/terminal`).
    pub login_id: String,
}

/// `PATCH /api/admin/accounts/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateAccount {
    /// Enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Priority.
    #[serde(default)]
    pub priority: Option<i32>,
    /// Label.
    #[serde(default)]
    pub label: Option<String>,
}

/// `GET /api/admin/github`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubStatus {
    /// App credentials stored.
    pub configured: bool,
    /// App slug.
    pub app_slug: Option<String>,
    /// Instance owner.
    pub owner_login: Option<String>,
    /// Installation URL when not yet installed.
    pub install_url: Option<String>,
    /// Installation found.
    pub installed: bool,
}

/// `POST /api/admin/github/manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRequest {
    /// Organization or user login.
    pub owner_login: String,
    /// `org` or `user`.
    pub owner_kind: String,
}

/// Manifest form data the browser posts to GitHub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestResponse {
    /// GitHub URL to POST the form to.
    pub post_url: String,
    /// The manifest JSON.
    pub manifest: serde_json::Value,
}

/// Query of `GET /api/admin/github/manifest/callback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCallback {
    /// Conversion code from GitHub.
    #[serde(default)]
    pub code: String,
    /// The state issued with the manifest.
    #[serde(default)]
    pub state: String,
}

/// `GET`/`PATCH /api/admin/settings`: instance behavior settings (instance administrators).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InstanceSettings {
    /// Rotation may move tasks to another harness without asking (ROT-012; off by default).
    #[serde(default)]
    pub auto_harness_switch: bool,
}
