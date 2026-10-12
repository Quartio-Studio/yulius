//! Secrets vault (SPEC-001 VLT-001..009, SPEC-002 ADR-20).
//!
//! Two sections: **team secrets** (every member lists and edits them; revealing a value or notes
//! needs the person's two-factor unlock within 10 minutes, and is audited) and
//! **agent secrets** (agents use them without seeing them: the egress proxy adds them to
//! requests for their bound hosts, `browser_fill_secret` types them into pages of those hosts,
//! and every channel through which an agent reads is redacted of their values and encodings).
//!
//! Every value is stored under envelope encryption ([`crate::ports::Crypto::envelope_seal`])
//! with the row's id and section as associated data; plaintexts are zeroed when dropped.

mod agents;
mod runtime;
mod team;
mod terminal;
mod two_factor;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use yulius_domain::ids::SandboxId;
use yulius_domain::vault::encode::needles;
use yulius_domain::vault::redact::{PlainMatcher, redact_text};
use yulius_domain::vault::{MAX_FIELD_BYTES, VaultError};
use zeroize::{Zeroize, Zeroizing};

pub use runtime::{EGRESS_CA_FILE, FINGERPRINTS_FILE, SCRUB_EXEC_FLAG, SCRUB_FILE_ENV};
pub use team::Revealed;
pub use terminal::TerminalScrub;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::vault::{VaultSecretRecord, VaultSection};

/// Bound of remembered "already audited" keys before the memory is cleared.
const AUDITED_KEYS_MAX: usize = 10_000;

/// Runtime state of the vault.
#[derive(Default)]
pub struct VaultState {
    /// Bumped on every agent-secret change (redaction caches follow it).
    generation: AtomicU64,
    /// Plain matchers of agent secrets per sandbox (decrypted needles, in memory only).
    matchers: Mutex<HashMap<String, (u64, Arc<PlainMatcher>)>>,
    /// The egress proxy is wired (composition): runs get the proxy environment.
    egress: AtomicBool,
    /// CA bundle for runs: the host's roots plus the egress CA (`None`: roots not found).
    ca_bundle: OnceLock<Option<String>>,
    /// Uses and redactions already audited (attempt, secret, context).
    audited: Mutex<HashSet<String>>,
    /// Recently authenticated egress proxy clients.
    clients: runtime::ClientCache,
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

    /// The redaction matcher of every agent secret that applies to `sandbox` (values, TOTP
    /// seeds and their encodings), cached until an agent secret changes.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn agent_secret_matcher(&self, sandbox: &SandboxId) -> AppResult<Arc<PlainMatcher>> {
        let generation = self.vault.generation.load(Ordering::SeqCst);
        if let Ok(cache) = self.vault.matchers.lock()
            && let Some((cached, matcher)) = cache.get(sandbox.as_str())
            && *cached == generation
        {
            return Ok(matcher.clone());
        }
        let records = self.deps.store.vault_agent(Some(sandbox)).await?;
        let mut secrets = Vec::with_capacity(records.len());
        for record in &records {
            let payload: AgentPayload = self.open_payload(record)?;
            let mut forms = needles(&payload.value);
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
            cache.insert(sandbox.as_str().to_owned(), (generation, matcher.clone()));
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

    /// Wires the egress proxy (composition): agent runs get the proxy environment and, when
    /// `ca_bundle` is set, a CA bundle (the host's roots plus the egress CA).
    pub fn enable_egress(&self, ca_bundle: Option<String>) {
        let _ = self.vault.ca_bundle.set(ca_bundle);
        self.vault.egress.store(true, Ordering::SeqCst);
    }
}
