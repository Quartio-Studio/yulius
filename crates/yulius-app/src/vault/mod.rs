//! Secrets vault (SPEC-001 VLT-001..009, SPEC-002 ADR-20).
//!
//! Two sections: **team secrets**, each of one sandbox or team-wide (SPEC-216; the people who can
//! view its sandbox, or every member, list and edit them; revealing a value or notes needs the
//! person's two-factor unlock within 10 minutes, and is audited), and **agent secrets** (agents
//! use them without seeing them: the egress proxy adds them to requests for their bound hosts,
//! `browser_fill_secret` types them into pages of those hosts, and every channel through which an
//! agent reads is redacted of their values and encodings), each a project's, global or one
//! person's (SPEC-245, [`Holder`]).
//!
//! Every value is stored under envelope encryption ([`crate::ports::Crypto::envelope_seal`])
//! with the row's id and section as associated data; plaintexts are zeroed when dropped.

mod agents;
mod create;
mod env_files;
pub(crate) mod reach;
mod requests;
mod runtime;
mod scopes;
mod team;
mod terminal;
mod two_factor;
mod users;
mod verify;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use yulius_domain::ids::SandboxId;
use yulius_domain::vault::encode::needles;
use yulius_domain::vault::redact::{PlainMatcher, redact_text};
use yulius_domain::vault::{MAX_FIELD_BYTES, VaultError};
use zeroize::{Zeroize, Zeroizing};

pub use reach::Holder;
pub use runtime::{EGRESS_CA_FILE, FINGERPRINTS_FILE, SCRUB_EXEC_FLAG, SCRUB_FILE_ENV};
pub use team::Revealed;
pub use terminal::TerminalScrub;
pub(crate) use users::SecretUser;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{VaultSecretRecord, VaultSection};
use crate::vidoc_key::{VIDOC_HOST, VIDOC_SECRET, VIDOC_SECRET_ID};

/// Bound of remembered "already audited" keys before the memory is cleared.
const AUDITED_KEYS_MAX: usize = 10_000;

/// Runtime state of the vault.
#[derive(Default)]
pub struct VaultState {
    /// Bumped on every agent-secret change (redaction caches follow it).
    generation: AtomicU64,
    /// Plain matchers of agent secrets per sandbox and holder (decrypted needles, in memory
    /// only).
    matchers: Mutex<HashMap<String, (u64, Arc<PlainMatcher>)>>,
    /// The egress proxy is wired (composition): runs get the proxy environment.
    egress: AtomicBool,
    /// CA bundle for runs: the host's roots plus the egress CA (`None`: roots not found).
    ca_bundle: OnceLock<Option<String>>,
    /// Uses and redactions already audited (attempt, secret, context).
    audited: Mutex<HashSet<String>>,
    /// Recently authenticated egress proxy clients.
    clients: runtime::ClientCache,
    /// Proxy credentials of running CI jobs (SPEC-142).
    ci_clients: runtime::CiClients,
}

impl VaultState {
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// Converts a validation error into an `Invalid` application error.
fn invalid(err: VaultError) -> AppError {
    AppError::Invalid(err.to_string())
}

/// A text field: trimmed (`trim`), at most [`MAX_FIELD_BYTES`], no NUL; empty means absent.
fn field(value: Option<&str>, name: &str, trim: bool) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = if trim { value.trim() } else { value };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_FIELD_BYTES || value.contains('\0') {
        return Err(AppError::Invalid(format!(
            "{name} is too long or malformed"
        )));
    }
    Ok(Some(value.to_owned()))
}

/// Team secret payload (sealed as JSON): a value and notes (SPEC-040).
#[derive(Default, Serialize, Deserialize)]
#[serde(from = "StoredTeamPayload")]
pub(crate) struct TeamPayload {
    pub value: Option<String>,
    pub notes: Option<String>,
}

impl Drop for TeamPayload {
    fn drop(&mut self) {
        for value in [&mut self.value, &mut self.notes].into_iter().flatten() {
            value.zeroize();
        }
    }
}

impl TeamPayload {
    fn fields(&self) -> Vec<String> {
        [
            ("value", self.value.is_some()),
            ("notes", self.notes.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name.to_owned())
        .collect()
    }
}

/// A sealed team payload in either form: the current one, or the one before SPEC-040 (an
/// address, a user name, a password, notes and a TOTP seed).
#[derive(Default, Deserialize)]
struct StoredTeamPayload {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    totp: Option<String>,
}

impl Drop for StoredTeamPayload {
    fn drop(&mut self) {
        for value in [
            &mut self.value,
            &mut self.url,
            &mut self.username,
            &mut self.password,
            &mut self.notes,
            &mut self.totp,
        ]
        .into_iter()
        .flatten()
        {
            value.zeroize();
        }
    }
}

impl From<StoredTeamPayload> for TeamPayload {
    /// Folds the old form without losing a value: the password becomes the value, and the
    /// address, user name and seed lead the notes.
    fn from(mut stored: StoredTeamPayload) -> Self {
        let mut lines: Vec<String> = [
            ("Address", stored.url.take()),
            ("User name", stored.username.take()),
            ("Two-factor seed", stored.totp.take()),
        ]
        .into_iter()
        .filter_map(|(label, value)| {
            value.map(|mut v| {
                let line = format!("{label}: {v}");
                v.zeroize();
                line
            })
        })
        .collect();
        let notes = stored.notes.take();
        let notes = if lines.is_empty() {
            notes
        } else {
            if let Some(notes) = notes {
                lines.push(String::new());
                lines.push(notes);
            }
            let joined = lines.join("\n");
            for line in &mut lines {
                line.zeroize();
            }
            Some(joined)
        };
        Self {
            value: stored.value.take().or_else(|| stored.password.take()),
            notes,
        }
    }
}

/// Why a production secret refuses a use by agents (SPEC-134, round 2 M-B): one that production
/// deploys get is never also copied into `.env` files or sent to hosts.
pub(crate) const PRODUCTION_SECRET: &str = "this is a production secret: production deploys get it \
and agents never do. Add a separate secret for agents";

/// Why `browser_fill_secret` refuses a personal secret (SPEC-245 RES-005).
pub(crate) const PERSONAL_SECRET_BROWSER: &str = "a personal secret is never typed into the \
project browser: every task of this project shares it and would be signed in as its person. Only \
the proxy sends a personal secret, to its hosts; a website sign-in needs a project secret";

/// Why `secret_copy_to_env` refuses a personal secret (SPEC-245 RES-005).
pub(crate) const PERSONAL_SECRET_ENV: &str = "a personal secret is never copied into .env files: \
every task of this project can read them. Only the proxy sends a personal secret, to its hosts; \
code that needs the value outside the proxy needs a project secret";

/// Agent secret payload (sealed as JSON).
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct AgentPayload {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub totp: Option<String>,
}

impl Drop for AgentPayload {
    fn drop(&mut self) {
        self.value.zeroize();
        for value in [&mut self.username, &mut self.totp].into_iter().flatten() {
            value.zeroize();
        }
    }
}

impl AgentPayload {
    fn fields(&self) -> Vec<String> {
        let mut out = vec!["value".to_owned()];
        if self.username.is_some() {
            out.push("username".into());
        }
        if self.totp.is_some() {
            out.push("totp".into());
        }
        out
    }
}

fn aad(id: &str, section: VaultSection) -> Vec<u8> {
    format!("{id}|{}", section.as_str()).into_bytes()
}

impl App {
    /// Seals a payload for the row `id`.
    fn seal_payload<T: Serialize>(
        &self,
        id: &str,
        section: VaultSection,
        payload: &T,
    ) -> AppResult<crate::ports::Envelope> {
        let bytes = Zeroizing::new(serde_json::to_vec(payload).map_err(AppError::internal)?);
        self.deps.crypto.envelope_seal(&aad(id, section), &bytes)
    }

    /// Opens a row's payload.
    fn open_payload<T: serde::de::DeserializeOwned>(
        &self,
        record: &VaultSecretRecord,
    ) -> AppResult<T> {
        let bytes = self
            .deps
            .crypto
            .envelope_open(&aad(&record.id, record.section), &record.envelope)?;
        serde_json::from_slice(&bytes).map_err(|_| AppError::Internal("vault payload".into()))
    }

    /// Appends a vault audit entry; a failure fails the operation (every reveal is audited).
    async fn vault_audit(
        &self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: &str,
    ) -> AppResult<()> {
        self.deps
            .store
            .audit(self.now(), actor, action, target, Some(detail))
            .await
    }

    /// Audits once per key (agent uses and redactions repeat for every request).
    async fn vault_audit_once(
        &self,
        key: String,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: &str,
    ) {
        let first = match self.vault.audited.lock() {
            Ok(mut seen) => {
                if seen.len() >= AUDITED_KEYS_MAX {
                    seen.clear();
                }
                seen.insert(key)
            }
            Err(_) => true,
        };
        if first && let Err(err) = self.vault_audit(actor, action, target, detail).await {
            tracing::warn!(error = %err, action, "vault audit entry not written");
        }
    }

    /// The person's audit label.
    fn vault_actor(ctx: &AuthContext) -> String {
        ctx.user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into())
    }

    /// The agent secrets of `sandbox` that runs of `holder` get (SPEC-245 RES-001..004): one per
    /// name, the project's before the holder's personal one before a global one switched on for
    /// the project. Not the instance Vidoc key: CD and MCP references never get it.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn resolved_agent_secrets(
        &self,
        sandbox: &SandboxId,
        holder: Holder<'_>,
    ) -> AppResult<Vec<VaultSecretRecord>> {
        let candidates = self.deps.store.vault_agent_in(sandbox).await?;
        Ok(reach::resolve(candidates, holder))
    }

    /// The agent secrets an agent run, the Superagent or a granted CI job uses through the egress
    /// proxy: [`Self::resolved_agent_secrets`] and, when the instance has a Vidoc key and none of
    /// them is a `VIDOC_API_KEY`, the instance key as that virtual secret (SPEC-195 VDK-005).
    /// Never for MCP references, `.env` copies or browser filling, which would hand out its value.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn applied_agent_secrets(
        &self,
        sandbox: &SandboxId,
        holder: Holder<'_>,
    ) -> AppResult<Vec<VaultSecretRecord>> {
        let mut records = self.resolved_agent_secrets(sandbox, holder).await?;
        if self.vidoc_key().is_some() && !records.iter().any(|r| r.name == VIDOC_SECRET) {
            records.push(instance_vidoc_record());
        }
        Ok(records)
    }

    /// Whether runs of `task` get a personal secret (its owner's, switched on for its project).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn holds_personal_secrets(
        &self,
        task: &crate::model::TaskRecord,
    ) -> AppResult<bool> {
        Ok(self
            .resolved_agent_secrets(&task.sandbox_id, Holder::of_task(task))
            .await?
            .iter()
            .any(|r| matches!(r.scope(), crate::ports::vault::AgentScope::Personal(_))))
    }

    /// Forgets the cached proxy credentials of `task`'s runs: its owner changed, so the next
    /// request resolves the new owner's personal secrets (SPEC-245 review M2).
    pub(crate) fn forget_egress_clients(&self, task: &yulius_domain::ids::TaskId) {
        if let Ok(mut cache) = self.vault.clients.0.lock() {
            cache.retain(|(_, client, _)| {
                !matches!(&client.caller, crate::ports::egress::EgressCaller::Agent { task_id, .. } if task_id == task)
            });
        }
    }

    /// Every agent secret that can reach some run of `sandbox` (`holder: None`: every person's
    /// personal ones switched on for it, RES-006), or a run of `holder` (its own personal ones,
    /// never another person's), with the instance Vidoc key: what redaction covers.
    async fn redacted_agent_secrets(
        &self,
        sandbox: &SandboxId,
        holder: Option<Holder<'_>>,
    ) -> AppResult<Vec<VaultSecretRecord>> {
        let mut records = self.deps.store.vault_agent_in(sandbox).await?;
        if let Some(holder) = holder {
            records.retain(|r| holder.may_hold(r));
        }
        if self.vidoc_key().is_some() {
            records.push(instance_vidoc_record());
        }
        Ok(records)
    }

    /// A record's payload: the instance Vidoc key for its virtual record (`None` once removed),
    /// else the sealed payload.
    ///
    /// # Errors
    /// Decryption failures.
    pub(crate) fn agent_payload(
        &self,
        record: &VaultSecretRecord,
    ) -> AppResult<Option<AgentPayload>> {
        if record.id == VIDOC_SECRET_ID {
            return Ok(self.vidoc_key().map(|key| AgentPayload {
                username: None,
                value: key.expose().to_owned(),
                totp: None,
            }));
        }
        self.open_payload(record).map(Some)
    }

    /// Marks agent secrets as changed: redaction caches reload (also when the instance Vidoc key,
    /// a virtual agent secret, changes; SPEC-195 VDK-005).
    pub(crate) fn bump_vault_generation(&self) {
        self.vault.bump();
    }

    /// The redaction matcher of every agent secret that can reach a run of `sandbox` (values,
    /// TOTP seeds and their encodings), cached until an agent secret changes. Yulius' own
    /// redaction of the project's text uses it (RES-006).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn agent_secret_matcher(&self, sandbox: &SandboxId) -> AppResult<Arc<PlainMatcher>> {
        self.matcher_for(sandbox, None).await
    }

    /// The redaction matcher of `sandbox` for runs of `holder` (`None`: for every run), cached
    /// until an agent secret changes. A run's own fingerprint file is made from its holder's, so
    /// it never holds hashes of another person's values (ADR-0186).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub(crate) async fn matcher_for(
        &self,
        sandbox: &SandboxId,
        holder: Option<Holder<'_>>,
    ) -> AppResult<Arc<PlainMatcher>> {
        let generation = self.vault.generation.load(Ordering::SeqCst);
        let key = format!(
            "{}|{}",
            sandbox.as_str(),
            holder.map_or_else(|| "*".to_owned(), Holder::cache_key)
        );
        if let Ok(cache) = self.vault.matchers.lock()
            && let Some((cached, matcher)) = cache.get(&key)
            && *cached == generation
        {
            return Ok(matcher.clone());
        }
        let records = self.redacted_agent_secrets(sandbox, holder).await?;
        let mut secrets = Vec::with_capacity(records.len());
        for record in &records {
            let Some(payload) = self.agent_payload(record)? else {
                continue;
            };
            let mut forms = needles(&payload.value);
            // A key copied into a file (`.env`) shows as its lines (SPEC-186 SAV-005).
            for line in yulius_domain::vault::dotenv::key_lines(&payload.value) {
                forms.extend(needles(&line));
            }
            if let Some(seed) = &payload.totp {
                forms.extend(needles(seed));
            }
            secrets.push((record.name.clone(), forms));
        }
        let matcher = Arc::new(PlainMatcher::new(secrets));
        if let Ok(mut cache) = self.vault.matchers.lock() {
            if cache.len() > 256 {
                cache.clear();
            }
            cache.insert(key, (generation, matcher.clone()));
        }
        Ok(matcher)
    }

    /// Replaces every value of an agent secret that applies to `sandbox`, and its encodings,
    /// with `[secret:NAME]`. Every surface that shows agents or people text from a sandbox
    /// (MCP results, transcripts) applies it; on a failure the text is withheld, never shown
    /// unredacted.
    pub async fn redact_for_sandbox(&self, sandbox: &SandboxId, text: &str) -> String {
        match self.agent_secret_matcher(sandbox).await {
            Ok(matcher) => redact_text(matcher.as_ref(), text).0,
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be loaded for redaction");
                "[withheld: Yulius could not check this text for agent secrets]".into()
            }
        }
    }

    /// Whether the egress proxy is wired: runs get the proxy environment.
    pub(crate) fn egress_wired(&self) -> bool {
        self.vault.egress.load(Ordering::SeqCst)
    }

    /// Wires the egress proxy (composition): agent runs get the proxy environment and, when
    /// `ca_bundle` is set, a CA bundle (the host's roots plus the egress CA).
    pub fn enable_egress(&self, ca_bundle: Option<String>) {
        let _ = self.vault.ca_bundle.set(ca_bundle);
        self.vault.egress.store(true, Ordering::SeqCst);
    }
}

/// The instance Vidoc key as an agent secret record (SPEC-195 VDK-005): bound to Vidoc's API
/// host only, added by placeholder, never sealed in the vault (its payload comes from
/// [`App::vidoc_key`]).
fn instance_vidoc_record() -> VaultSecretRecord {
    let never = yulius_domain::time::Timestamp(0);
    VaultSecretRecord {
        id: VIDOC_SECRET_ID.into(),
        section: VaultSection::Agent,
        sandbox_id: None,
        owner_id: None,
        name: VIDOC_SECRET.into(),
        description: "The instance's Vidoc key (Settings, Instance): Vidoc's vidoc-1 security \
                      scan, `yulius security-scan`"
            .into(),
        hosts: vec![VIDOC_HOST.into()],
        injection: "none".into(),
        header_name: None,
        fields: vec!["value".into()],
        envelope: crate::ports::Envelope {
            kek_id: 0,
            wrapped_key: Vec::new(),
            ciphertext: Vec::new(),
        },
        created_by: "instance".into(),
        created_at: never,
        updated_by: "instance".into(),
        updated_at: never,
        value_updated_at: never,
        last_used_at: None,
        requested_hosts: Vec::new(),
        requested_by: None,
        env_files: crate::ports::vault::EnvFiles::No,
        env_requested_by: None,
        cd_prod: false,
        auto_allow: false,
    }
}
