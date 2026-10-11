//! Secrets vault (SPEC-001 VLT, SPEC-002 ADR-20): team secrets people reveal after
//! re-authenticating, and agent secrets agents use but nobody reads back.

use serde::{Deserialize, Serialize};

/// A team secret in lists: its secret fields are only flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSecretView {
    /// Id.
    pub id: String,
    /// Title, e.g. "Shared Gmail".
    pub title: String,
    /// Sign-in address.
    pub url: Option<String>,
    /// User name or email.
    pub username: Option<String>,
    /// Whether a password is stored.
    pub has_password: bool,
    /// Whether notes are stored.
    pub has_notes: bool,
    /// Whether a TOTP seed is stored (codes on demand).
    pub has_totp: bool,
    /// Who added it.
    pub created_by: String,
    /// Who changed it last.
    pub updated_by: String,
    /// When it changed last.
    pub updated_at: String,
    /// When a secret field changed last.
    pub value_updated_at: String,
    /// The secret fields are older than 180 days: rotate them.
    pub rotate_due: bool,
}

/// Create body (every field but the title optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSecretInput {
    /// Title.
    pub title: String,
    /// Sign-in address.
    #[serde(default)]
    pub url: Option<String>,
    /// User name or email.
    #[serde(default)]
    pub username: Option<String>,
    /// Password.
    #[serde(default)]
    pub password: Option<String>,
    /// Notes (recovery codes, security answers…).
    #[serde(default)]
    pub notes: Option<String>,
    /// TOTP seed: base32 or an `otpauth://totp/…` URI.
    #[serde(default)]
    pub totp: Option<String>,
}

/// Update body: an absent field stays, an empty string clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSecretPatch {
    /// Title (never cleared).
    #[serde(default)]
    pub title: Option<String>,
    /// Sign-in address.
    #[serde(default)]
    pub url: Option<String>,
    /// User name or email.
    #[serde(default)]
    pub username: Option<String>,
    /// Password.
    #[serde(default)]
    pub password: Option<String>,
    /// Notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// TOTP seed.
    #[serde(default)]
    pub totp: Option<String>,
}

/// `POST /api/team-secrets/{id}/reveal` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevealRequest {
    /// `show` (default) or `copy`, recorded in the audit trail.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Fields to reveal: `password`, `notes`, `totp` (default: every stored one).
    #[serde(default)]
    pub fields: Option<Vec<String>>,
}

/// Revealed fields (served with `Cache-Control: no-store`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSecretReveal {
    /// Password.
    pub password: Option<String>,
    /// Notes.
    pub notes: Option<String>,
    /// The TOTP seed as stored.
    pub totp: Option<String>,
}

/// A current TOTP code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpCode {
    /// The code.
    pub code: String,
    /// Step in seconds.
    pub period: u64,
    /// Seconds until the next code.
    pub remaining: u64,
}

/// `GET /api/auth/reauth`: whether this browser session may reveal now, and how to confirm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReauthStatus {
    /// Reveals are allowed now.
    pub fresh: bool,
    /// Until when (RFC 3339), when fresh.
    pub fresh_until: Option<String>,
    /// `password` (the superadmin) and/or `github` (sign in with GitHub again).
    pub methods: Vec<String>,
}

/// `POST /api/auth/reauth` body (password method).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReauthPassword {
    /// The superadmin password.
    pub password: String,
}

/// An agent secret: metadata only; the value is never returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSecretView {
    /// Id.
    pub id: String,
    /// NAME (environment style); agents refer to it by this name.
    pub name: String,
    /// What it is for (agents read it).
    pub description: String,
    /// `sandbox` or `instance` (every sandbox).
    pub scope: String,
    /// The hosts it is sent to (proxy) or typed into (browser).
    pub hosts: Vec<String>,
    /// `none`, `bearer`, `basic` or `header`.
    pub injection: String,
    /// Header of the `header` injection.
    pub header_name: Option<String>,
    /// How it is added to requests, e.g. `Authorization: Bearer <value>`.
    pub injection_text: String,
    /// The placeholder agents' code uses (`yulius-secret-NAME`).
    pub placeholder: String,
    /// Whether a user name is stored (Basic credentials, website logins).
    pub has_username: bool,
    /// Whether a TOTP seed is stored (website two-factor codes).
    pub has_totp: bool,
    /// Who added it.
    pub created_by: String,
    /// When it changed last.
    pub updated_at: String,
    /// When the value changed last.
    pub value_updated_at: String,
    /// When an agent last used it.
    pub last_used_at: Option<String>,
    /// The value is older than 180 days.
    pub rotate_due: bool,
    /// Whether the caller may change or delete it.
    pub can_manage: bool,
}

/// Create body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSecretInput {
    /// `sandbox` (default) or `instance` (instance administrators).
    #[serde(default)]
    pub scope: Option<String>,
    /// NAME.
    pub name: String,
    /// What it is for.
    #[serde(default)]
    pub description: Option<String>,
    /// Bound hosts (at least one).
    pub hosts: Vec<String>,
    /// `none`, `bearer` (default), `basic` or `header`.
    #[serde(default)]
    pub injection: Option<String>,
    /// Header for `header`.
    #[serde(default)]
    pub header_name: Option<String>,
    /// User name (Basic credentials, website logins).
    #[serde(default)]
    pub username: Option<String>,
    /// The value (at least 8 characters).
    pub value: String,
    /// TOTP seed (website two-factor codes).
    #[serde(default)]
    pub totp: Option<String>,
}

/// Update body: an absent field stays; an empty `username` or `totp` clears it; `value`
/// replaces the value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSecretPatch {
    /// NAME.
    #[serde(default)]
    pub name: Option<String>,
    /// What it is for.
    #[serde(default)]
    pub description: Option<String>,
    /// Bound hosts.
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    /// Injection kind.
    #[serde(default)]
    pub injection: Option<String>,
    /// Header for `header`.
    #[serde(default)]
    pub header_name: Option<String>,
    /// User name.
    #[serde(default)]
    pub username: Option<String>,
    /// New value.
    #[serde(default)]
    pub value: Option<String>,
    /// TOTP seed.
    #[serde(default)]
    pub totp: Option<String>,
}

/// One vault audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultActivity {
    /// When.
    pub at: String,
    /// Who: a GitHub login, `superadmin`, or `task:<n>` in a sandbox.
    pub actor: String,
    /// What, e.g. `vault.team.reveal`, `vault.agent.use`, `vault.agent.redacted`.
    pub action: String,
    /// The secret.
    pub secret_id: Option<String>,
    /// Name and context (never a value).
    pub detail: Option<String>,
}
