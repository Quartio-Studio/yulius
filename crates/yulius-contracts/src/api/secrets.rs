//! Secrets vault (SPEC-001 VLT, SPEC-002 ADR-20, SPEC-040): team secrets and agent secrets
//! people reveal after unlocking the vault with their two-factor code; agents use agent secrets
//! without seeing them.

use serde::{Deserialize, Serialize};

/// A team secret in lists: a title; its value and notes are only flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSecretView {
    /// Id.
    pub id: String,
    /// Title, e.g. "Shared Gmail".
    pub title: String,
    /// `sandbox` (one sandbox's people) or `instance` (team-wide) (SPEC-216).
    #[serde(default)]
    pub scope: String,
    /// Whether a value is stored.
    pub has_value: bool,
    /// Whether notes are stored.
    pub has_notes: bool,
    /// Who added it.
    pub created_by: String,
    /// Who changed it last.
    pub updated_by: String,
    /// When it changed last.
    pub updated_at: String,
    /// When the value or the notes changed last.
    pub value_updated_at: String,
    /// The value is older than 180 days: rotate it.
    pub rotate_due: bool,
}

/// Create body: a title, a value and notes (SPEC-040).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSecretInput {
    /// `sandbox` (the default under a sandbox) or `instance` (team-wide, the only one without a
    /// sandbox) (SPEC-216 SCO-001).
    #[serde(default)]
    pub scope: Option<String>,
    /// Title.
    pub title: String,
    /// The secret value (a password, a key, a recovery code…).
    #[serde(default)]
    pub value: Option<String>,
    /// Notes (user name, address, security answers…), encrypted like the value.
    #[serde(default)]
    pub notes: Option<String>,
}

/// Update body: an absent field stays, an empty string clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSecretPatch {
    /// Title (never cleared).
    #[serde(default)]
    pub title: Option<String>,
    /// Value.
    #[serde(default)]
    pub value: Option<String>,
    /// Notes.
    #[serde(default)]
    pub notes: Option<String>,
}

/// `POST /api/team-secrets/{id}/reveal` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevealRequest {
    /// `show` (default) or `copy`, recorded in the audit trail.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Fields to reveal: `value`, `notes` (default: both).
    #[serde(default)]
    pub fields: Option<Vec<String>>,
}

/// Revealed fields (served with `Cache-Control: no-store`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretReveal {
    /// The value.
    pub value: Option<String>,
    /// Notes.
    pub notes: Option<String>,
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

/// `GET /api/auth/reauth`: whether this browser session may reveal now, and how to get there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReauthStatus {
    /// The vault is unlocked: a two-factor code was entered in this session in the last 10
    /// minutes (SPEC-040).
    pub fresh: bool,
    /// Until when (RFC 3339), when unlocked.
    pub fresh_until: Option<String>,
    /// The person's two-factor: `missing`, `pending` (set up, not confirmed), `on`, or `locked`
    /// (10 wrong codes in a row, until identity is confirmed again).
    pub two_factor: String,
    /// The session signed in or confirmed it is them in the last 10 minutes (needed to set up
    /// two-factor).
    pub identity_fresh: bool,
    /// How to confirm identity: `password` (the superadmin) and/or `github` (sign in again).
    pub methods: Vec<String>,
}

/// `POST /api/auth/two-factor`: a new seed for the person's authenticator app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwoFactorSetup {
    /// The base32 seed ("enter this key").
    pub secret: String,
    /// The same as an `otpauth://totp/…` link (what a QR code holds).
    pub uri: String,
}

/// `POST /api/auth/unlock` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnlockRequest {
    /// The current code of the person's authenticator.
    pub code: String,
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
    /// The hosts it is sent to (proxy) or typed into (browser); empty until a person binds or
    /// allows some (SPEC-121).
    pub hosts: Vec<String>,
    /// Hosts an agent proposed, waiting for a person who manages it to allow or dismiss them.
    pub requested_hosts: Vec<String>,
    /// The task that asked last (`#107`).
    pub requested_by: Option<String>,
    /// Whether agents may copy the value into the project's git-ignored `.env` files
    /// (SPEC-129): `no`, `asked` (a person allows or dismisses it) or `allowed`.
    #[serde(default)]
    pub env_files: String,
    /// The tasks asking to copy it (`#111`) while `env_files` is `asked`.
    #[serde(default)]
    pub env_requested_by: Option<String>,
    /// Production CD runs get its value (SPEC-134): a person's allowance of its own.
    #[serde(default)]
    pub cd_prod: bool,
    /// Agents set it up themselves (SPEC-216): the hosts they ask for are bound at once and
    /// production CD runs get its value, with no person's allowance.
    #[serde(default)]
    pub auto_allow: bool,
    /// `none`, `bearer`, `basic` or `header`.
    pub injection: String,
    /// Header of the `header` injection.
    pub header_name: Option<String>,
    /// How it is added to requests, e.g. `Authorization: Bearer <value>`.
    pub injection_text: String,
    /// The placeholder agents' code uses (`yulius-secret-NAME`).
    pub placeholder: String,
    /// Whether a user name is stored (Basic credentials; older secrets: website logins).
    pub has_username: bool,
    /// Whether an older secret stores a TOTP seed (none can be added since SPEC-040).
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
    /// The value is a key whose line breaks were lost before SPEC-193 (a one-line field turned
    /// them into spaces): it must be pasted again. Told only to people who manage it.
    #[serde(default)]
    pub flattened: bool,
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
    /// Bound hosts; none by default: agents propose them and a person allows them (SPEC-121).
    #[serde(default)]
    pub hosts: Vec<String>,
    /// `none` (default: agents send the placeholder themselves), `bearer`, `basic` or `header`.
    #[serde(default)]
    pub injection: Option<String>,
    /// Header for `header`.
    #[serde(default)]
    pub header_name: Option<String>,
    /// User name of `basic` credentials (only there).
    #[serde(default)]
    pub username: Option<String>,
    /// The value (at least 8 characters).
    pub value: String,
    /// Agents set it up themselves (SPEC-216 SSA-001); `true` when absent.
    #[serde(default)]
    pub auto_allow: Option<bool>,
}

/// Update body: an absent field stays; an empty `username` clears it; `value` replaces the
/// value.
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
    /// User name of `basic` credentials.
    #[serde(default)]
    pub username: Option<String>,
    /// New value.
    #[serde(default)]
    pub value: Option<String>,
    /// Agents set it up themselves (SPEC-216 SSA-005).
    #[serde(default)]
    pub auto_allow: Option<bool>,
}

/// `POST /api/agent-secrets/{id}/request` body: answers the hosts an agent asked for (SPEC-121).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequestAnswer {
    /// `true`: the requested hosts join the secret's bindings; `false`: they are dismissed.
    pub allow: bool,
    /// The requested hosts the person saw: the answer is refused (409) unless they are exactly
    /// what waits, so a host asked for meanwhile is never allowed unseen.
    pub hosts: Vec<String>,
}

/// `POST /api/agent-secrets/{id}/env` body (SPEC-129): `true` allows agents to copy the secret
/// into `.env` files (answering their request); `false` dismisses the request or revokes the
/// allowance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvFilesAnswer {
    /// Allow (`true`) or not.
    pub allow: bool,
    /// The `env_files` the person saw: the answer is refused (409) unless it still holds.
    pub env_files: String,
    /// The askers the person saw (`env_requested_by`), compared the same way.
    #[serde(default)]
    pub env_requested_by: Option<String>,
}

/// `POST /api/agent-secrets/{id}/cd-prod`: a person allows (or stops) production CD runs getting
/// the secret's value (SPEC-134).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CdProdAnswer {
    /// Allow (`true`) or revoke.
    pub allow: bool,
}

/// One vault audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultActivity {
    /// When.
    pub at: String,
    /// Who: a GitHub login, `superadmin`, `task:<n>` in a sandbox, or `ci:<run>` for a CI job
    /// (SPEC-142).
    pub actor: String,
    /// What, e.g. `vault.team.reveal`, `vault.agent.use`, `vault.agent.redacted`.
    pub action: String,
    /// The secret.
    pub secret_id: Option<String>,
    /// Name and context (never a value).
    pub detail: Option<String>,
}
