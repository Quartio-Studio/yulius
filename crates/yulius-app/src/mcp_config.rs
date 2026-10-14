//! MCP servers attached to agent runs (SPEC-001 MCP-001..003, AC-MCP-1/2).
//!
//! Servers are configured once per instance (instance administrators) or per sandbox (write
//! access) and attached to every Claude Code run of the sandbox; the Yulius tool server
//! itself is always attached by the harness adapter, so the name `yulius` is reserved. Secrets
//! (stdio environment, HTTP headers) are sealed with the instance key, never returned (only
//! their names), and opened only to build a run's attachments. Names are unique per scope
//! case-insensitively with `-` and `_` treated alike (they become environment variable names);
//! a sandbox server overrides an instance server of the same name, also when disabled, which
//! turns the instance server off for that sandbox.
//!
//! The attachments are built for every run, whichever subscription it runs on (MCP-004): the
//! harness port writes them into the run's own MCP configuration, never into a per-account
//! Claude Code home. A secret value may instead be a reference, `${secret:NAME}` (for a header,
//! also `Bearer ${secret:NAME}`): it is resolved when a run starts through
//! [`App::resolve_secret_ref`], the hook of the agent-secrets vault, and left out of the run
//! while nothing answers to it.

pub mod probe;
pub mod project;

use std::collections::BTreeMap;
use std::path::PathBuf;

use yulius_contracts::api::mcp::{McpServerInput, McpServerView, McpTestResult};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Sealed};
use crate::ports::harness::McpAttachment;

/// Reserved server name (the Yulius tool server).
pub const RESERVED_NAME: &str = "yulius";
/// Most servers per scope (the instance, or one sandbox).
pub const MAX_SERVERS_PER_SCOPE: u32 = 50;
/// Longest server name.
pub const MAX_NAME_LEN: usize = 64;
/// Most arguments of a stdio server.
pub const MAX_ARGS: usize = 64;
/// Longest argument or command.
pub const MAX_ARG_BYTES: usize = 4 * 1024;
/// Longest URL.
pub const MAX_URL_BYTES: usize = 2 * 1024;
/// Most secrets per map.
pub const MAX_SECRETS: usize = 64;
/// Longest secret name.
pub const MAX_SECRET_NAME: usize = 128;
/// Largest secret value.
pub const MAX_SECRET_BYTES: usize = 16 * 1024;
/// Longest name in a secret reference (`${secret:NAME}`).
pub const MAX_SECRET_REF_NAME: usize = 128;

/// The `NAME` of a value that is exactly a secret reference, `${secret:NAME}` (NAME: 1–128
/// ASCII letters, digits, `_`, `-` or `.`).
#[must_use]
pub fn secret_ref(value: &str) -> Option<&str> {
    let name = value.strip_prefix("${secret:")?.strip_suffix('}')?;
    let valid = !name.is_empty()
        && name.len() <= MAX_SECRET_REF_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    valid.then_some(name)
}

/// Transport of a configured server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McpTransport {
    /// Child process speaking JSON-RPC on stdin/stdout.
    Stdio,
    /// Streamable HTTP endpoint.
    Http,
}

impl McpTransport {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "stdio" => Some(Self::Stdio),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

/// A stored MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRecord {
    /// Opaque id.
    pub id: String,
    /// Owning sandbox (`None` = instance scope).
    pub sandbox_id: Option<SandboxId>,
    /// Name (tool namespace in the harnesses).
    pub name: String,
    /// Transport.
    pub transport: McpTransport,
    /// Executable (stdio).
    pub command: Option<String>,
    /// Arguments (stdio).
    pub args: Vec<String>,
    /// URL (HTTP).
    pub url: Option<String>,
    /// Sealed JSON object of environment secrets.
    pub env: Option<Sealed>,
    /// Sealed JSON object of header secrets.
    pub headers: Option<Sealed>,
    /// Enabled.
    pub enabled: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

impl McpServerRecord {
    /// `instance` or `sandbox`.
    #[must_use]
    pub fn scope(&self) -> &'static str {
        if self.sandbox_id.is_some() {
            "sandbox"
        } else {
            "instance"
        }
    }
}

/// Uniqueness key of a server name: lowercase with `-` folded to `_`.
#[must_use]
pub fn name_key(name: &str) -> String {
    name.to_ascii_lowercase().replace('-', "_")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_SECRET_NAME
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn check_secrets(
    map: &BTreeMap<String, String>,
    what: &str,
    valid: fn(&str) -> bool,
) -> AppResult<()> {
    if map.len() > MAX_SECRETS {
        return Err(AppError::Invalid(format!("at most {MAX_SECRETS} {what}")));
    }
    for (key, value) in map {
        if !valid(key) {
            return Err(AppError::Invalid(format!("invalid {what} name `{key}`")));
        }
        if value.len() > MAX_SECRET_BYTES || value.contains(['\0', '\r', '\n']) || value.is_empty()
        {
            return Err(AppError::Invalid(format!(
                "the value of {key} must be 1 byte to 16 KiB on one line"
            )));
        }
    }
    Ok(())
}

/// Validated non-secret fields.
struct Fields {
    name: String,
    transport: McpTransport,
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
}

fn validate(input: &McpServerInput) -> AppResult<Fields> {
    let name = input.name.trim();
    if !valid_name(name) {
        return Err(AppError::Invalid(format!(
            "the name must be 1 to {MAX_NAME_LEN} letters, digits, - or _"
        )));
    }
    if name_key(name) == RESERVED_NAME {
        return Err(AppError::Invalid(
            "the name yulius is reserved for the Yulius tool server, which is always attached"
                .into(),
        ));
    }
    let transport = McpTransport::parse(&input.transport)
        .ok_or_else(|| AppError::Invalid("transport must be stdio or http".into()))?;
    if let Some(env) = &input.env {
        check_secrets(env, "environment variables", valid_env_name)?;
    }
    if let Some(headers) = &input.headers {
        check_secrets(headers, "headers", valid_header_name)?;
    }
    match transport {
        McpTransport::Stdio => {
            let command = input.command.as_deref().map(str::trim).unwrap_or("");
            if command.is_empty() || command.len() > MAX_ARG_BYTES || command.contains('\0') {
                return Err(AppError::Invalid(
                    "a stdio server needs a command (an executable name or path)".into(),
                ));
            }
            if input.args.len() > MAX_ARGS
                || input
                    .args
                    .iter()
                    .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
            {
                return Err(AppError::Invalid(format!(
                    "at most {MAX_ARGS} arguments of up to 4 KiB each"
                )));
            }
            Ok(Fields {
                name: name.to_owned(),
                transport,
                command: Some(command.to_owned()),
                args: input.args.clone(),
                url: None,
            })
        }
        McpTransport::Http => {
            let url = input.url.as_deref().map(str::trim).unwrap_or("");
            let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
            if !scheme_ok
                || url.len() > MAX_URL_BYTES
                || url.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(AppError::Invalid(
                    "an http server needs an http:// or https:// URL".into(),
                ));
            }
            Ok(Fields {
                name: name.to_owned(),
                transport,
                command: None,
                args: Vec::new(),
                url: Some(url.to_owned()),
            })
        }
    }
}

/// Splits an `Authorization: Bearer <token>` header from other headers.
fn bearer_of(headers: &BTreeMap<String, String>) -> (Option<String>, Vec<String>) {
    let mut bearer = None;
    let mut ignored = Vec::new();
    for (name, value) in headers {
        let token = name
            .eq_ignore_ascii_case("authorization")
            .then(|| {
                value
                    .split_once(' ')
                    .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
                    .map(|(_, token)| token.trim().to_owned())
            })
            .flatten();
        match token {
            Some(token) if !token.is_empty() => bearer = Some(token),
            _ => ignored.push(name.clone()),
        }
    }
    (bearer, ignored)
}

impl App {
    /// The value of the secret `name` for runs in `sandbox`, for `${secret:NAME}` references in
    /// MCP configuration (MCP-003, VLT-009): the agent secret of that NAME that applies to the
    /// sandbox (its own or an instance-wide one). An unknown name does not resolve, and the
    /// referencing variable or header is left out of the run (with a warning), never passed on
    /// as the literal reference. The value reaches the run's environment for the MCP client;
    /// every channel the agent reads redacts it (SPEC-002 ADR-20).
    ///
    /// # Errors
    /// Store or decryption failures.
    pub async fn resolve_secret_ref(
        &self,
        sandbox: &SandboxRecord,
        name: &str,
    ) -> AppResult<Option<String>> {
        Ok(self
            .agent_secret_value(&sandbox.id, name)
            .await?
            .map(|value| value.to_string()))
    }

    /// A stored secret value for a run: itself, or what its reference resolves to.
    async fn secret_value(
        &self,
        sandbox: &SandboxRecord,
        server: &str,
        key: &str,
        value: String,
    ) -> AppResult<Option<String>> {
        let Some(name) = secret_ref(&value) else {
            return Ok(Some(value));
        };
        let resolved = self.resolve_secret_ref(sandbox, name).await?;
        if resolved.is_none() {
            tracing::warn!(server = %server, key = %key, secret = %name, "an MCP secret reference does not resolve; the run gets the server without it");
        }
        Ok(resolved)
    }

    fn seal_secrets(&self, map: &BTreeMap<String, String>) -> AppResult<Option<Sealed>> {
        if map.is_empty() {
            return Ok(None);
        }
        let json = serde_json::to_vec(map).map_err(AppError::internal)?;
        self.deps.crypto.seal(&json).map(Some)
    }

    fn open_secrets(&self, sealed: Option<&Sealed>) -> AppResult<BTreeMap<String, String>> {
        match sealed {
            None => Ok(BTreeMap::new()),
            Some(sealed) => {
                let bytes = self.deps.crypto.open(sealed)?;
                serde_json::from_slice(&bytes).map_err(AppError::internal)
            }
        }
    }

    fn mcp_view(&self, record: &McpServerRecord) -> AppResult<McpServerView> {
        Ok(McpServerView {
            id: record.id.clone(),
            scope: record.scope().to_owned(),
            name: record.name.clone(),
            transport: record.transport.as_str().to_owned(),
            command: record.command.clone(),
            args: record.args.clone(),
            url: record.url.clone(),
            env_keys: self
                .open_secrets(record.env.as_ref())?
                .into_keys()
                .collect(),
            header_keys: self
                .open_secrets(record.headers.as_ref())?
                .into_keys()
                .collect(),
            enabled: record.enabled,
        })
    }

    /// `GET /api/sandboxes/{sid}/mcp`: sandbox servers plus inherited instance servers.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_mcp_servers(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<McpServerView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        self.deps
            .store
            .mcp_servers(&sid)
            .await?
            .iter()
            .map(|s| self.mcp_view(s))
            .collect()
    }

    /// `POST /api/sandboxes/{sid}/mcp`; `scope: "instance"` creates an instance server
    /// (instance administrators only).
    ///
    /// # Errors
    /// Access, validation, duplicate-name and capacity errors.
    pub async fn create_mcp_server(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        input: &McpServerInput,
    ) -> AppResult<McpServerView> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let instance = match input.scope.as_deref().unwrap_or("sandbox") {
            "instance" => true,
            "sandbox" | "" => false,
            _ => {
                return Err(AppError::Invalid(
                    "scope must be instance or sandbox".into(),
                ));
            }
        };
        if instance {
            self.require(&ctx.user, &sid, Action::View).await?;
            self.require_instance_admin(ctx)?;
        } else {
            self.require(&ctx.user, &sid, Action::Configure).await?;
        }
        let record = self.new_mcp_record((!instance).then_some(sid), input)?;
        self.deps
            .store
            .insert_mcp_server(&record, MAX_SERVERS_PER_SCOPE)
            .await
            .map_err(|err| match err {
                AppError::Conflict(_) => AppError::Conflict(format!(
                    "an MCP server named {} already exists in this scope",
                    record.name
                )),
                other => other,
            })?;
        self.audit_mcp(ctx, "mcp.created", &record).await?;
        self.mcp_view(&record)
    }

    /// A new server of `sandbox_id`'s scope (`None`: the instance) from validated `input`, its
    /// secrets sealed.
    fn new_mcp_record(
        &self,
        sandbox_id: Option<SandboxId>,
        input: &McpServerInput,
    ) -> AppResult<McpServerRecord> {
        let fields = validate(input)?;
        Ok(McpServerRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id,
            name: fields.name,
            transport: fields.transport,
            command: fields.command,
            args: fields.args,
            url: fields.url,
            env: self.seal_secrets(&input.env.clone().unwrap_or_default())?,
            headers: self.seal_secrets(&input.headers.clone().unwrap_or_default())?,
            enabled: input.enabled.unwrap_or(true),
            created_at: self.now(),
        })
    }

    async fn mcp_for_change(&self, ctx: &AuthContext, id: &str) -> AppResult<McpServerRecord> {
        let record = self
            .deps
            .store
            .mcp_server(id)
            .await?
            .ok_or(AppError::NotFound)?;
        match &record.sandbox_id {
            Some(sid) => {
                self.require(&ctx.user, sid, Action::Configure).await?;
            }
            None => self.require_instance_admin(ctx)?,
        }
        Ok(record)
    }

    /// `PUT /api/mcp/{id}`. Omitted `env`/`headers` keep the stored secrets; a given map
    /// replaces all values of that map (an empty map clears it). The scope cannot change.
    ///
    /// # Errors
    /// Access, validation and duplicate-name errors.
    pub async fn update_mcp_server(
        &self,
        ctx: &AuthContext,
        id: &str,
        input: &McpServerInput,
    ) -> AppResult<McpServerView> {
        let current = self.mcp_for_change(ctx, id).await?;
        let fields = validate(input)?;
        let env = match &input.env {
            Some(map) => self.seal_secrets(map)?,
            None => current.env.clone(),
        };
        let headers = match &input.headers {
            Some(map) => self.seal_secrets(map)?,
            None => current.headers.clone(),
        };
        let record = McpServerRecord {
            name: fields.name,
            transport: fields.transport,
            command: fields.command,
            args: fields.args,
            url: fields.url,
            env,
            headers,
            enabled: input.enabled.unwrap_or(current.enabled),
            ..current
        };
        let updated =
            self.deps
                .store
                .update_mcp_server(&record)
                .await
                .map_err(|err| match err {
                    AppError::Conflict(_) => AppError::Conflict(format!(
                        "an MCP server named {} already exists in this scope",
                        record.name
                    )),
                    other => other,
                })?;
        if !updated {
            return Err(AppError::NotFound);
        }
        self.audit_mcp(ctx, "mcp.updated", &record).await?;
        self.mcp_view(&record)
    }

    /// `DELETE /api/mcp/{id}` (stored secrets are deleted with the row).
    ///
    /// # Errors
    /// Access errors.
    pub async fn delete_mcp_server(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let record = self.mcp_for_change(ctx, id).await?;
        if !self.deps.store.delete_mcp_server(&record.id).await? {
            return Err(AppError::NotFound);
        }
        self.audit_mcp(ctx, "mcp.deleted", &record).await
    }

    /// `POST /api/mcp/{id}/test`: connects, initializes and lists tools (bounded: 5 s for
    /// HTTP, 10 s for stdio, after which the process group is killed and reaped). A stdio test
    /// runs the command on the Yulius host outside any sandbox containment, so on a contained
    /// instance it needs an instance administrator.
    ///
    /// # Errors
    /// Access errors (connection failures are reported in the result, not as errors).
    pub async fn test_mcp_server(&self, ctx: &AuthContext, id: &str) -> AppResult<McpTestResult> {
        let record = self.mcp_for_change(ctx, id).await?;
        let result = match record.transport {
            McpTransport::Http => {
                let headers: Vec<(String, String)> = self
                    .open_secrets(record.headers.as_ref())?
                    .into_iter()
                    .collect();
                let url = record.url.as_deref().unwrap_or("");
                match probe::resolve_target(url).await {
                    Err(reason) => Err(reason),
                    Ok((host, addrs)) => {
                        let class = addrs
                            .iter()
                            .map(|a| probe::classify(a.ip()))
                            .max()
                            .unwrap_or(probe::AddressClass::Forbidden);
                        match class {
                            probe::AddressClass::Forbidden => Err(
                                "link-local, metadata and broadcast addresses cannot be tested"
                                    .to_owned(),
                            ),
                            probe::AddressClass::Private => {
                                self.require_instance_admin(ctx)?;
                                probe::probe_http(url, &headers, Some((&host, &addrs))).await
                            }
                            probe::AddressClass::Public => {
                                probe::probe_http(url, &headers, Some((&host, &addrs))).await
                            }
                        }
                    }
                }
            }
            McpTransport::Stdio => {
                if self.config.contained {
                    self.require_instance_admin(ctx)?;
                }
                let env: Vec<(String, String)> = self
                    .open_secrets(record.env.as_ref())?
                    .into_iter()
                    .collect();
                let home = self.mcp_test_home()?;
                probe::probe_stdio(
                    &probe::StdioSpec {
                        command: record.command.as_deref().unwrap_or(""),
                        args: &record.args,
                        env: &env,
                        home: &home,
                    },
                    probe::STDIO_TIMEOUT,
                )
                .await
            }
        };
        self.audit_mcp(
            ctx,
            if result.is_ok() {
                "mcp.test_ok"
            } else {
                "mcp.test_failed"
            },
            &record,
        )
        .await?;
        Ok(match result {
            Ok(tools) => McpTestResult {
                ok: true,
                tools,
                error: None,
            },
            Err(error) => McpTestResult {
                ok: false,
                tools: Vec::new(),
                error: Some(error),
            },
        })
    }

    fn mcp_test_home(&self) -> AppResult<PathBuf> {
        let home = self.config.data_dir.join("mcp-test");
        std::fs::create_dir_all(&home).map_err(AppError::internal)?;
        Ok(home)
    }

    async fn audit_mcp(
        &self,
        ctx: &AuthContext,
        action: &str,
        record: &McpServerRecord,
    ) -> AppResult<()> {
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                action,
                Some(&record.id),
                Some(&project::mcp_detail(record)),
            )
            .await
    }

    /// Configured MCP servers for runs in `sandbox`: enabled instance and sandbox servers with
    /// opened secrets, one per name (a sandbox server overrides an instance server of the same
    /// name). Every run gets them, whichever subscription it runs on (MCP-004). HTTP servers
    /// forward an `Authorization: Bearer` header as the bearer token; other headers cannot be
    /// passed to the harnesses yet and are skipped with a warning. `${secret:NAME}` values are
    /// resolved through [`App::resolve_secret_ref`]; one that does not resolve is left out.
    ///
    /// # Errors
    /// Store, crypto or secret-resolution failures.
    pub async fn mcp_attachments(&self, sandbox: &SandboxRecord) -> AppResult<Vec<McpAttachment>> {
        let servers = self.deps.store.mcp_servers(&sandbox.id).await?;
        let mut effective: BTreeMap<String, &McpServerRecord> = BTreeMap::new();
        for server in servers.iter().filter(|s| s.sandbox_id.is_none()) {
            effective.insert(name_key(&server.name), server);
        }
        for server in servers.iter().filter(|s| s.sandbox_id.is_some()) {
            effective.insert(name_key(&server.name), server);
        }
        let mut out = Vec::new();
        for (key, server) in effective {
            if !server.enabled || key == RESERVED_NAME {
                continue;
            }
            match server.transport {
                McpTransport::Stdio => {
                    let mut env = Vec::new();
                    for (key, value) in self.open_secrets(server.env.as_ref())? {
                        if let Some(value) = self
                            .secret_value(sandbox, &server.name, &key, value)
                            .await?
                        {
                            env.push((key, value));
                        }
                    }
                    out.push(McpAttachment::Stdio {
                        name: server.name.clone(),
                        command: server.command.clone().unwrap_or_default(),
                        args: server.args.clone(),
                        env,
                    });
                }
                McpTransport::Http => {
                    let (bearer, ignored) = bearer_of(&self.open_secrets(server.headers.as_ref())?);
                    let bearer = match bearer {
                        Some(token) => {
                            self.secret_value(sandbox, &server.name, "Authorization", token)
                                .await?
                        }
                        None => None,
                    };
                    if !ignored.is_empty() {
                        tracing::warn!(server = %server.name, headers = ?ignored, "MCP headers other than Authorization: Bearer are not passed to agent runs");
                    }
                    out.push(McpAttachment::Http {
                        name: server.name.clone(),
                        url: server.url.clone().unwrap_or_default(),
                        bearer,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, transport: &str) -> McpServerInput {
        McpServerInput {
            name: name.to_owned(),
            transport: transport.to_owned(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "@apify/actors-mcp-server".into()],
            url: Some("https://mcp.example.com/mcp".into()),
            env: None,
            headers: None,
            enabled: None,
            scope: None,
        }
    }

    #[test]
    fn names_and_reserved() {
        assert!(validate(&input("quartio", "http")).is_ok());
        assert!(validate(&input("Yulius", "http")).is_err());
        assert!(validate(&input("bad name", "http")).is_err());
        assert!(validate(&input(&"a".repeat(MAX_NAME_LEN + 1), "http")).is_err());
        assert!(validate(&input("x", "ws")).is_err());
        assert_eq!(name_key("My-Server"), name_key("my_server"));
    }

    #[test]
    fn transport_specific_fields() {
        let stdio = validate(&input("apify", "stdio"));
        assert!(matches!(&stdio, Ok(f) if f.url.is_none() && f.command.as_deref() == Some("npx")));
        let http = validate(&input("quartio", "http"));
        assert!(matches!(&http, Ok(f) if f.command.is_none() && f.args.is_empty()));
        let mut no_url = input("q", "http");
        no_url.url = Some("ftp://x".into());
        assert!(validate(&no_url).is_err());
        let mut no_cmd = input("q", "stdio");
        no_cmd.command = Some("  ".into());
        assert!(validate(&no_cmd).is_err());
    }

    #[test]
    fn secret_names_and_values() {
        let mut bad_env = input("a", "stdio");
        bad_env.env = Some(BTreeMap::from([("1BAD".to_owned(), "v".to_owned())]));
        assert!(validate(&bad_env).is_err());
        let mut newline = input("a", "stdio");
        newline.env = Some(BTreeMap::from([("TOKEN".to_owned(), "a\nb".to_owned())]));
        assert!(validate(&newline).is_err());
        let mut ok = input("a", "http");
        ok.headers = Some(BTreeMap::from([("X-Api-Key".to_owned(), "k".to_owned())]));
        assert!(validate(&ok).is_ok());
    }

    #[test]
    fn secret_references() {
        assert_eq!(secret_ref("${secret:apify}"), Some("apify"));
        assert_eq!(
            secret_ref("${secret:team.APIFY_TOKEN-2}"),
            Some("team.APIFY_TOKEN-2")
        );
        for literal in [
            "apify",
            "${secret:}",
            "${secret:a b}",
            "${APIFY}",
            "x${secret:a}",
            "${secret:a}x",
        ] {
            assert_eq!(secret_ref(literal), None, "{literal}");
        }
        let long = format!("${{secret:{}}}", "a".repeat(MAX_SECRET_REF_NAME + 1));
        assert_eq!(secret_ref(&long), None);
        let headers = BTreeMap::from([(
            "Authorization".to_owned(),
            "Bearer ${secret:quartio}".to_owned(),
        )]);
        assert_eq!(bearer_of(&headers).0.as_deref(), Some("${secret:quartio}"));
    }

    #[test]
    fn bearer_extraction() {
        let headers = BTreeMap::from([
            ("authorization".to_owned(), "Bearer abc".to_owned()),
            ("X-Extra".to_owned(), "1".to_owned()),
        ]);
        let (bearer, ignored) = bearer_of(&headers);
        assert_eq!(bearer.as_deref(), Some("abc"));
        assert_eq!(ignored, ["X-Extra"]);
        let basic = BTreeMap::from([("Authorization".to_owned(), "Basic xyz".to_owned())]);
        assert_eq!(bearer_of(&basic), (None, vec!["Authorization".to_owned()]));
    }
}
