//! Vault persistence port (SPEC-002 ADR-20): sealed secrets, re-authentication times, sealed
//! vault key material and the vault's audit trail. Records hold envelopes, never plaintext.

use async_trait::async_trait;
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::ports::Envelope;

/// Which section a secret belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSection {
    /// People's shared credentials (every member).
    Team,
    /// Credentials agents use without seeing.
    Agent,
}

impl VaultSection {
    /// Stored form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Team => "team",
            Self::Agent => "agent",
        }
    }

    /// Parses the stored form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "team" => Some(Self::Team),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// One stored secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultSecretRecord {
    /// Identifier.
    pub id: String,
    /// Section.
    pub section: VaultSection,
    /// Agent secrets: the sandbox, or `None` for every sandbox. Always `None` for team secrets.
    pub sandbox_id: Option<SandboxId>,
    /// Team: title. Agent: NAME.
    pub name: String,
    /// Agent: what it is for.
    pub description: String,
    /// Agent: host bindings as entered (normalized).
    pub hosts: Vec<String>,
    /// Agent: injection kind.
    pub injection: String,
    /// Agent: header of the `header` injection.
    pub header_name: Option<String>,
    /// Names of the fields present (no values).
    pub fields: Vec<String>,
    /// The sealed payload.
    pub envelope: Envelope,
    /// Who created it (user id).
    pub created_by: String,
    /// When.
    pub created_at: Timestamp,
    /// Who changed it last.
    pub updated_by: String,
    /// When.
    pub updated_at: Timestamp,
    /// When a value field last changed (rotation age).
    pub value_updated_at: Timestamp,
    /// When an agent last used it.
    pub last_used_at: Option<Timestamp>,
    /// Agent: hosts an agent proposed, waiting for a person to allow them (SPEC-121). Never
    /// used to send the secret; written only by [`VaultStore::vault_set_request`].
    pub requested_hosts: Vec<String>,
    /// Agent: the task that asked last (`#107`).
    pub requested_by: Option<String>,
    /// Agent: whether agents may copy the value into the project's `.env` files (SPEC-129);
    /// written only by [`VaultStore::vault_set_env_files`].
    pub env_files: EnvFiles,
    /// Agent: the tasks asking to copy it while `env_files` is [`EnvFiles::Asked`] (`#111`).
    pub env_requested_by: Option<String>,
    /// Agent: a person allowed production CD runs to get its value (SPEC-134); only
    /// [`VaultStore::vault_set_cd_prod`] writes it.
    pub cd_prod: bool,
    /// Agent: agents set it up themselves (SPEC-216): the hosts they ask for are bound at once
    /// and production CD runs get its value. Never together with `cd_prod`.
    pub auto_allow: bool,
}

/// Whether agents may copy an agent secret's value into the project's git-ignored `.env` files
/// (SPEC-129, ADR-0105): a person allows each secret once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvFiles {
    /// Never copied.
    #[default]
    No,
    /// An agent asked; a person allows or dismisses it.
    Asked,
    /// A person allowed it.
    Allowed,
}

impl EnvFiles {
    /// Stored form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Asked => "asked",
            Self::Allowed => "allowed",
        }
    }

    /// Parses the stored form.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "no" => Some(Self::No),
            "asked" => Some(Self::Asked),
            "allowed" => Some(Self::Allowed),
            _ => None,
        }
    }
}

/// A person's two-factor seed (SPEC-040).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactorRecord {
    /// The person (user id).
    pub user_id: String,
    /// The sealed base32 seed.
    pub envelope: Envelope,
    /// When it was set up.
    pub created_at: Timestamp,
    /// When the first correct code confirmed it (`None`: pending).
    pub confirmed_at: Option<Timestamp>,
    /// The time step of the last accepted code.
    pub last_counter: Option<u64>,
    /// Wrong codes in a row.
    pub failed_attempts: u32,
    /// When the last wrong code was entered.
    pub failed_at: Option<Timestamp>,
}

/// One vault audit entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAuditEntry {
    /// When.
    pub at: Timestamp,
    /// Who (a login, `superadmin` or `task:<n>`).
    pub actor: String,
    /// `vault.…` action.
    pub action: String,
    /// Secret id.
    pub target: Option<String>,
    /// Name and context (never a value).
    pub detail: Option<String>,
}

/// Vault storage.
#[async_trait]
pub trait VaultStore: Send + Sync {
    /// Inserts a secret; `Conflict` for an agent name taken in the same scope.
    async fn vault_insert(&self, record: &VaultSecretRecord) -> AppResult<()>;
    /// Replaces every mutable column of the secret `record.id` but its host request; `false` when
    /// it is gone.
    async fn vault_update(&self, record: &VaultSecretRecord) -> AppResult<bool>;
    /// Deletes a secret; `false` when it is gone.
    async fn vault_delete(&self, id: &str) -> AppResult<bool>;
    /// One secret.
    async fn vault_get(&self, id: &str) -> AppResult<Option<VaultSecretRecord>>;
    /// Team secrets by name: those of `sandbox` and the team-wide ones, or every one (`None`).
    async fn vault_team(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<VaultSecretRecord>>;
    /// Agent secrets that apply to `sandbox` (its own and the instance-wide ones), or every
    /// agent secret when `None`.
    async fn vault_agent(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<VaultSecretRecord>>;
    /// Replaces an agent secret's host request when it still is `from` (compare and set, so an
    /// answer never drops a newer request); `false` when it changed or the secret is gone.
    async fn vault_set_request(
        &self,
        id: &str,
        from: &[String],
        to: &[String],
        by: Option<&str>,
    ) -> AppResult<bool>;
    /// Answers an agent secret's host request in one step: `hosts` replace its bindings and the
    /// request is cleared, only while the request still is `request` and the bindings still are
    /// `bound` (what the person saw); `false` otherwise. Nothing else in the row changes.
    async fn vault_answer_request(
        &self,
        id: &str,
        request: &[String],
        bound: &[String],
        hosts: &[String],
        by: &str,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Binds the hosts an agent asked for of a secret it sets up itself (SPEC-216 SSA-002):
    /// `hosts` replace its bindings and `requested` its waiting request, only while they still
    /// are `bound` and `waiting` and the secret still has `auto_allow`; `false` otherwise.
    async fn vault_auto_bind(
        &self,
        id: &str,
        (bound, hosts): (&[String], &[String]),
        (waiting, requested): (&[String], &[String]),
    ) -> AppResult<bool>;
    /// Sets whether agents may copy an agent secret into `.env` files, and who asks, only while
    /// they still are `from` (compare and set); `false` otherwise. Nothing else in the row
    /// changes.
    async fn vault_set_env_files(
        &self,
        id: &str,
        from: (EnvFiles, Option<&str>),
        to: (EnvFiles, Option<&str>),
    ) -> AppResult<bool>;
    /// Sets whether production CD runs get the agent secret `id` (SPEC-134) when it still is
    /// `from` (compare and set) and agents do not set it up themselves (`auto_allow`, SPEC-216
    /// SSA-004); `false` otherwise.
    async fn vault_set_cd_prod(&self, id: &str, from: bool, to: bool) -> AppResult<bool>;
    /// The secrets that applied to every sandbox when schema version 35 was reached and are not
    /// assigned to sandboxes yet (SPEC-216 SCO-004).
    async fn vault_scope_pending(&self) -> AppResult<Vec<String>>;
    /// Replaces the pending secret `id` by `copies` (one per sandbox) and takes it off the
    /// pending list, in one transaction; with no copies it stays as it is (team-wide) and only
    /// leaves the list. `false`, and nothing but the list changes, when it was not pending or no
    /// longer applies to every sandbox.
    async fn vault_assign(&self, id: &str, copies: &[VaultSecretRecord]) -> AppResult<bool>;
    /// Records an agent's use.
    async fn vault_touch_used(&self, id: &str, at: Timestamp) -> AppResult<()>;
    /// Records a re-authentication of a browser session.
    async fn vault_set_reauth(&self, session_hash: &str, at: Timestamp) -> AppResult<()>;
    /// The session's last re-authentication.
    async fn vault_reauth(&self, session_hash: &str) -> AppResult<Option<Timestamp>>;
    /// The person's two-factor seed.
    async fn vault_two_factor(&self, user_id: &str) -> AppResult<Option<TwoFactorRecord>>;
    /// Stores a new seed unless a confirmed one exists; `false` when it does.
    async fn vault_put_two_factor(&self, record: &TwoFactorRecord) -> AppResult<bool>;
    /// Accepts a code of time step `counter` checked against the sealed seed `ciphertext`:
    /// records it, confirms the seed at `at` and clears the wrong codes; `false` when a code of
    /// this or a later step was accepted before (a replay) or the seed was replaced meanwhile.
    async fn vault_accept_two_factor(
        &self,
        user_id: &str,
        ciphertext: &[u8],
        counter: u64,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// Counts a wrong code at `at`; returns the wrong codes in a row (0 without a seed).
    async fn vault_two_factor_failed(&self, user_id: &str, at: Timestamp) -> AppResult<u32>;
    /// Clears the wrong codes (the person confirmed it is them).
    async fn vault_two_factor_clear_failures(&self, user_id: &str) -> AppResult<()>;
    /// Removes the person's two-factor seed and the vault unlocks of their browser sessions (an
    /// administrator's reset); `true` when a seed was removed.
    async fn vault_reset_two_factor(&self, user_id: &str) -> AppResult<bool>;
    /// Records a two-factor unlock of a browser session.
    async fn vault_set_unlock(&self, session_hash: &str, at: Timestamp) -> AppResult<()>;
    /// The session's last two-factor unlock.
    async fn vault_unlock(&self, session_hash: &str) -> AppResult<Option<Timestamp>>;
    /// Sealed vault key material by name.
    async fn vault_meta(&self, name: &str) -> AppResult<Option<Envelope>>;
    /// Stores key material unless `name` exists; `false` when it existed (another writer won).
    async fn vault_insert_meta(
        &self,
        name: &str,
        envelope: &Envelope,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// The newest vault audit entries (`vault.*`), newest first.
    async fn vault_audit(&self, limit: u32) -> AppResult<Vec<VaultAuditEntry>>;
}
