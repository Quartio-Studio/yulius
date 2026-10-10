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
    /// Weekly cap of Opus models (`seven_day_opus`), when the provider reports it.
    #[serde(default)]
    pub weekly_opus: Option<UsageWindowView>,
    /// Weekly cap of Sonnet models (`seven_day_sonnet`), when the provider reports it.
    #[serde(default)]
    pub weekly_sonnet: Option<UsageWindowView>,
    /// When the snapshot was observed.
    pub observed_at: Option<String>,
}

/// A connected provider account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountView {
    /// Id.
    pub id: String,
    /// Provider (`anthropic`).
    pub provider: String,
    /// Label.
    pub label: String,
    /// `ok`, `relogin`, `error`, `limited` or `pending_login`.
    pub status: String,
    /// Usage snapshot.
    pub usage: AccountUsageView,
    /// Last error (no secrets).
    pub last_error: Option<String>,
    /// Live tasks using it.
    pub in_use_by: u32,
    /// Tokens Yulius observed on this account in the last 24 hours (ROT-009).
    #[serde(default)]
    pub tokens_24h: Option<TokenTotalsView>,
    /// Tokens Yulius observed on this account in the last 7 days.
    #[serde(default)]
    pub tokens_7d: Option<TokenTotalsView>,
}

/// Token totals of an account's runs; `None` values are unknown (runs without usage data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TokenTotalsView {
    /// Runs started in the window.
    pub runs: u32,
    /// Distinct tasks of those runs.
    pub tasks: u32,
    /// Input tokens.
    pub input: Option<u64>,
    /// Output tokens.
    pub output: Option<u64>,
    /// Cache-read tokens.
    pub cache_read: Option<u64>,
    /// Cache-write tokens.
    pub cache_write: Option<u64>,
}

/// `POST /api/admin/accounts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAccount {
    /// Provider: `anthropic` (Claude), the only one supported.
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

/// `PATCH /api/admin/accounts/{id}`: only the label. The pool has no priorities or switches
/// (F 2026-10-10); any other field is refused (422).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAccount {
    /// Label.
    pub label: String,
}

/// One account in [`CapacityView`]. Everyone sees label, status and usage; `id` and
/// `in_use_by` are for instance administrators only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacityAccount {
    /// Label.
    pub label: String,
    /// `ok`, `relogin`, `error`, `limited` or `pending_login`.
    pub status: String,
    /// Usage snapshot (unknown windows are `null`).
    pub usage: AccountUsageView,
    /// Below the threshold in its windows and healthy: new work can go here now.
    pub available: bool,
    /// Id (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Live tasks on it (instance administrators only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_use_by: Option<u32>,
}

/// `GET /api/capacity` (signed in): the subscription pool, its burn rate and forecast
/// (SPEC-001 ROT-013, SPEC-002 ADR-17 §2.4). Points are percentage points of one
/// subscription's weekly window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacityView {
    /// Pause and switch threshold in percent (98).
    pub threshold: f64,
    /// Healthy subscriptions in the pool.
    pub subscriptions: u32,
    /// Healthy subscriptions whose weekly usage is not known yet.
    pub unknown_usage: u32,
    /// Subscriptions below the threshold right now.
    pub available: u32,
    /// Points left in the pool's weekly windows before it pauses; `null` when unknown.
    pub weekly_left: Option<f64>,
    /// Points left in the pool's 5-hour windows; `null` when unknown.
    pub five_hour_left: Option<f64>,
    /// Burn rate of the last 24 hours in points per hour; `null` without enough readings.
    pub burn_24h: Option<f64>,
    /// Burn rate of the last 7 days in points per hour.
    pub burn_7d: Option<f64>,
    /// When the pool's weekly capacity runs out at the 24-hour rate, if within 7 days.
    pub runs_out_at: Option<String>,
    /// Task-hours paused for lack of capacity in the last 7 days.
    pub paused_hours_7d: f64,
    /// Task-hours agents ran in the last 7 days (pauses excluded).
    pub active_hours_7d: f64,
    /// Subscriptions the observed demand needs.
    pub needed: u32,
    /// Subscriptions to add (0 when capacity is sufficient).
    pub add: u32,
    /// Plain recommendation ("Capacity is sufficient", "Add about 2 subscriptions to stop
    /// pausing").
    pub recommendation: String,
    /// Tasks paused right now.
    pub paused_tasks: u32,
    /// While every subscription is exhausted: the earliest known time one has capacity again.
    pub resumes_at: Option<String>,
    /// The subscriptions.
    pub accounts: Vec<CapacityAccount>,
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
