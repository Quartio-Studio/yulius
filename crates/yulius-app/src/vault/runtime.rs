//! Agent secrets at run time (VLT-004..008): the run environment (proxy, CA bundle, placeholders,
//! the Bash wrapper and its fingerprints), the egress policy, the MCP tools and redaction.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::browser::{TabId, TypeParams};
use yulius_contracts::scrub::{FINGERPRINT_VERSION, Fingerprint, FingerprintSet, RedactionReport};
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::vault::hosts::HostPattern;
use yulius_domain::vault::redact::{Matcher, PlainMatcher, StreamRedactor, rolling_hash};
use yulius_domain::vault::{
    Injection, PLACEHOLDER_PREFIX, SecretName, placeholder, substitute_placeholders,
};

use super::AgentPayload;
use crate::context::App;
use crate::coordination::{AgentContext, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::egress::{
    EgressCaller, EgressClient, EgressDecision, EgressPolicy, EgressRequest,
};
use crate::ports::session::{Containment, LaunchPlan, PlannedFile};
use crate::ports::vault::{AgentScope, VaultSecretRecord};

/// First argument of `yulius` that runs the Bash wrapper (`CLAUDE_CODE_SHELL_PREFIX`).
pub const SCRUB_EXEC_FLAG: &str = "--scrub-exec";
/// The run's fingerprint file (fallback when Yulius cannot be reached).
pub const FINGERPRINTS_FILE: &str = "scrub.json";
/// The run's CA bundle: the host's roots plus the egress CA.
pub const EGRESS_CA_FILE: &str = "egress-ca.pem";
/// Environment variable naming the fingerprint file.
pub const SCRUB_FILE_ENV: &str = "YULIUS_SCRUB_FILE";
/// Hosts whose traffic never goes through the proxy: loopback, and Claude Code's own API so a
/// Yulius restart never interrupts a turn.
const NO_PROXY_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    "anthropic.com",
    ".anthropic.com",
    "claude.ai",
    ".claude.ai",
];
/// Variables that select a CA bundle in common clients (OpenSSL, Python requests, curl, Git,
/// Node.js).
const CA_ENV: &[&str] = &[
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "NODE_EXTRA_CA_CERTS",
];
/// How long an egress credential stays cached.
const CLIENT_CACHE_TTL: Duration = Duration::from_secs(15);
/// Cached egress credentials.
const CLIENT_CACHE_MAX: usize = 1024;

/// Recently authenticated proxy clients: (token digest, client, when).
#[derive(Default)]
pub(crate) struct ClientCache(Mutex<Vec<(Vec<u8>, EgressClient, Instant)>>);

/// Most CI jobs holding a proxy credential at once (far above the CI's job slots).
const CI_CLIENTS_MAX: usize = 256;

/// The proxy credentials of running CI jobs (SPEC-142 CSE-004): (token digest, client). In
/// memory only, removed when the job ends, never cached.
#[derive(Default)]
pub(crate) struct CiClients(Mutex<Vec<(Vec<u8>, EgressClient)>>);

/// The authority (`host:port`) of an `http://` URL.
fn authority(url: &str) -> Option<&str> {
    url.strip_prefix("http://")?.split('/').next()
}

/// Host and port of a page URL (`https` 443, `http` 80).
fn page_host(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    if authority.starts_with('[') {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_ascii_lowercase(), port.parse().ok()?)),
        None => Some((authority.to_ascii_lowercase(), default)),
    }
}

/// Binds `files` (written outside the sandbox, in a run directory) read-only into a contained
/// session, so it reads them where the plan names them; nothing to do without containment.
fn bind_read_only(containment: &mut Containment, files: &[PlannedFile]) {
    if let Containment::Bubblewrap { readonly, .. } = containment {
        readonly.extend(files.iter().map(|f| f.path.clone()));
    }
}

fn bound_to(record: &VaultSecretRecord, host: &str, port: u16) -> bool {
    record
        .hosts
        .iter()
        .filter_map(|h| HostPattern::parse(h).ok())
        .any(|p| p.matches(host, port))
}

fn injection_of(record: &VaultSecretRecord) -> Injection {
    Injection::parse(&record.injection, record.header_name.as_deref()).unwrap_or(Injection::None)
}

/// Replaces placeholders in one header value (also inside `Basic` credentials).
fn substitute_header(
    name: &str,
    value: &[u8],
    resolve: &dyn Fn(&str) -> Option<String>,
    used: &mut Vec<String>,
) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(value) else {
        return value.to_vec();
    };
    if name == "authorization"
        && let Some(encoded) = text.strip_prefix("Basic ")
        && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim())
        && let Ok(decoded) = String::from_utf8(decoded)
        && let Some(sub) = substitute_placeholders(&decoded, resolve)
    {
        used.extend(sub.used);
        return format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(sub.text.as_bytes())
        )
        .into_bytes();
    }
    match substitute_placeholders(text, resolve) {
        Some(sub) => {
            used.extend(sub.used);
            sub.text.into_bytes()
        }
        None => value.to_vec(),
    }
}

/// Arguments of `browser_fill_secret`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillArgs {
    tab_id: TabId,
    secret: String,
    #[serde(default)]
    field: Option<String>,
    #[serde(default)]
    selector: Option<String>,
}

/// For whom a vault tool's caller resolves its project's secrets: its task's holder, or the
/// Superagent's person (SPEC-245 RES-002).
pub(crate) fn caller_holder(caller: &ProjectCaller) -> super::Holder<'_> {
    caller
        .task
        .as_ref()
        .map_or(super::Holder::Person(&caller.user), super::Holder::of_task)
}

/// Name of the egress CA's sealed material in `vault_meta`.
const EGRESS_CA_META: &str = "egress-ca";
/// Associated data of the egress CA's envelope.
const EGRESS_CA_AAD: &[u8] = b"vault-meta|egress-ca";

impl App {
    /// The egress proxy's CA material (sealed in the vault): loaded, or made by `generate` and
    /// stored on first use (a concurrent first use keeps the stored one).
    ///
    /// # Errors
    /// Store, decryption or generation failures.
    pub async fn egress_ca_material(
        &self,
        generate: impl FnOnce() -> Result<String, String>,
    ) -> AppResult<String> {
        let open = |envelope: &crate::ports::Envelope| -> AppResult<String> {
            let bytes = self.deps.crypto.envelope_open(EGRESS_CA_AAD, envelope)?;
            String::from_utf8(bytes.to_vec()).map_err(|_| AppError::Internal("egress CA".into()))
        };
        if let Some(envelope) = self.deps.store.vault_meta(EGRESS_CA_META).await? {
            return open(&envelope);
        }
        let material = zeroize::Zeroizing::new(generate().map_err(AppError::Internal)?);
        let envelope = self
            .deps
            .crypto
            .envelope_seal(EGRESS_CA_AAD, material.as_bytes())?;
        if self
            .deps
            .store
            .vault_insert_meta(EGRESS_CA_META, &envelope, self.now())
            .await?
        {
            return Ok(material.to_string());
        }
        let stored = self
            .deps
            .store
            .vault_meta(EGRESS_CA_META)
            .await?
            .ok_or_else(|| AppError::Internal("egress CA".into()))?;
        open(&stored)
    }

    /// The value of the agent secret `name` that applies to `sandbox` (MCP `${secret:NAME}`
    /// references, VLT-009): its project secret, else a global one switched on for it; never a
    /// personal one, whose value would reach a file system every task of the sandbox reads
    /// (SPEC-245 RES-005). A production secret (SPEC-134) never resolves: its value would reach
    /// an agent run's environment.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub(crate) async fn agent_secret_value(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<Option<zeroize::Zeroizing<String>>> {
        let Some(record) = self
            .resolved_agent_secrets(sandbox, super::Holder::Nobody)
            .await?
            .into_iter()
            .find(|r| r.name == name && !r.cd_prod)
        else {
            return Ok(None);
        };
        let payload: AgentPayload = self.open_payload(&record)?;
        Ok(Some(zeroize::Zeroizing::new(payload.value.clone())))
    }

    /// Whose personal secrets apply to `client` now: an agent run's task's current holder (read
    /// per request, so a hand-over, a deleted task or an automation's origin applies also to
    /// connections and cached credentials from before it), a Superagent's person, no CI job's.
    async fn client_person(&self, client: &EgressClient) -> AppResult<Option<UserId>> {
        match &client.caller {
            EgressCaller::Agent { task_id, .. } => Ok(self
                .deps
                .store
                .task(task_id)
                .await?
                .and_then(|task| super::Holder::of_task(&task).person().cloned())),
            EgressCaller::Ci { .. } => Ok(None),
            EgressCaller::Superagent { person, .. } => Ok(Some(person.clone())),
        }
    }

    /// The agent secrets that apply to `client` now, by its current holder.
    async fn client_secrets(&self, client: &EgressClient) -> AppResult<Vec<VaultSecretRecord>> {
        let person = self.client_person(client).await?;
        let holder = person
            .as_ref()
            .map_or(super::Holder::Nobody, super::Holder::Person);
        self.applied_agent_secrets(&client.sandbox_id, holder).await
    }

    /// The agent secrets that apply to `client` and are bound to `host:port`, with their
    /// payloads.
    async fn bound_secrets(
        &self,
        client: &EgressClient,
        host: &str,
        port: u16,
    ) -> AppResult<Vec<(VaultSecretRecord, AgentPayload)>> {
        let mut out = Vec::new();
        for record in self.client_secrets(client).await? {
            if client.may_use(&record.name, record.cd_prod)
                && bound_to(&record, host, port)
                && let Some(payload) = self.agent_payload(&record)?
            {
                out.push((record, payload));
            }
        }
        Ok(out)
    }

    /// Records a use of a secret by `actor` (`task:<n>`, `ci:<run>`): `last_used_at`, and one
    /// audit entry per run (`run`: the agent's attempt or the CI job), secret and context.
    pub(super) async fn note_agent_use(
        &self,
        (run, actor): (&str, &str),
        record: &VaultSecretRecord,
        action: &str,
        context: &str,
    ) {
        let _ = self
            .deps
            .store
            .vault_touch_used(&record.id, self.now())
            .await;
        self.vault_audit_once(
            format!("{action}|{run}|{}|{context}", record.id),
            actor,
            action,
            Some(&record.id),
            &format!("{} → {context}", record.name),
        )
        .await;
    }

    /// The fingerprints of every agent secret that can reach runs of `holder` in `sandbox`
    /// (never another person's: SPEC-245, ADR-0186), with a fresh salt and rolling-hash base.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn scrub_fingerprints_for(
        &self,
        sandbox: &SandboxId,
        holder: super::Holder<'_>,
    ) -> AppResult<FingerprintSet> {
        let matcher = self.matcher_for(sandbox, Some(holder)).await?;
        let random = self.deps.crypto.random_bytes();
        let mut base_bytes = [0u8; 8];
        base_bytes.copy_from_slice(&random[..8]);
        let base = (u64::from_le_bytes(base_bytes) % ((1 << 61) - 1 - 512)) + 512;
        let salt = &random[8..24];
        let needles = matcher
            .needles()
            .map(|(needle, secret)| {
                let mut salted = Vec::with_capacity(salt.len() + needle.len());
                salted.extend_from_slice(salt);
                salted.extend_from_slice(needle);
                Fingerprint {
                    secret,
                    len: needle.len(),
                    rolling: rolling_hash(base, needle),
                    digest: self.deps.crypto.sha256_hex(&salted),
                }
            })
            .collect();
        Ok(FingerprintSet {
            version: FINGERPRINT_VERSION,
            base,
            salt: crate::auth::hex_token(salt),
            names: matcher.names().to_vec(),
            needles,
        })
    }

    /// Whose redaction calls with `token`: a live run's attempt (`task:<n>`), or a person's
    /// workspace, where their Superagent runs (SPEC-147 SAM-003). Answers the sandbox, the key a
    /// redaction is audited once per, the audit actor and whose personal secrets apply.
    async fn scrub_caller(
        &self,
        token: &str,
    ) -> AppResult<(SandboxId, String, String, Option<UserId>)> {
        match self.agent_context(token).await {
            Ok(ctx) => Ok((
                ctx.task.sandbox_id.clone(),
                ctx.attempt.id.as_str().to_owned(),
                ctx.actor(),
                super::Holder::of_task(&ctx.task).person().cloned(),
            )),
            Err(AppError::Unauthenticated) => {
                let (user, sandbox) = self.workspace_worker(token).await?;
                let once = format!("ws:{}:{}", user.id.as_str(), sandbox.as_str());
                let actor = user
                    .github_login
                    .clone()
                    .unwrap_or_else(|| "superadmin".into());
                Ok((sandbox, once, actor, Some(user.id)))
            }
            Err(err) => Err(err),
        }
    }

    /// `POST /internal/scrub/fingerprints`: the caller's run's fingerprints, or those of a
    /// person's workspace (SPEC-147 SAM-003).
    ///
    /// # Errors
    /// `Unauthenticated` for unknown or ended runs and for people who may no longer work in the
    /// workspace's sandbox; store failures.
    pub async fn scrub_fingerprints(&self, token: &str) -> AppResult<FingerprintSet> {
        let (sandbox, _, _, person) = self.scrub_caller(token).await?;
        let holder = person
            .as_ref()
            .map_or(super::Holder::Nobody, super::Holder::Person);
        self.scrub_fingerprints_for(&sandbox, holder).await
    }

    /// `POST /internal/scrub/report`: a sandbox process redacted these secrets (audited once
    /// per run, or person and sandbox, secret and channel).
    ///
    /// # Errors
    /// `Unauthenticated`, `Invalid` channel.
    pub async fn scrub_report(&self, token: &str, report: &RedactionReport) -> AppResult<()> {
        let (sandbox, once, actor, person) = self.scrub_caller(token).await?;
        if !matches!(report.channel.as_str(), "bash" | "tool") {
            return Err(AppError::Invalid("channel must be bash or tool".into()));
        }
        let holder = person
            .as_ref()
            .map_or(super::Holder::Nobody, super::Holder::Person);
        let records = self.redacted_agent_secrets(&sandbox, Some(holder)).await?;
        for name in report.names.iter().take(64) {
            if let Some(record) = records.iter().find(|r| &r.name == name) {
                self.vault_audit_once(
                    format!("redacted|{once}|{}|{}", record.id, report.channel),
                    &actor,
                    "vault.agent.redacted",
                    Some(&record.id),
                    &format!("{} in {} output", record.name, report.channel),
                )
                .await;
            }
        }
        Ok(())
    }

    /// Adds the vault's part of an agent run's environment (SPEC-002 ADR-20): placeholders for
    /// every agent secret that applies to runs of `holder` (SPEC-245), the Bash wrapper (`CLAUDE_CODE_SHELL_PREFIX`) and its
    /// fingerprint file, and — when the egress proxy is wired — the proxy variables (with the
    /// run's task token, as secret environment) and the CA bundle.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub(crate) async fn vault_run_setup(
        &self,
        sandbox: &SandboxRecord,
        holder: super::Holder<'_>,
        run_dir: &Path,
        task_token: &str,
        launch: &mut LaunchPlan,
    ) -> AppResult<()> {
        let records = self.applied_agent_secrets(&sandbox.id, holder).await?;
        for record in &records {
            if let Ok(name) = SecretName::parse(&record.name) {
                launch
                    .env
                    .push((name.as_str().to_owned(), placeholder(&name)));
            }
        }
        let fingerprints = self.scrub_fingerprints_for(&sandbox.id, holder).await?;
        let scrub_file = run_dir.join(FINGERPRINTS_FILE);
        launch.files.push(PlannedFile {
            path: scrub_file.clone(),
            contents: serde_json::to_vec(&fingerprints).map_err(AppError::internal)?,
            mode: 0o600,
        });
        launch.env.push((
            SCRUB_FILE_ENV.into(),
            scrub_file.to_string_lossy().into_owned(),
        ));
        launch.env.extend(self.scrub_shell_prefix());
        self.egress_env("yulius", task_token, run_dir, launch);
        Ok(())
    }

    /// Rewrites the fingerprint file of `task`'s live run for the task's holder as it is now
    /// (SPEC-248 FUP-002): after a hand-over (SPEC-070) the Bash wrapper's fallback holds no
    /// hashes of the previous owner's personal values. One rewrite at a time, each reading the
    /// task afresh, so the last one is always the latest holder's. In place, as a contained
    /// session binds the file read-only; a run without one, or a task that is gone, is left alone.
    ///
    /// # Errors
    /// Store, decryption and file failures; `Internal` for a link or a path that is no regular
    /// file (opened without following a link or blocking, checked on the handle).
    pub async fn rewrite_run_fingerprints(&self, task: &TaskId) -> AppResult<()> {
        use nix::fcntl::OFlag;
        use std::os::unix::fs::OpenOptionsExt as _;
        let _one_at_a_time = self.vault.fingerprint_rewrites.lock().await;
        let Some(task) = self.deps.store.task(task).await? else {
            return Ok(());
        };
        let Some(attempt) = self.deps.store.live_attempt(&task.id).await? else {
            return Ok(());
        };
        let file = Path::new(&attempt.run_dir).join(FINGERPRINTS_FILE);
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(&file)
        {
            Ok(out) => out,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(AppError::internal(err)),
        };
        if !out.metadata().map_err(AppError::internal)?.is_file() {
            return Err(AppError::Internal(
                "the run's fingerprint file is no regular file".into(),
            ));
        }
        let set = self
            .scrub_fingerprints_for(&task.sandbox_id, super::Holder::of_task(&task))
            .await?;
        let bytes = serde_json::to_vec(&set).map_err(AppError::internal)?;
        out.set_len(0).map_err(AppError::internal)?;
        std::io::Write::write_all(&mut out, &bytes).map_err(AppError::internal)
    }

    /// Puts a Superagent behind the egress proxy as an agent run is, with its own bearer
    /// (SPEC-186 SAV-001): a placeholder per agent secret of `sandbox` its person (`person`)
    /// gets that its environment does not set already, and — when the proxy is wired — the proxy variables and the CA bundle,
    /// written to its shell's `run_dir` and bound read-only into a contained session.
    ///
    /// # Errors
    /// Store failures: a vault that cannot be read fails the start, as an agent run's does.
    pub(crate) async fn vault_superagent_setup(
        &self,
        sandbox: &SandboxRecord,
        person: &UserId,
        run_dir: &Path,
        bearer: &str,
        launch: &mut LaunchPlan,
    ) -> AppResult<()> {
        for record in self
            .applied_agent_secrets(&sandbox.id, super::Holder::Person(person))
            .await?
        {
            if let Ok(name) = SecretName::parse(&record.name) {
                let set = |(key, _): &(String, String)| key == name.as_str();
                if !launch.env.iter().any(set) && !launch.secret_env.iter().any(set) {
                    launch
                        .env
                        .push((name.as_str().to_owned(), placeholder(&name)));
                }
            }
        }
        let before = launch.files.len();
        self.egress_env("yulius", bearer, run_dir, launch);
        let added: Vec<PlannedFile> = launch.files[before..].to_vec();
        bind_read_only(&mut launch.containment, &added);
        Ok(())
    }

    /// The Bash wrapper of agent sessions and the Superagent (SPEC-147 SAM-002):
    /// `CLAUDE_CODE_SHELL_PREFIX=<yulius> --scrub-exec`. Claude Code runs
    /// `'<prefix before the last " -">' <flag> '<command>'` for Bash commands, hooks and stdio MCP
    /// servers, so a path holding `" -"` cannot be the prefix (`None`, with a warning).
    pub(crate) fn scrub_shell_prefix(&self) -> Option<(String, String)> {
        let exe = self.config.yulius_exe.to_string_lossy();
        if exe.contains(" -") {
            tracing::warn!(
                "the yulius path contains \" -\": Bash output is redacted by the tool hook only"
            );
            return None;
        }
        Some((
            "CLAUDE_CODE_SHELL_PREFIX".into(),
            format!("{exe} {SCRUB_EXEC_FLAG}"),
        ))
    }

    /// Adds the egress proxy to a run's environment when the proxy is wired (`false` when it is
    /// not): the proxy variables with `user:token` (secret environment), `NO_PROXY`, and the CA
    /// bundle written to `run_dir`. Shared by agent runs and CI jobs (SPEC-142).
    pub(crate) fn egress_env(
        &self,
        user: &str,
        token: &str,
        run_dir: &Path,
        launch: &mut LaunchPlan,
    ) -> bool {
        if !self.vault.egress.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let Some(proxy_authority) = authority(&self.config.internal_url) else {
            return false;
        };
        let proxy = format!("http://{user}:{token}@{proxy_authority}");
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            launch.secret_env.push((name.into(), proxy.clone()));
        }
        let internal_host = proxy_authority
            .rsplit_once(':')
            .map_or(proxy_authority, |(host, _)| host);
        let mut no_proxy: Vec<&str> = NO_PROXY_HOSTS.to_vec();
        if !no_proxy.contains(&internal_host) {
            no_proxy.push(internal_host);
        }
        let no_proxy = no_proxy.join(",");
        launch.env.push(("NO_PROXY".into(), no_proxy.clone()));
        launch.env.push(("no_proxy".into(), no_proxy));
        launch.env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
        if let Some(Some(bundle)) = self.vault.ca_bundle.get() {
            let path = run_dir.join(EGRESS_CA_FILE);
            launch.files.push(PlannedFile {
                path: path.clone(),
                contents: bundle.as_bytes().to_vec(),
                mode: 0o600,
            });
            for name in CA_ENV {
                launch
                    .env
                    .push(((*name).to_owned(), path.to_string_lossy().into_owned()));
            }
        }
        true
    }

    /// A streaming redactor of the task terminal for `sandbox` (`None` without agent secrets).
    pub(crate) async fn terminal_redactor(
        &self,
        sandbox: &SandboxId,
    ) -> Option<StreamRedactor<Arc<PlainMatcher>>> {
        match self.agent_secret_matcher(sandbox).await {
            Ok(matcher) if !matcher.is_empty() => Some(super::terminal::screen_redactor(matcher)),
            Ok(_) => None,
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be loaded for the terminal");
                None
            }
        }
    }

    /// The vault's generation (terminal redactors reload when it changes).
    pub(crate) fn vault_generation(&self) -> u64 {
        self.vault
            .generation
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// `secrets_list`: names, descriptions, bindings and placeholders of the agent secrets the
    /// caller's project may use, and how a task's agent or the Superagent (SPEC-186 SAV-007) uses
    /// them; never a value.
    pub(crate) async fn tool_secrets_list(&self, caller: &ProjectCaller) -> AppResult<Value> {
        let records = self
            .applied_agent_secrets(&caller.sandbox, caller_holder(caller))
            .await?;
        let secrets: Vec<Value> = records
            .iter()
            .map(|r| {
                let scope = if r.id == crate::vidoc_key::VIDOC_SECRET_ID {
                    "the instance (its Vidoc key, set by the superadmin)"
                } else {
                    match r.scope() {
                        AgentScope::Project(_) => "this project",
                        AgentScope::Global => "global (an administrator switched it on for this project)",
                        AgentScope::Personal(_) => {
                            "personal (your person's own: only the proxy sends it; never .env copies, MCP references or the browser)"
                        }
                    }
                };
                json!({
                    "name": r.name,
                    "description": r.description,
                    "scope": scope,
                    "hosts": r.hosts,
                    "requested_hosts": r.requested_hosts,
                    "adds": injection_of(r).describe(),
                    "placeholder": format!("{PLACEHOLDER_PREFIX}{}", r.name),
                    "fields": r.fields,
                    "env_files": r.env_files.as_str(),
                    "auto_allow": r.auto_allow,
                })
            })
            .collect();
        // Folders of operator state sent from a laptop (SPEC-271 OPS-007): names, never content.
        let states = self.states_for_list(&caller.sandbox).await?;
        if caller.task.is_none() {
            let sandbox = self
                .deps
                .store
                .sandbox(&caller.sandbox)
                .await?
                .ok_or(AppError::NotFound)?;
            return Ok(json!({
                "secrets": secrets,
                "states": states,
                "how_to_use": [
                    "This session uses the project's agent secrets as a task's agent does, without seeing them: Yulius' proxy adds a secret to requests for its bound hosts (call them normally, for example curl https://<host>/...), and in code or CLIs the environment variable NAME holds the placeholder yulius-secret-NAME (for example Authorization: Bearer $NAME), which the proxy replaces for the bound hosts only. Several secrets on one host and header: choose with the request header X-Yulius-Secret: NAME. Secrets added after this session started have no variable yet: write the placeholder.",
                    "A secret with no hosts (or not the one you need) is sent nowhere yet: work out the service's API host from its description and documentation and ask for it with secret_request_hosts; a person allows it on the Secrets page (they may be the person in front of you: give them the link). For a secret with auto_allow: true its creator let agents set it up: the hosts you ask for are bound at once, so ask only for the service's own documented hosts.",
                    "Code that runs outside the proxy (a deploy script, ssh, a deployed app) reads its keys from the git-ignored .env of this repository: put a secret there with secret_copy_to_env, never by writing the file yourself (you would write [secret:NAME]). A person allows each secret once (env_files: allowed); until then the call asks them. A multi-line key (stored with literal \\n) is written as its file, line breaks and final line break included.",
                    "To check that a value someone holds (a file, a key they pasted into a file) is the one in the vault, compute its SHA-256 in a shell without printing the value (for example sha256sum key.pem, or printf %s \"$VAR\" | sha256sum) and call secret_verify with the secret's name and that hash. It says whether it matches exactly, only without a final line break, or with its line breaks written as literal \\n (an agent secret never spans lines, so a multi-line key is stored that way), and on a match how many lines the value stands for.",
                    "A secret that is in a file you can reach (a key in ~/.ssh, a key or token you generated without printing it) becomes a new agent secret with secret_create_from_file: Yulius reads the file, so neither you nor anyone sees the value. Never print the file or ask a person to paste it. The new secret has no hosts and no .env allowance until a person allows them.",
                    format!("A person adds, rotates and reveals agent secrets (with their two-factor code) and allows the hosts and .env copies asked for on the Secrets page: {}", self.agent_secrets_link(&sandbox.slug)),
                    "You never receive values: anything that would show one (command output, files, tool results, web pages) shows [secret:NAME] instead. Do not try to print, copy or move secret values yourself.",
                    "states are folders of operator state (a deploy tool's inventory, rollout folders, configs) a person sent from their computer with yulius secrets import --state: a task restores one into the sandbox home with secret_state_restore and saves it back with secret_state_save; restored_path is the copy written to. The person gets the newest saved copy with yulius secrets state-get NAME."
                ],
            }));
        }
        Ok(json!({
            "secrets": secrets,
            "states": states,
            "how_to_use": [
                "A secret with no hosts (or not the one you need) is sent nowhere yet: work out the service's API or sign-in host from its description and the service's documentation, then ask for it with secret_request_hosts; a person allows it. Hosts in requested_hosts already wait for them. For a secret with auto_allow: true its creator let agents set it up: the hosts you ask for are bound at once, so ask only for the service's own documented hosts.",
                "HTTP APIs: call the bound host normally (for example curl https://<host>/...). Yulius' proxy adds the credential to requests for that host only; you never send or see it.",
                "Code and CLIs that need a key: use the environment variable NAME (it holds the placeholder yulius-secret-NAME) or the placeholder itself in the header, Basic credentials or query the service expects (for example Authorization: Bearer $NAME, X-Api-Key: $NAME, curl -u user:$NAME); the proxy replaces it for the bound hosts only. Secrets added after this session started have no variable yet: write the placeholder.",
                "Several secrets on one host and one header: choose with the request header X-Yulius-Secret: NAME.",
                "Website sign-in in the project browser: browser_fill_secret types the value into a field of a page on a bound host; a login's user name is in the description (older secrets may also store a user name or a two-factor seed: field username or totp).",
                "Code that runs outside Yulius' proxy (a deployed app, a deploy script) reads its keys from the project's git-ignored .env, named without values in the tracked .env.example. Put an agent secret there with secret_copy_to_env, never by writing or editing the file: you would write [secret:NAME], not the value. A person allows each secret once (env_files: allowed); until then the call asks them.",
                "A secret that is in a file you can reach (a key in ~/.ssh, a key or token you generated without printing it) becomes a new agent secret with secret_create_from_file: Yulius reads the file, so neither you nor anyone sees the value. Never print the file or ask a person to paste it. The new secret has no hosts and no .env allowance until a person allows them.",
                "You never receive values: anything that would show one (command output, files, tool results, web pages) shows [secret:NAME] instead. Do not try to print, copy or move secret values yourself.",
                "states are folders of operator state (a deploy tool's inventory, rollout folders, configs) the person sent from their computer. Restore one where the project's tools expect it, in your home (~/…), with secret_state_restore; once restored_path is set, that folder is the copy written to and later tasks use it as it is. Never print its files or copy them into the conversation, and call secret_state_save after each change (a rollout) so the vault keeps the newest copy."
            ],
        }))
    }

    /// `browser_fill_secret`: types an agent secret's user name, value or current TOTP code
    /// into the focused (or `selector`) field of a tab whose page is on one of its hosts.
    pub(crate) async fn tool_browser_fill_secret(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: FillArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let field = args.field.as_deref().unwrap_or("value");
        if !matches!(field, "value" | "password" | "username" | "totp") {
            return Err(AppError::Invalid(
                "field is value (or password), username or totp".into(),
            ));
        }
        let record = self
            .resolved_agent_secrets(&ctx.task.sandbox_id, super::Holder::of_task(&ctx.task))
            .await?
            .into_iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
        if matches!(record.scope(), AgentScope::Personal(_)) {
            return Err(AppError::Forbidden(format!(
                "{}: {}",
                record.name,
                super::PERSONAL_SECRET_BROWSER
            )));
        }
        let browser = self.browser_session(&ctx.task.sandbox_id).await?;
        let tab = browser
            .tabs()
            .into_iter()
            .find(|t| t.id == args.tab_id)
            .ok_or_else(|| AppError::Invalid("unknown tab (see browser_tabs)".into()))?;
        let Some((host, port)) = page_host(&tab.url) else {
            return Err(AppError::Invalid(
                "the tab is not showing a web page".into(),
            ));
        };
        if record.hosts.is_empty() {
            return Err(AppError::Forbidden(format!(
                "{} is bound to no host yet: find the service's sign-in host from its description \
                 and ask for it with secret_request_hosts; a person allows it",
                record.name
            )));
        }
        if !bound_to(&record, &host, port) {
            return Err(AppError::Forbidden(format!(
                "{} may only be typed into pages of {}; this tab is on {host}",
                record.name,
                record.hosts.join(", ")
            )));
        }
        let payload: AgentPayload = self.open_payload(&record)?;
        let text = match field {
            "username" => payload
                .username
                .clone()
                .ok_or_else(|| AppError::Invalid(format!("{} has no user name", record.name)))?,
            "totp" => {
                let seed = payload.totp.as_deref().ok_or_else(|| {
                    AppError::Invalid(format!("{} has no two-factor seed", record.name))
                })?;
                self.totp_code(seed)?.code
            }
            _ => payload.value.clone(),
        };
        let text = zeroize::Zeroizing::new(text);
        let params = TypeParams {
            tab_id: args.tab_id.clone(),
            text: text.to_string(),
            selector: args.selector.clone(),
        };
        browser.tool_type(&params).await?;
        browser.agent_activity(
            &args.tab_id,
            None,
            "type",
            &format!("#{} {}", ctx.task.number, ctx.task.harness.display_name()),
        );
        self.note_agent_use(
            (ctx.attempt.id.as_str(), &ctx.actor()),
            &record,
            "vault.agent.fill",
            &format!("{field} on {host}"),
        )
        .await;
        Ok(json!({"filled": true, "secret": record.name, "field": field, "host": host}))
    }

    /// Opens the egress proxy to a CI job of `run` (SPEC-142 CSE-004): a fresh credential whose
    /// client, `ci:<run>`, may use only `secrets` of `sandbox`, until
    /// [`App::ci_egress_close`]. `None` when too many jobs hold one.
    #[must_use]
    pub fn ci_egress_open(
        &self,
        sandbox: &SandboxId,
        run: &str,
        job: &str,
        secrets: &[String],
    ) -> Option<String> {
        let token = crate::auth::hex_token(&self.deps.crypto.random_bytes());
        let digest = self.deps.crypto.sha256_hex(token.as_bytes()).into_bytes();
        let client = EgressClient {
            caller: EgressCaller::Ci {
                run_id: run.to_owned(),
                job_id: job.to_owned(),
                secrets: secrets.to_vec(),
            },
            sandbox_id: sandbox.clone(),
        };
        let mut clients = self.vault.ci_clients.0.lock().ok()?;
        if clients.len() >= CI_CLIENTS_MAX {
            return None;
        }
        clients.push((digest, client));
        Some(token)
    }

    /// Ends a CI job's proxy credential: from now on the proxy refuses it (407).
    pub fn ci_egress_close(&self, token: &str) {
        let digest = self.deps.crypto.sha256_hex(token.as_bytes()).into_bytes();
        if let Ok(mut clients) = self.vault.ci_clients.0.lock() {
            clients.retain(|(d, _)| !self.deps.crypto.constant_time_eq(d, &digest));
        }
    }

    /// The egress client of a CI job's credential, of a task token or of a Superagent's bearer
    /// (the latter two cached briefly; a Superagent's only while its shell lives, SPEC-186
    /// SAV-001); digests are compared in constant time.
    async fn cached_egress_client(&self, token: &str) -> Option<EgressClient> {
        if token.starts_with(crate::shells::COCKPIT_TOKEN_PREFIX) {
            let client = self.cached_token_client(token).await?;
            let EgressCaller::Superagent { shell_id, .. } = &client.caller else {
                return None;
            };
            // An ended Superagent's bearer injects nothing, cached or not.
            return self.shell(shell_id).is_some().then_some(client);
        }
        self.cached_token_client(token).await
    }

    /// [`Self::cached_egress_client`] for a CI credential, a task token or a Superagent bearer.
    async fn cached_token_client(&self, token: &str) -> Option<EgressClient> {
        let digest = self.deps.crypto.sha256_hex(token.as_bytes()).into_bytes();
        if let Ok(clients) = self.vault.ci_clients.0.lock()
            && let Some((_, client)) = clients
                .iter()
                .find(|(d, _)| self.deps.crypto.constant_time_eq(d, &digest))
        {
            return Some(client.clone());
        }
        if let Ok(cache) = self.vault.clients.0.lock()
            && let Some((_, client, _)) = cache.iter().find(|(d, _, at)| {
                at.elapsed() < CLIENT_CACHE_TTL && self.deps.crypto.constant_time_eq(d, &digest)
            })
        {
            return Some(client.clone());
        }
        let client = if token.starts_with(crate::shells::COCKPIT_TOKEN_PREFIX) {
            // A live Superagent whose person may still work in its sandbox (SPEC-186 SAV-001).
            let caller = self.cockpit_caller(token).await.ok()?;
            EgressClient {
                caller: EgressCaller::Superagent {
                    shell_id: caller.detail["shell"].as_str()?.to_owned(),
                    actor: caller.actor,
                    person: caller.user,
                },
                sandbox_id: caller.sandbox,
            }
        } else {
            let ctx = self.agent_context(token).await.ok()?;
            EgressClient {
                caller: EgressCaller::Agent {
                    attempt_id: ctx.attempt.id.as_str().to_owned(),
                    task_id: ctx.task.id.clone(),
                    task_number: ctx.task.number,
                },
                sandbox_id: ctx.task.sandbox_id.clone(),
            }
        };
        if let Ok(mut cache) = self.vault.clients.0.lock() {
            cache.retain(|(_, _, at)| at.elapsed() < CLIENT_CACHE_TTL);
            if cache.len() >= CLIENT_CACHE_MAX {
                cache.clear();
            }
            cache.push((digest, client.clone(), Instant::now()));
        }
        Some(client)
    }
}

#[async_trait]
impl EgressPolicy for App {
    async fn egress_client(&self, token: &str) -> Option<EgressClient> {
        self.cached_egress_client(token).await
    }

    async fn egress_intercepts(&self, client: &EgressClient, host: &str, port: u16) -> bool {
        match self.client_secrets(client).await {
            Ok(records) => records
                .iter()
                .any(|r| client.may_use(&r.name, r.cd_prod) && bound_to(r, host, port)),
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be read for the proxy");
                false
            }
        }
    }

    async fn egress_apply(&self, client: &EgressClient, request: EgressRequest) -> EgressDecision {
        let bound = match self
            .bound_secrets(client, &request.host, request.port)
            .await
        {
            Ok(bound) => bound,
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be opened for the proxy");
                return EgressDecision::Refuse {
                    status: 502,
                    message: "Yulius could not read its agent secrets".into(),
                };
            }
        };
        let resolve = |name: &str| -> Option<String> {
            bound
                .iter()
                .find(|(r, _)| r.name == name)
                .map(|(_, p)| p.value.clone())
        };
        let mut used: Vec<String> = Vec::new();
        let mut chosen: Option<String> = None;
        let mut headers: Vec<(String, Vec<u8>)> = Vec::with_capacity(request.headers.len() + 2);
        for (name, value) in &request.headers {
            match name.as_str() {
                "x-yulius-secret" => {
                    chosen = Some(String::from_utf8_lossy(value).trim().to_owned());
                }
                "proxy-authorization" | "proxy-connection" | "accept-encoding" => {}
                _ => {
                    let value = substitute_header(name, value, &resolve, &mut used);
                    headers.push((name.clone(), value));
                }
            }
        }
        let path_and_query = match substitute_placeholders(&request.path_and_query, resolve) {
            Some(sub) => {
                used.extend(sub.used);
                sub.text
            }
            None => request.path_and_query.clone(),
        };
        if let Some(name) = &chosen
            && !bound.iter().any(|(r, _)| &r.name == name)
        {
            return EgressDecision::Refuse {
                status: 403,
                message: format!(
                    "Yulius: no agent secret named {name:?} is bound to {}",
                    request.host
                ),
            };
        }
        // Secrets that add a header the request lacks, grouped by header.
        let mut wanting: BTreeMap<String, Vec<&(VaultSecretRecord, AgentPayload)>> =
            BTreeMap::new();
        for entry in &bound {
            let (record, _) = entry;
            if chosen.as_ref().is_some_and(|c| c != &record.name) {
                continue;
            }
            if let Some(header) = injection_of(record).header_name()
                && !headers.iter().any(|(n, _)| n == &header)
            {
                wanting.entry(header).or_default().push(entry);
            }
        }
        for (header, entries) in wanting {
            if entries.len() > 1 {
                let names: Vec<&str> = entries.iter().map(|(r, _)| r.name.as_str()).collect();
                return EgressDecision::Refuse {
                    status: 409,
                    message: format!(
                        "Yulius: several agent secrets add {header} for {}: {}. Choose one with the request header X-Yulius-Secret: <NAME>.",
                        request.host,
                        names.join(", ")
                    ),
                };
            }
            let (record, payload) = entries[0];
            if let Some((name, value)) =
                injection_of(record).render(payload.username.as_deref(), &payload.value)
            {
                headers.push((name, value.into_bytes()));
                used.push(record.name.clone());
            }
        }
        headers.push(("accept-encoding".into(), b"identity".to_vec()));
        used.sort();
        used.dedup();
        for name in &used {
            if let Some((record, _)) = bound.iter().find(|(r, _)| &r.name == name) {
                self.note_agent_use(
                    (client.use_key(), &client.actor()),
                    record,
                    "vault.agent.use",
                    &request.host,
                )
                .await;
            }
        }
        EgressDecision::Forward {
            path_and_query,
            headers,
            used,
        }
    }

    async fn egress_redactor(&self, client: &EgressClient) -> Arc<PlainMatcher> {
        self.agent_secret_matcher(&client.sandbox_id)
            .await
            .unwrap_or_else(|err| {
                tracing::error!(error = %err, "agent secrets could not be loaded for redaction");
                Arc::new(PlainMatcher::default())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_hosts_and_authorities() {
        assert_eq!(
            page_host("https://Mail.Google.com/mail/u/0"),
            Some(("mail.google.com".into(), 443))
        );
        assert_eq!(
            page_host("http://a.example.com:8080/x?y#z"),
            Some(("a.example.com".into(), 8080))
        );
        assert_eq!(page_host("about:blank"), None);
        assert_eq!(page_host("chrome://newtab"), None);
        assert_eq!(authority("http://127.0.0.1:8081"), Some("127.0.0.1:8081"));
        assert_eq!(authority("http://127.0.0.1:8081/x"), Some("127.0.0.1:8081"));
    }

    /// SPEC-186 SAV-001: a contained Superagent reads its CA bundle, written to its shell's run
    /// directory, through a read-only bind.
    #[test]
    fn run_files_are_bound_read_only_into_a_contained_session() {
        let file = PlannedFile {
            path: "/data/shells/abc/egress-ca.pem".into(),
            contents: Vec::new(),
            mode: 0o600,
        };
        let mut contained = Containment::Bubblewrap {
            sandbox_root: "/data/sandboxes/s1".into(),
            home: "/home/alice".into(),
            writable: Vec::new(),
            readonly: vec!["/usr/bin/yulius".into()],
            binds: Vec::new(),
            identity: None,
            join: None,
        };
        bind_read_only(&mut contained, std::slice::from_ref(&file));
        let Containment::Bubblewrap { readonly, .. } = &contained else {
            unreachable!("still contained");
        };
        assert_eq!(
            readonly,
            &[
                std::path::PathBuf::from("/usr/bin/yulius"),
                file.path.clone()
            ]
        );
        let mut plain = Containment::None;
        bind_read_only(&mut plain, &[file]);
        assert_eq!(plain, Containment::None);
    }

    #[test]
    fn basic_credentials_are_substituted_inside() {
        let resolve = |name: &str| (name == "PASS").then(|| "s3cret-value".to_owned());
        let mut used = Vec::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode("bob:yulius-secret-PASS");
        let out = substitute_header(
            "authorization",
            format!("Basic {encoded}").as_bytes(),
            &resolve,
            &mut used,
        );
        let expected = base64::engine::general_purpose::STANDARD.encode("bob:s3cret-value");
        assert_eq!(out, format!("Basic {expected}").into_bytes());
        assert_eq!(used, vec!["PASS".to_owned()]);
        let untouched = substitute_header("x-other", b"plain", &resolve, &mut used);
        assert_eq!(untouched, b"plain".to_vec());
    }
}
