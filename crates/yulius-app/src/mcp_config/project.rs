//! A repository's `.mcp.json` (Claude Code's project MCP servers) feeding the sandbox's MCP
//! servers (SPEC-095, ADR-0078).
//!
//! Before a task's run and before a Superagent starts, each server of the sandbox repository's
//! `.mcp.json` that the sandbox and the instance lack is added as a sandbox server, so it reaches
//! every run like any server configured in Yulius. A server already configured is never changed:
//! one turned off in Yulius stays off. Values naming an environment variable (`${VAR}`,
//! `${VAR:-default}`, `Bearer ${VAR}`) become references to the agent secret of that name
//! (`${secret:VAR}`, MCP-003). The Superagent gets every name in the file to reject, so Claude
//! Code never asks about them. Nothing here stops a run: a file that cannot be read or a server
//! that cannot be added is logged and skipped.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read as _};
use std::path::Path;

use serde_json::{Map, Value};
use yulius_contracts::api::mcp::McpServerInput;

use super::{MAX_SERVERS_PER_SCOPE, McpServerRecord, name_key, validate};
use crate::context::App;
use crate::error::AppResult;
use crate::model::SandboxRecord;
use crate::projects::blocking;

/// File name of a repository's project MCP servers.
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
/// Largest `.mcp.json` read; a larger one is ignored.
pub const MAX_PROJECT_MCP_BYTES: u64 = 256 * 1024;
/// Most servers taken from one `.mcp.json` (one scope's limit).
pub const MAX_PROJECT_SERVERS: usize = MAX_SERVERS_PER_SCOPE as usize;
/// Audit actor of an imported server.
const IMPORT_ACTOR: &str = "yulius";

/// The servers of one `.mcp.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectMcp {
    /// Every server name in the file: Claude Code asks about each one.
    pub names: Vec<String>,
    /// The servers Yulius can attach (stdio and http with fields it accepts), as sandbox servers.
    pub servers: Vec<McpServerInput>,
}

/// Parses `.mcp.json` text; `None` when it is not an object with an `mcpServers` object. Skipped
/// servers are logged and still named.
#[must_use]
pub fn parse_project_mcp(text: &str) -> Option<ProjectMcp> {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let Some(Value::Object(servers)) = root.get("mcpServers") else {
        return None;
    };
    let mut project = ProjectMcp::default();
    for (name, server) in servers {
        project.names.push(name.clone());
        if project.servers.len() >= MAX_PROJECT_SERVERS {
            tracing::warn!(server = %name, "more than {MAX_PROJECT_SERVERS} servers in .mcp.json; the rest are not imported");
            continue;
        }
        match server_input(name, server) {
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
fn server_input(name: &str, server: &Value) -> Option<McpServerInput> {
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
            input.env = secrets(server.get("env"))?;
        }
        "http" => {
            input.url = Some(text("url")?.to_owned());
            input.headers = secrets(server.get("headers"))?;
        }
        _ => return None,
    }
    Some(input)
}

/// An `env` or `headers` object with environment references turned into secret references;
/// `None` when it is not an object of strings. Empty values are left out.
fn secrets(map: Option<&Value>) -> Option<Option<BTreeMap<String, String>>> {
    let Some(map) = map else {
        return Some(None);
    };
    let map: &Map<String, Value> = map.as_object()?;
    let mut out = BTreeMap::new();
    for (key, value) in map {
        let value = value.as_str()?;
        if !value.is_empty() {
            out.insert(key.clone(), secret_value(value));
        }
    }
    Some((!out.is_empty()).then_some(out))
}

/// `${VAR}` or `${VAR:-default}` → `${secret:VAR}`, `Bearer ${VAR}` → `Bearer ${secret:VAR}`;
/// anything else as written.
fn secret_value(value: &str) -> String {
    if let Some(var) = env_ref(value) {
        return format!("${{secret:{var}}}");
    }
    if let Some((scheme, rest)) = value.split_once(' ')
        && scheme.eq_ignore_ascii_case("bearer")
        && let Some(var) = env_ref(rest.trim())
    {
        return format!("{scheme} ${{secret:{var}}}");
    }
    value.to_owned()
}

/// The `VAR` of a value that is exactly `${VAR}` or `${VAR:-default}`.
fn env_ref(value: &str) -> Option<&str> {
    let inner = value.strip_prefix("${")?.strip_suffix('}')?;
    let var = inner.split_once(":-").map_or(inner, |(var, _)| var);
    let mut bytes = var.bytes();
    let valid = var.len() <= super::MAX_SECRET_REF_NAME
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    valid.then_some(var)
}

/// The servers of `repo`'s `.mcp.json`; `None` when it is missing, larger than
/// [`MAX_PROJECT_MCP_BYTES`], unreadable or invalid (all but missing are logged).
#[must_use]
pub fn read_project_mcp(repo: &Path) -> Option<ProjectMcp> {
    let path = repo.join(PROJECT_MCP_FILE);
    let text = match std::fs::File::open(&path).and_then(|file| {
        let mut text = String::new();
        file.take(MAX_PROJECT_MCP_BYTES + 1)
            .read_to_string(&mut text)?;
        Ok(text)
    }) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "the repository's .mcp.json is unreadable; its servers are not imported");
            return None;
        }
    };
    if text.len() as u64 > MAX_PROJECT_MCP_BYTES {
        tracing::warn!(path = %path.display(), "the repository's .mcp.json is larger than 256 KiB; its servers are not imported");
        return None;
    }
    let project = parse_project_mcp(&text);
    if project.is_none() {
        tracing::warn!(path = %path.display(), "the repository's .mcp.json has no mcpServers object; its servers are not imported");
    }
    project
}

impl App {
    /// Adds each server of the sandbox repository's `.mcp.json` that no instance or sandbox
    /// server of `sandbox` has as an enabled sandbox server, audited `mcp.imported` (SPEC-095
    /// PMC-001). Returns every server name in the file, for the Superagent to reject (PMC-004).
    /// Never fails: what cannot be read or added is logged and skipped (PMC-003).
    pub(crate) async fn import_project_mcp(&self, sandbox: &SandboxRecord) -> Vec<String> {
        let repo = self.sandbox_repo(&sandbox.id);
        let project = match blocking(move || Ok(read_project_mcp(&repo))).await {
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
            if existing.contains(&name_key(&input.name)) {
                continue;
            }
            if let Err(err) = self.import_mcp_server(sandbox, input).await {
                tracing::warn!(error = %err, server = %input.name, sandbox = %sandbox.id, "a .mcp.json server could not be added");
            }
        }
        project.names
    }

    async fn import_mcp_server(
        &self,
        sandbox: &SandboxRecord,
        input: &McpServerInput,
    ) -> AppResult<()> {
        let record = self.new_mcp_record(Some(sandbox.id.clone()), input)?;
        self.deps
            .store
            .insert_mcp_server(&record, MAX_SERVERS_PER_SCOPE)
            .await?;
        tracing::info!(server = %record.name, sandbox = %sandbox.id, "a .mcp.json server joined the sandbox's MCP servers");
        self.deps
            .store
            .audit(
                self.now(),
                IMPORT_ACTOR,
                "mcp.imported",
                Some(&record.id),
                Some(&mcp_detail(&record)),
            )
            .await
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

    fn parsed(text: &str) -> ProjectMcp {
        parse_project_mcp(text).unwrap_or_default()
    }

    #[test]
    fn stdio_http_and_typeless_servers_are_imported_with_secret_references() {
        let project = parsed(
            r#"{"mcpServers": {
                "supabase": {"command": "npx", "args": ["-y", "@supabase/mcp-server-supabase"],
                             "env": {"SUPABASE_ACCESS_TOKEN": "${SUPABASE_ACCESS_TOKEN}",
                                     "REGION": "eu-1", "UNSET": ""}},
                "apify": {"type": "stdio", "command": "npx",
                          "env": {"APIFY_TOKEN": "${APIFY_TOKEN:-none}"}},
                "pipedream-gmail": {"url": "https://remote.mcp.pipedream.test/gmail",
                                    "headers": {"Authorization": "Bearer ${PIPEDREAM_TOKEN}",
                                                "X-Team": "${secret:team}"}}
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
        assert_eq!(
            apify.env,
            Some(BTreeMap::from([(
                "APIFY_TOKEN".into(),
                "${secret:APIFY_TOKEN}".into()
            )]))
        );
        assert_eq!(supabase.args, ["-y", "@supabase/mcp-server-supabase"]);
        assert_eq!(
            supabase.env,
            Some(BTreeMap::from([
                ("REGION".into(), "eu-1".into()),
                (
                    "SUPABASE_ACCESS_TOKEN".into(),
                    "${secret:SUPABASE_ACCESS_TOKEN}".into()
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
                    "Bearer ${secret:PIPEDREAM_TOKEN}".into()
                ),
                ("X-Team".into(), "${secret:team}".into()),
            ]))
        );
        assert!(
            project
                .servers
                .iter()
                .all(|s| s.enabled == Some(true) && s.scope.as_deref() == Some("sandbox"))
        );
    }

    #[test]
    fn values_that_are_not_a_whole_reference_are_kept_as_written() {
        for (value, kept) in [
            ("plain-token", "plain-token"),
            ("prefix-${VAR}", "prefix-${VAR}"),
            ("${1BAD}", "${1BAD}"),
            ("${}", "${}"),
            ("Basic ${VAR}", "Basic ${VAR}"),
            ("bearer ${VAR}", "bearer ${secret:VAR}"),
            ("${VAR:-}", "${secret:VAR}"),
        ] {
            assert_eq!(secret_value(value), kept, "{value}");
        }
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
            assert_eq!(parse_project_mcp(text), None, "{text}");
        }
        assert_eq!(
            parse_project_mcp(r#"{"mcpServers": {}}"#),
            Some(ProjectMcp::default())
        );
    }

    #[test]
    fn at_most_one_scope_of_servers_is_imported_and_every_name_is_kept() {
        let servers: Vec<String> = (0..MAX_PROJECT_SERVERS + 3)
            .map(|i| format!(r#""s{i:03}": {{"command": "x"}}"#))
            .collect();
        let project = parsed(&format!(r#"{{"mcpServers": {{{}}}}}"#, servers.join(",")));
        assert_eq!(project.names.len(), MAX_PROJECT_SERVERS + 3);
        assert_eq!(project.servers.len(), MAX_PROJECT_SERVERS);
    }

    #[test]
    fn a_missing_oversized_or_invalid_file_reads_as_none() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        assert_eq!(read_project_mcp(dir.path()), None);
        let file = dir.path().join(PROJECT_MCP_FILE);
        std::fs::write(&file, "{ not json")?;
        assert_eq!(read_project_mcp(dir.path()), None);
        let padding = " ".repeat(usize::try_from(MAX_PROJECT_MCP_BYTES)?);
        std::fs::write(
            &file,
            format!(r#"{{"mcpServers": {{"a": {{"command": "x"}}}}}}{padding}"#),
        )?;
        assert_eq!(read_project_mcp(dir.path()), None);
        std::fs::write(&file, r#"{"mcpServers": {"a": {"command": "x"}}}"#)?;
        assert_eq!(
            read_project_mcp(dir.path()).map(|p| p.names),
            Some(vec!["a".to_owned()])
        );
        Ok(())
    }
}
