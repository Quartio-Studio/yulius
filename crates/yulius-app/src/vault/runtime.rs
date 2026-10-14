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
use yulius_domain::ids::SandboxId;
use yulius_domain::vault::hosts::HostPattern;
use yulius_domain::vault::redact::{Matcher, PlainMatcher, StreamRedactor, rolling_hash};
use yulius_domain::vault::{
    Injection, PLACEHOLDER_PREFIX, SecretName, placeholder, substitute_placeholders,
};

use super::AgentPayload;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::egress::{EgressClient, EgressDecision, EgressPolicy, EgressRequest};
use crate::ports::session::{LaunchPlan, PlannedFile};
use crate::ports::vault::VaultSecretRecord;

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
    /// references, VLT-009).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub(crate) async fn agent_secret_value(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<Option<zeroize::Zeroizing<String>>> {
        let Some(record) = self
            .deps
            .store
            .vault_agent(Some(sandbox))
            .await?
            .into_iter()
            .find(|r| r.name == name)
        else {
            return Ok(None);
        };
        let payload: AgentPayload = self.open_payload(&record)?;
        Ok(Some(zeroize::Zeroizing::new(payload.value.clone())))
    }

    /// The agent secrets bound to `host:port` for a sandbox, with their payloads.
    async fn bound_secrets(
        &self,
        sandbox: &SandboxId,
        host: &str,
        port: u16,
    ) -> AppResult<Vec<(VaultSecretRecord, AgentPayload)>> {
        let mut out = Vec::new();
        for record in self.deps.store.vault_agent(Some(sandbox)).await? {
            if bound_to(&record, host, port) {
                let payload: AgentPayload = self.open_payload(&record)?;
                out.push((record, payload));
            }
        }
        Ok(out)
    }

    /// Records an agent's use of a secret: `last_used_at`, and one audit entry per run, secret
    /// and context.
    async fn note_agent_use(
        &self,
        attempt: &str,
        task_number: u32,
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
            format!("{action}|{attempt}|{}|{context}", record.id),
            &format!("task:{task_number}"),
            action,
            Some(&record.id),
            &format!("{} → {context}", record.name),
        )
        .await;
    }

    /// The fingerprints of every agent secret that applies to `sandbox`, with a fresh salt and
    /// rolling-hash base.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn scrub_fingerprints_for(&self, sandbox: &SandboxId) -> AppResult<FingerprintSet> {
        let matcher = self.agent_secret_matcher(sandbox).await?;
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

    /// `POST /internal/scrub/fingerprints`: the caller's run's fingerprints.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown or ended runs; store failures.
    pub async fn scrub_fingerprints(&self, token: &str) -> AppResult<FingerprintSet> {
        let ctx = self.agent_context(token).await?;
        self.scrub_fingerprints_for(&ctx.task.sandbox_id).await
    }

    /// `POST /internal/scrub/report`: a sandbox process redacted these secrets (audited once
    /// per run, secret and channel).
    ///
    /// # Errors
    /// `Unauthenticated`, `Invalid` channel.
    pub async fn scrub_report(&self, token: &str, report: &RedactionReport) -> AppResult<()> {
        let ctx = self.agent_context(token).await?;
        if !matches!(report.channel.as_str(), "bash" | "tool") {
            return Err(AppError::Invalid("channel must be bash or tool".into()));
        }
        let records = self
            .deps
            .store
            .vault_agent(Some(&ctx.task.sandbox_id))
            .await?;
        for name in report.names.iter().take(64) {
            if let Some(record) = records.iter().find(|r| &r.name == name) {
                self.vault_audit_once(
                    format!(
                        "redacted|{}|{}|{}",
                        ctx.attempt.id.as_str(),
                        record.id,
                        report.channel
                    ),
                    &ctx.actor(),
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
    /// every agent secret that applies, the Bash wrapper (`CLAUDE_CODE_SHELL_PREFIX`) and its
    /// fingerprint file, and — when the egress proxy is wired — the proxy variables (with the
    /// run's task token, as secret environment) and the CA bundle.
    ///
    /// # Errors
    /// Store or decryption failures.
    pub(crate) async fn vault_run_setup(
        &self,
        sandbox: &SandboxRecord,
        run_dir: &Path,
        task_token: &str,
        launch: &mut LaunchPlan,
    ) -> AppResult<()> {
        let records = self.deps.store.vault_agent(Some(&sandbox.id)).await?;
        for record in &records {
            if let Ok(name) = SecretName::parse(&record.name) {
                launch
                    .env
                    .push((name.as_str().to_owned(), placeholder(&name)));
            }
        }
        let fingerprints = self.scrub_fingerprints_for(&sandbox.id).await?;
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
        // Claude Code runs `'<prefix before the last " -">' <flag> '<command>'` for Bash
        // commands, hooks and stdio MCP servers.
        let exe = self.config.yulius_exe.to_string_lossy();
        if exe.contains(" -") {
            tracing::warn!(
                "the yulius path contains \" -\": Bash output is redacted by the tool hook only"
            );
        } else {
            launch.env.push((
                "CLAUDE_CODE_SHELL_PREFIX".into(),
                format!("{exe} {SCRUB_EXEC_FLAG}"),
            ));
        }
        if !self.vault.egress.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let Some(proxy_authority) = authority(&self.config.internal_url) else {
            return Ok(());
        };
        let proxy = format!("http://yulius:{task_token}@{proxy_authority}");
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
        Ok(())
    }

    /// A streaming redactor of the task terminal for `sandbox` (`None` without agent secrets).
    pub(crate) async fn terminal_redactor(
        &self,
        sandbox: &SandboxId,
    ) -> Option<StreamRedactor<Arc<PlainMatcher>>> {
        match self.agent_secret_matcher(sandbox).await {
            Ok(matcher) if !matcher.is_empty() => Some(StreamRedactor::new(matcher, 4)),
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

    /// `secrets_list`: names, descriptions and bindings of the agent secrets this task may use,
    /// and how to use them; never a value.
    pub(crate) async fn tool_secrets_list(&self, ctx: &AgentContext) -> AppResult<Value> {
        let records = self
            .deps
            .store
            .vault_agent(Some(&ctx.task.sandbox_id))
            .await?;
        let secrets: Vec<Value> = records
            .iter()
            .map(|r| {
                json!({
                    "name": r.name,
                    "description": r.description,
                    "scope": if r.sandbox_id.is_some() { "this project" } else { "every project" },
                    "hosts": r.hosts,
                    "requested_hosts": r.requested_hosts,
                    "adds": injection_of(r).describe(),
                    "placeholder": format!("{PLACEHOLDER_PREFIX}{}", r.name),
                    "fields": r.fields,
                })
            })
            .collect();
        Ok(json!({
            "secrets": secrets,
            "how_to_use": [
                "A secret with no hosts (or not the one you need) is sent nowhere yet: work out the service's API or sign-in host from its description and the service's documentation, then ask for it with secret_request_hosts; a person allows it. Hosts in requested_hosts already wait for them.",
                "HTTP APIs: call the bound host normally (for example curl https://<host>/...). Yulius' proxy adds the credential to requests for that host only; you never send or see it.",
                "Code and CLIs that need a key: use the environment variable NAME (it holds the placeholder yulius-secret-NAME) or the placeholder itself in the header, Basic credentials or query the service expects (for example Authorization: Bearer $NAME, X-Api-Key: $NAME, curl -u user:$NAME); the proxy replaces it for the bound hosts only. Secrets added after this session started have no variable yet: write the placeholder.",
                "Several secrets on one host and one header: choose with the request header X-Yulius-Secret: NAME.",
                "Website sign-in in the project browser: browser_fill_secret types the value into a field of a page on a bound host; a login's user name is in the description (older secrets may also store a user name or a two-factor seed: field username or totp).",
                "You never receive values: anything that would show one (command output, files, tool results, web pages) shows [secret:NAME] instead. Do not try to print, copy or move secret values."
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
            .deps
            .store
            .vault_agent(Some(&ctx.task.sandbox_id))
            .await?
            .into_iter()
            .find(|r| r.name == args.secret.trim())
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {:?} applies to this project (see secrets_list)",
                    args.secret
                ))
            })?;
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
            ctx.attempt.id.as_str(),
            ctx.task.number,
            &record,
            "vault.agent.fill",
            &format!("{field} on {host}"),
        )
        .await;
        Ok(json!({"filled": true, "secret": record.name, "field": field, "host": host}))
    }

    /// The egress client of a task token, cached briefly (compared in constant time).
    async fn cached_egress_client(&self, token: &str) -> Option<EgressClient> {
        let digest = self.deps.crypto.sha256_hex(token.as_bytes()).into_bytes();
        if let Ok(cache) = self.vault.clients.0.lock()
            && let Some((_, client, _)) = cache.iter().find(|(d, _, at)| {
                at.elapsed() < CLIENT_CACHE_TTL && self.deps.crypto.constant_time_eq(d, &digest)
            })
        {
            return Some(client.clone());
        }
        let ctx = self.agent_context(token).await.ok()?;
        let client = EgressClient {
            attempt_id: ctx.attempt.id.as_str().to_owned(),
            task_id: ctx.task.id.clone(),
            task_number: ctx.task.number,
            sandbox_id: ctx.task.sandbox_id.clone(),
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
        match self.deps.store.vault_agent(Some(&client.sandbox_id)).await {
            Ok(records) => records.iter().any(|r| bound_to(r, host, port)),
            Err(err) => {
                tracing::error!(error = %err, "agent secrets could not be read for the proxy");
                false
            }
        }
    }

    async fn egress_apply(&self, client: &EgressClient, request: EgressRequest) -> EgressDecision {
        let bound = match self
            .bound_secrets(&client.sandbox_id, &request.host, request.port)
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
                    &client.attempt_id,
                    client.task_number,
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
