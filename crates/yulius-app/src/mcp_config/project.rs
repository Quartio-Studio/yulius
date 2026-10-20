//! A repository's `.mcp.json` (Claude Code's project MCP servers) feeding the sandbox's MCP
//! servers (SPEC-095, ADR-0078).
//!
//! Before a task's run and before a Superagent starts, each server of the sandbox repository's
//! `.mcp.json` that the sandbox and the instance lack is added as a sandbox server, so it reaches
//! every run like any server configured in Yulius. A server already configured is never changed:
//! one turned off in Yulius stays off. The Superagent gets every name in the file to reject, so
//! Claude Code never asks about them. Nothing here stops a run: a file that cannot be read or a
//! server that cannot be added is logged and skipped.
//!
//! Agents can write the file, so it never chooses which command receives a secret's value. A value
//! naming a variable (`${VAR}`, `${VAR:-default}`, `${secret:VAR}`, `Bearer ${VAR}`) becomes the
//! agent secret's placeholder, `yulius-secret-VAR`, as agents get it in their own environment: the
//! egress proxy puts the value in on the secret's bound hosts only (SPEC-002 ADR-20). A stored
//! value is never a `${secret:…}` reference, which would hand the value itself to the server.
//! Only a person changing the server on the MCP page can add one.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde_json::{Map, Value};
use yulius_contracts::api::mcp::McpServerInput;
use yulius_domain::vault::{SecretName, placeholder};

use super::{
    MAX_SECRET_BYTES, MAX_SERVERS_PER_SCOPE, McpOrigin, McpServerRecord, bearer_of, name_key,
    secret_ref, validate,
};
use crate::context::App;
use crate::error::AppResult;
use crate::hooks::read_repo_doc;
use crate::model::SandboxRecord;
use crate::projects::blocking;

/// File name of a repository's project MCP servers.
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
/// Most servers taken from one `.mcp.json` (one scope's limit).
pub const MAX_PROJECT_SERVERS: usize = MAX_SERVERS_PER_SCOPE as usize;
/// Most bytes of server names handed to the Superagent: they travel in one argv element, which
/// Linux bounds at 128 KiB.
pub const MAX_PROJECT_NAME_BYTES: usize = 64 * 1024;
/// Audit actor of an imported server.
const IMPORT_ACTOR: &str = "yulius";

/// The servers of one `.mcp.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectMcp {
    /// The server names in the file (Claude Code asks about each), up to
    /// [`MAX_PROJECT_NAME_BYTES`].
    pub names: Vec<String>,
    /// The servers Yulius can attach (stdio and http with fields it accepts), as sandbox servers.
    pub servers: Vec<McpServerInput>,
}

/// Parses `.mcp.json` text; `None` when it is not an object with an `mcpServers` object.
/// `agent_secrets` are the names of the agent secrets that apply to the sandbox: a
/// `${VAR:-default}` value is the secret's placeholder when VAR is one, else the default. Skipped
/// servers are logged and still named.
#[must_use]
pub fn parse_project_mcp(text: &str, agent_secrets: &BTreeSet<String>) -> Option<ProjectMcp> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let Some(Value::Object(servers)) = root.get("mcpServers") else {
        return None;
    };
    let mut project = ProjectMcp::default();
    let mut name_bytes = 0;
    for (name, server) in servers {
        name_bytes += name.len() + 4;
        if name_bytes <= MAX_PROJECT_NAME_BYTES {
            project.names.push(name.clone());
        }
        if project.servers.len() >= MAX_PROJECT_SERVERS {
            tracing::warn!(server = %name, "more than {MAX_PROJECT_SERVERS} servers in .mcp.json; the rest are not imported");
            continue;
        }
        match server_input(name, server, agent_secrets) {
            Some(input) if validate(&input).is_ok() => project.servers.push(input),
            _ => {
                tracing::warn!(server = %name, "a .mcp.json server Yulius cannot attach (only stdio and http) is not imported");
            }
        }
    }
    Some(project)
}

/// One `.mcp.json` entry as a sandbox server: `type` stdio or http, or no type with a `command`
/// (stdio) or a `url` (http).
fn server_input(
    name: &str,
    server: &Value,
    agent_secrets: &BTreeSet<String>,
) -> Option<McpServerInput> {
    let server = server.as_object()?;
    let text = |key: &str| server.get(key).and_then(Value::as_str);
    let transport = match text("type") {
        Some(kind) => kind,
        None if server.contains_key("command") => "stdio",
        None if server.contains_key("url") => "http",
        None => return None,
    };
    let mut input = McpServerInput {
        name: name.to_owned(),
        transport: transport.to_owned(),
        command: None,
        args: Vec::new(),
        url: None,
        env: None,
        headers: None,
        enabled: Some(true),
        scope: Some("sandbox".into()),
    };
    match transport {
        "stdio" => {
            input.command = Some(text("command")?.to_owned());
            input.args = match server.get("args") {
                None => Vec::new(),
                Some(args) => args
                    .as_array()?
                    .iter()
                    .map(|a| a.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?,
            };
            input.env = values(name, server.get("env"), agent_secrets)?;
        }
        "http" => {
            input.url = Some(text("url")?.to_owned());
            input.headers = values(name, server.get("headers"), agent_secrets)?;
            if let Some(headers) = &input.headers {
                let (_, ignored) = bearer_of(headers);
                if !ignored.is_empty() {
                    tracing::warn!(server = %name, headers = ?ignored, "runs get only an Authorization: Bearer header; the other .mcp.json headers are kept but not sent");
                }
            }
        }
        _ => return None,
    }
    Some(input)
}

/// An `env` or `headers` object with variables turned into placeholders; `None` when it is not an
/// object of strings. Empty values, and values that cannot be stored safely, are left out.
fn values(
    server: &str,
    map: Option<&Value>,
    agent_secrets: &BTreeSet<String>,
) -> Option<Option<BTreeMap<String, String>>> {
    let Some(map) = map else {
        return Some(None);
    };
    let map: &Map<String, Value> = map.as_object()?;
    let mut out = BTreeMap::new();
    for (key, value) in map {
        let value = value.as_str()?;
        match file_value(value, agent_secrets) {
            Some(value) if value.is_empty() => {}
            // What `validate` would refuse for the whole server leaves out only this value.
            Some(value)
                if value.len() <= MAX_SECRET_BYTES && !value.contains(['\0', '\r', '\n']) =>
            {
                out.insert(key.clone(), value);
            }
            Some(_) => {
                tracing::warn!(server = %server, key = %key, "a .mcp.json value is not one line of at most 16 KiB; it is left out");
            }
            None => {
                tracing::warn!(server = %server, key = %key, "a .mcp.json value names no valid agent secret; it is left out");
            }
        }
    }
    Some((!out.is_empty()).then_some(out))
}

/// The stored form of a value from the file: a whole variable reference (also after `Bearer `)
/// becomes the agent secret's placeholder, anything else stays as written. `None` when the
/// reference names no valid agent secret, or when the result would still be a secret reference.
fn file_value(value: &str, agent_secrets: &BTreeSet<String>) -> Option<String> {
    let stored = if let Some(reference) = Reference::of(value) {
        reference.stored(agent_secrets)?
    } else if let Some((scheme, rest)) = value.split_once(' ')
        && scheme.eq_ignore_ascii_case("bearer")
        && let Some(reference) = Reference::of(rest.trim())
    {
        format!("{scheme} {}", reference.stored(agent_secrets)?)
    } else {
        value.to_owned()
    };
    // What [`App::mcp_attachments`] would resolve to a clear value is never stored from the file.
    let probe = BTreeMap::from([("Authorization".to_owned(), stored.clone())]);
    let resolvable = secret_ref(&stored).is_some()
        || bearer_of(&probe)
            .0
            .as_deref()
            .and_then(secret_ref)
            .is_some();
    (!resolvable).then_some(stored)
}

/// A value that is exactly `${VAR}`, `${VAR:-default}` or `${secret:VAR}`.
struct Reference<'a> {
    var: &'a str,
    default: Option<&'a str>,
}

impl<'a> Reference<'a> {
    fn of(value: &'a str) -> Option<Self> {
        let inner = value.strip_prefix("${")?.strip_suffix('}')?;
        if let Some(var) = inner.strip_prefix("secret:") {
            return Some(Self { var, default: None });
        }
        let (var, default) = match inner.split_once(":-") {
            Some((var, default)) => (var, Some(default)),
            None => (inner, None),
        };
        let mut bytes = var.bytes();
        let shaped = bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
        shaped.then_some(Self { var, default })
    }

    /// The placeholder of VAR; the default instead when there is one and VAR is no agent secret.
    fn stored(&self, agent_secrets: &BTreeSet<String>) -> Option<String> {
        if let Some(default) = self.default
            && !agent_secrets.contains(self.var)
        {
            return Some(default.to_owned());
        }
        SecretName::parse(self.var)
            .ok()
            .filter(|name| name.as_str() == self.var)
            .map(|name| placeholder(&name))
    }
}

/// The servers of `repo`'s `.mcp.json`; `None` when it is missing, not a regular file of the
/// repository (a link out of it, a FIFO, a directory), unreadable or invalid. It is read with the
/// rules for agent-written files the server reads outside containment (`hooks::read_repo_doc`), at most
/// 256 KiB.
#[must_use]
pub fn read_project_mcp(repo: &Path, agent_secrets: &BTreeSet<String>) -> Option<ProjectMcp> {
    let path = repo.join(PROJECT_MCP_FILE);
    let Some(text) = read_repo_doc(repo, PROJECT_MCP_FILE) else {
        if std::fs::symlink_metadata(&path).is_ok() {
            tracing::warn!(path = %path.display(), "the repository's .mcp.json is not a readable regular file of the repository; its servers are not imported");
        }
        return None;
    };
    let project = parse_project_mcp(&text, agent_secrets);
    if project.is_none() {
        tracing::warn!(path = %path.display(), "the repository's .mcp.json is not valid (or larger than 256 KiB); its servers are not imported");
    }
    project
}

impl App {
    /// Adds each server of the sandbox repository's `.mcp.json` that no instance or sandbox
    /// server of `sandbox` has as an enabled sandbox server, audited `mcp.imported` (SPEC-095
    /// PMC-001). Returns the server names in the file, for the Superagent to reject (PMC-004).
    /// Never fails: what cannot be read or added is logged and skipped (PMC-003).
    pub(crate) async fn import_project_mcp(&self, sandbox: &SandboxRecord) -> Vec<String> {
        let agent_secrets: BTreeSet<String> = match self
            .resolved_agent_secrets(&sandbox.id, crate::vault::Holder::Nobody)
            .await
        {
            Ok(records) => records.into_iter().map(|r| r.name).collect(),
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.id, "agent secrets unreadable; .mcp.json defaults are kept");
                BTreeSet::new()
            }
        };
        let repo = self.sandbox_repo(&sandbox.id);
        let project = match blocking(move || Ok(read_project_mcp(&repo, &agent_secrets))).await {
            Ok(Some(project)) => project,
            Ok(None) => return Vec::new(),
            Err(err) => {
                tracing::warn!(error = %err, "the repository's .mcp.json could not be read");
                return Vec::new();
            }
        };
        let existing: HashSet<String> = match self.deps.store.mcp_servers(&sandbox.id).await {
            Ok(servers) => servers.iter().map(|s| name_key(&s.name)).collect(),
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.id, "MCP servers unreadable; .mcp.json servers are not imported");
                return project.names;
            }
        };
        for input in &project.servers {
            if !existing.contains(&name_key(&input.name)) {
                self.import_mcp_server(sandbox, input).await;
            }
        }
        project.names
    }

    async fn import_mcp_server(&self, sandbox: &SandboxRecord, input: &McpServerInput) {
        let added: AppResult<McpServerRecord> = async {
            let record =
                self.new_mcp_record(Some(sandbox.id.clone()), input, McpOrigin::McpJson)?;
            self.deps
                .store
                .insert_mcp_server(&record, MAX_SERVERS_PER_SCOPE)
                .await?;
            Ok(record)
        }
        .await;
        let record = match added {
            Ok(record) => record,
            Err(err) => {
                tracing::warn!(error = %err, server = %input.name, sandbox = %sandbox.id, "a .mcp.json server could not be added");
                return;
            }
        };
        tracing::info!(server = %record.name, sandbox = %sandbox.id, "a .mcp.json server joined the sandbox's MCP servers");
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                IMPORT_ACTOR,
                "mcp.imported",
                Some(&record.id),
                Some(&mcp_detail(&record)),
            )
            .await
        {
            tracing::warn!(error = %err, server = %record.name, "the import of a .mcp.json server could not be audited");
        }
    }
}

/// `name (scope, transport)`, as the MCP audit entries say it.
pub(super) fn mcp_detail(record: &McpServerRecord) -> String {
    format!(
        "{} ({}, {})",
        record.name,
        record.scope(),
        record.transport.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn parsed(text: &str) -> ProjectMcp {
        parse_project_mcp(text, &secrets(&["APIFY_TOKEN"])).unwrap_or_default()
    }

    #[test]
    fn stdio_http_and_typeless_servers_are_imported_with_placeholders() {
        let project = parsed(
            r#"{"mcpServers": {
                "supabase": {"command": "npx", "args": ["-y", "@supabase/mcp-server-supabase"],
                             "env": {"SUPABASE_ACCESS_TOKEN": "${SUPABASE_ACCESS_TOKEN}",
                                     "REGION": "eu-1", "UNSET": "", "LOG": "${LOG_LEVEL:-info}"}},
                "apify": {"type": "stdio", "command": "npx",
                          "env": {"APIFY_TOKEN": "${APIFY_TOKEN:-none}"}},
                "pipedream-gmail": {"url": "https://remote.mcp.pipedream.test/gmail",
                                    "headers": {"Authorization": "Bearer ${PIPEDREAM_TOKEN}",
                                                "X-Team": "${secret:TEAM_KEY}"}}
            }}"#,
        );
        assert_eq!(project.names, ["apify", "pipedream-gmail", "supabase"]);
        let [apify, gmail, supabase] = project.servers.as_slice() else {
            panic!("{project:?}");
        };
        assert_eq!(
            (apify.transport.as_str(), apify.command.as_deref()),
            ("stdio", Some("npx"))
        );
        assert!(apify.args.is_empty());
        // APIFY_TOKEN is an agent secret: its placeholder wins over the default.
        assert_eq!(
            apify.env,
            Some(BTreeMap::from([(
                "APIFY_TOKEN".into(),
                "yulius-secret-APIFY_TOKEN".into()
            )]))
        );
        assert_eq!(supabase.args, ["-y", "@supabase/mcp-server-supabase"]);
        assert_eq!(
            supabase.env,
            Some(BTreeMap::from([
                ("LOG".into(), "info".into()),
                ("REGION".into(), "eu-1".into()),
                (
                    "SUPABASE_ACCESS_TOKEN".into(),
                    "yulius-secret-SUPABASE_ACCESS_TOKEN".into()
                ),
            ]))
        );
        assert_eq!(gmail.transport, "http");
        assert_eq!(
            gmail.url.as_deref(),
            Some("https://remote.mcp.pipedream.test/gmail")
        );
        assert_eq!(
            gmail.headers,
            Some(BTreeMap::from([
                (
                    "Authorization".into(),
                    "Bearer yulius-secret-PIPEDREAM_TOKEN".into()
                ),
                ("X-Team".into(), "yulius-secret-TEAM_KEY".into()),
            ]))
        );
        assert!(
            project
                .servers
                .iter()
                .all(|s| s.enabled == Some(true) && s.scope.as_deref() == Some("sandbox"))
        );
    }

    /// Agents can write the file: it never stores a value the run would resolve to a secret's
    /// clear value.
    #[test]
    fn a_value_from_the_file_is_never_a_secret_reference() {
        let none = BTreeSet::new();
        for (value, stored) in [
            ("plain-token", Some("plain-token")),
            ("prefix-${VAR}", Some("prefix-${VAR}")),
            ("${secret:STRIPE_KEY}", Some("yulius-secret-STRIPE_KEY")),
            (
                "Bearer ${secret:STRIPE_KEY}",
                Some("Bearer yulius-secret-STRIPE_KEY"),
            ),
            (
                "bearer  ${secret:STRIPE_KEY}",
                Some("bearer yulius-secret-STRIPE_KEY"),
            ),
            (
                "Bearer ${STRIPE_KEY}",
                Some("Bearer yulius-secret-STRIPE_KEY"),
            ),
            ("${secret:team.key}", None),
            ("${secret:}", None),
            ("${lower_case}", None),
            ("${YULIUS_TASK_TOKEN}", None),
            ("${1BAD}", Some("${1BAD}")),
            ("${}", Some("${}")),
            ("Basic ${VAR}", Some("Basic ${VAR}")),
            ("${VAR:-}", Some("")),
            ("${VAR:-${secret:STRIPE_KEY}}", None),
        ] {
            assert_eq!(file_value(value, &none).as_deref(), stored, "{value}");
        }
    }

    /// One value Yulius cannot store (a default with a line break, one over 16 KiB) is left out;
    /// the server is still imported.
    #[test]
    fn a_value_that_cannot_be_stored_leaves_only_itself_out() {
        let long = "x".repeat(MAX_SECRET_BYTES + 1);
        let project = parsed(&format!(
            r#"{{"mcpServers": {{"a": {{"command": "x",
                "env": {{"MULTI": "${{M:-a\nb}}", "LONG": "{long}", "OK": "1"}}}}}}}}"#
        ));
        let [server] = project.servers.as_slice() else {
            panic!("{project:?}");
        };
        assert_eq!(
            server.env,
            Some(BTreeMap::from([("OK".into(), "1".into())]))
        );
    }

    #[test]
    fn servers_yulius_cannot_attach_are_named_but_not_imported() {
        let project = parsed(
            r#"{"mcpServers": {
                "legacy": {"type": "sse", "url": "https://legacy.test/sse"},
                "bad name": {"command": "x"},
                "yulius": {"command": "x"},
                "no-command": {"type": "stdio"},
                "args-not-text": {"command": "x", "args": [1]},
                "env-not-text": {"command": "x", "env": {"A": 1}},
                "ftp": {"type": "http", "url": "ftp://x"},
                "nothing": {},
                "not-an-object": "x",
                "ok": {"command": "ok-mcp"}
            }}"#,
        );
        assert_eq!(project.names.len(), 10);
        let imported: Vec<&str> = project.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(imported, ["ok"]);
    }

    #[test]
    fn a_file_without_an_mcp_servers_object_is_none() {
        for text in [
            "",
            "{ not json",
            "[]",
            "{}",
            r#"{"mcpServers": []}"#,
            r#"{"mcpServers": "x"}"#,
        ] {
            assert_eq!(parse_project_mcp(text, &BTreeSet::new()), None, "{text}");
        }
        assert_eq!(
            parse_project_mcp(r#"{"mcpServers": {}}"#, &BTreeSet::new()),
            Some(ProjectMcp::default())
        );
    }

    #[test]
    fn at_most_one_scope_of_servers_is_imported_and_names_fit_one_argument() {
        let servers: Vec<String> = (0..MAX_PROJECT_SERVERS + 3)
            .map(|i| format!(r#""s{i:03}": {{"command": "x"}}"#))
            .collect();
        let project = parsed(&format!(r#"{{"mcpServers": {{{}}}}}"#, servers.join(",")));
        assert_eq!(project.names.len(), MAX_PROJECT_SERVERS + 3);
        assert_eq!(project.servers.len(), MAX_PROJECT_SERVERS);

        let long: Vec<String> = (0..100)
            .map(|i| format!(r#""{i:03}{}": {{}}"#, "n".repeat(1_000)))
            .collect();
        let project = parsed(&format!(r#"{{"mcpServers": {{{}}}}}"#, long.join(",")));
        let json = serde_json::to_string(&project.names).unwrap_or_default();
        assert!(json.len() <= MAX_PROJECT_NAME_BYTES, "{}", json.len());
        assert!(project.names.len() > 50, "{}", project.names.len());
    }

    #[test]
    fn only_a_regular_file_of_the_repository_is_read() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo)?;
        let none = BTreeSet::new();
        assert_eq!(read_project_mcp(&repo, &none), None);
        let file = repo.join(PROJECT_MCP_FILE);
        std::fs::write(&file, "{ not json")?;
        assert_eq!(read_project_mcp(&repo, &none), None);
        let padding = " ".repeat(256 * 1024);
        std::fs::write(
            &file,
            format!(r#"{padding}{{"mcpServers": {{"a": {{"command": "x"}}}}}}"#),
        )?;
        assert_eq!(read_project_mcp(&repo, &none), None, "larger than 256 KiB");
        std::fs::write(&file, r#"{"mcpServers": {"a": {"command": "x"}}}"#)?;
        assert_eq!(
            read_project_mcp(&repo, &none).map(|p| p.names),
            Some(vec!["a".to_owned()])
        );

        // A link out of the repository (another sandbox's file, the service's own config).
        let outside = dir.path().join("other.json");
        std::fs::write(&outside, r#"{"mcpServers": {"leak": {"command": "x"}}}"#)?;
        std::fs::remove_file(&file)?;
        std::os::unix::fs::symlink(&outside, &file)?;
        assert_eq!(read_project_mcp(&repo, &none), None);
        // A FIFO would block the reader forever; a directory is no file.
        std::fs::remove_file(&file)?;
        nix::unistd::mkfifo(&file, nix::sys::stat::Mode::S_IRWXU)?;
        assert_eq!(read_project_mcp(&repo, &none), None);
        std::fs::remove_file(&file)?;
        std::fs::create_dir(&file)?;
        assert_eq!(read_project_mcp(&repo, &none), None);
        Ok(())
    }
}
