//! Launch plans for Claude Code sessions (SPEC-002 §7.2, ADR-05, ADR-07).
//!
//! [`plan_launch`] is pure: it turns a [`LaunchRequest`] into the argv, environment and config
//! files of one run. The caller (supervisor) writes [`LaunchPlan::files`], adds the common
//! environment (`HOME`, `TERM`, `YULIUS_TASK_TOKEN`, git identity, ports, MCP server secrets)
//! and spawns `argv` without a shell in `cwd`.
//!
//! Secrets appear only in [`LaunchPlan::secret_env`] and in files with mode `0600`; never in
//! argv. `Debug` output of plans redacts both.

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use yulius_domain::harness::Harness;

use crate::paths::valid_session_id;
use crate::secret::Secret;

/// Claude Code executable name (resolved through `PATH` by the spawner).
pub const CLAUDE_PROGRAM: &str = "claude";
/// Environment variable holding the per-attempt Yulius bearer token (set by the supervisor).
pub const TASK_TOKEN_ENV: &str = "YULIUS_TASK_TOKEN";
/// Environment variable holding the internal Yulius listener URL.
pub const YULIUS_URL_ENV: &str = "YULIUS_URL";
/// Turns off Claude Code's background self-update: the operator installs and updates the CLI
/// on the host, and a contained session cannot write there, so the update could only fail
/// (#45). Manual `claude update` is unaffected.
pub const DISABLE_AUTOUPDATER_ENV: (&str, &str) = ("DISABLE_AUTOUPDATER", "1");
/// Name of the always-attached Yulius MCP server.
pub const YULIUS_MCP_NAME: &str = "yulius";
/// Hook events registered for Claude Code. `SessionEnd` tells Yulius at once why a session ends
/// (`/exit`, Ctrl+C twice, `/clear`, a signal), before the process is gone.
pub const CLAUDE_HOOK_EVENTS: [&str; 5] = [
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
];
/// The pseudo-event of Claude Code's `statusLine` command (`yulius hook claude StatusLine`): it
/// prints the task's status line and forwards the plan usage Claude Code read from its own API
/// responses, the only usage a `claude setup-token` sign-in can see (SPEC-002 ADR-17).
pub const CLAUDE_STATUS_LINE_EVENT: &str = "StatusLine";
/// Claude settings file name inside the run directory.
pub const CLAUDE_SETTINGS_FILE: &str = "claude-settings.json";
/// Claude MCP config file name inside the run directory.
pub const CLAUDE_MCP_FILE: &str = "mcp.json";
/// Mode of every planned file (owner read/write only).
pub const PRIVATE_FILE_MODE: u32 = 0o600;
/// Claude hook timeout in seconds; the hook client itself gives up after 5 s (fail-open).
pub const CLAUDE_HOOK_TIMEOUT_SECONDS: u64 = 15;
/// Hook events Yulius may hold while no subscription has capacity (SPEC-002 ADR-17): the turn
/// boundaries.
pub const CLAUDE_HELD_HOOK_EVENTS: [&str; 2] = ["Stop", "UserPromptSubmit"];
/// Claude's timeout for the held events: an hour longer than the hook client's own hold bound
/// ([`crate::hook_client::HOLD_MAX`], 8 days), so the client always gives up first and fails
/// open, and far below the 24.8 days at which a Node.js timer would overflow and fire at once.
pub const CLAUDE_HELD_HOOK_TIMEOUT_SECONDS: u64 = 8 * 24 * 3600 + 3600;

/// How the session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    /// A new session.
    New {
        /// Session id to assign (`--session-id`).
        session_id: Option<String>,
        /// Initial prompt.
        prompt: Option<String>,
    },
    /// Native resume of an existing session.
    Resume {
        /// Provider session id to resume.
        session_id: String,
        /// Continuation prompt.
        prompt: Option<String>,
    },
}

/// Transport of an attached MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// A local process speaking MCP on stdio.
    Stdio {
        /// Executable.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// Names of environment variables forwarded to the server; their values are supplied
        /// by the supervisor in the child environment.
        env_names: Vec<String>,
    },
    /// A Streamable HTTP server.
    Http {
        /// Endpoint URL.
        url: String,
        /// Environment variable holding a bearer token, if the server needs one.
        bearer_env: Option<String>,
    },
}

/// An MCP server attached to every session (SPEC-001 MCP-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    /// Server name: 1–64 ASCII letters, digits, `-` or `_`; `yulius` is reserved.
    pub name: String,
    /// Transport.
    pub transport: McpTransport,
}

/// The provider credential of the account the run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// Claude long-lived OAuth token from `claude setup-token`.
    ClaudeOauthToken(Secret<String>),
}

/// Everything needed to plan one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// Harness to run.
    pub harness: Harness,
    /// New session or resume.
    pub mode: LaunchMode,
    /// Task worktree (working directory of the run).
    pub worktree: PathBuf,
    /// Sandbox home; Claude's config dir is `<sandbox_home>/.claude`.
    pub sandbox_home: PathBuf,
    /// Run directory for generated config files.
    pub run_dir: PathBuf,
    /// Model override.
    pub model: Option<String>,
    /// Reasoning effort override.
    pub effort: Option<String>,
    /// The run is inside an OS containment boundary; only then are approvals bypassed.
    pub contained: bool,
    /// Path of the `yulius` executable used by hooks.
    pub yulius_exe: PathBuf,
    /// Internal Yulius listener URL (`http://127.0.0.1:<port>`).
    pub yulius_url: String,
    /// Additional MCP servers.
    pub mcp_servers: Vec<McpServerSpec>,
    /// Provider credential.
    pub credential: Credential,
}

/// A config file the supervisor writes before spawning.
#[derive(Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Contents (may be secret; never printed by `Debug`).
    pub contents: Vec<u8>,
    /// Unix permission bits.
    pub mode: u32,
}

impl fmt::Debug for PlannedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedFile")
            .field("path", &self.path)
            .field("bytes", &self.contents.len())
            .field("mode", &format_args!("{:o}", self.mode))
            .finish()
    }
}

impl PlannedFile {
    /// Writes the file crash-safely: a temporary file in the same directory created with
    /// [`Self::mode`], fsync, rename over the target, fsync of the directory. The parent
    /// directory must exist.
    ///
    /// # Errors
    /// Any I/O error; the temporary file is removed on failure.
    pub fn write(&self) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        let mut tmp_name = name.to_os_string();
        tmp_name.push(format!(".tmp-{}", std::process::id()));
        let tmp = parent.join(tmp_name);
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(self.mode);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(&self.contents)?;
            file.sync_all()?;
            std::fs::rename(&tmp, &self.path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ignored = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// The argv, environment and files of one run.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Executable name followed by arguments; spawn without a shell.
    pub argv: Vec<String>,
    /// Working directory (the worktree).
    pub cwd: PathBuf,
    /// Non-secret environment variables specific to the harness.
    pub env: Vec<(String, String)>,
    /// Secret environment variables (never logged).
    pub secret_env: Vec<(String, String)>,
    /// Files to write before spawning.
    pub files: Vec<PlannedFile>,
    /// Session id the run will have, when known before spawning.
    pub session_id: Option<String>,
}

impl fmt::Debug for LaunchPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secret_names: Vec<&str> = self.secret_env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("LaunchPlan")
            .field("argv", &self.argv)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("secret_env", &secret_names)
            .field("files", &self.files)
            .field("session_id", &self.session_id)
            .finish()
    }
}

/// A request that cannot be planned. Messages name the field, never its value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    /// A path that must appear in argv or config is not valid UTF-8.
    #[error("path {0} is not valid UTF-8")]
    NonUtf8Path(&'static str),
    /// A field has a value that cannot be passed safely.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// Two MCP servers share a name, or a server uses the reserved name.
    #[error("duplicate or reserved MCP server name {0:?}")]
    DuplicateMcpServer(String),
}

fn utf8<'a>(path: &'a Path, field: &'static str) -> Result<&'a str, LaunchError> {
    path.to_str().ok_or(LaunchError::NonUtf8Path(field))
}

fn check(ok: bool, field: &'static str) -> Result<(), LaunchError> {
    if ok {
        Ok(())
    } else {
        Err(LaunchError::Invalid(field))
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && !model.starts_with('-')
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@[]-".contains(&b))
}

fn valid_effort(effort: &str) -> bool {
    !effort.is_empty()
        && effort.len() <= 32
        && !effort.starts_with('-')
        && effort
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() <= 2048
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'')
}

/// POSIX shell single-quoting (hook commands are run by the CLI through a shell).
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn validate(request: &LaunchRequest) -> Result<(), LaunchError> {
    let (session_id, prompt) = match &request.mode {
        LaunchMode::New { session_id, prompt } => (session_id.as_deref(), prompt.as_deref()),
        LaunchMode::Resume { session_id, prompt } => (Some(session_id.as_str()), prompt.as_deref()),
    };
    if let Some(id) = session_id {
        check(valid_session_id(id), "session_id")?;
    }
    if let Some(prompt) = prompt {
        check(!prompt.contains('\0'), "prompt")?;
    }
    if let Some(model) = &request.model {
        check(valid_model(model), "model")?;
    }
    if let Some(effort) = &request.effort {
        check(valid_effort(effort), "effort")?;
    }
    check(valid_url(&request.yulius_url), "yulius_url")?;
    let mut names = vec![YULIUS_MCP_NAME];
    for server in &request.mcp_servers {
        check(valid_server_name(&server.name), "mcp server name")?;
        if names.contains(&server.name.as_str()) {
            return Err(LaunchError::DuplicateMcpServer(server.name.clone()));
        }
        names.push(&server.name);
        match &server.transport {
            McpTransport::Stdio {
                command, env_names, ..
            } => {
                check(!command.is_empty(), "mcp server command")?;
                check(env_names.iter().all(|n| valid_env_name(n)), "mcp env name")?;
            }
            McpTransport::Http { url, bearer_env } => {
                check(valid_url(url), "mcp server url")?;
                check(
                    bearer_env.as_deref().is_none_or(valid_env_name),
                    "mcp bearer env name",
                )?;
            }
        }
    }
    Ok(())
}

/// Builds the launch plan of one run.
///
/// # Errors
/// [`LaunchError`] when a path is not UTF-8, or a session id, model, effort, prompt, URL or MCP
/// server definition cannot be passed safely.
pub fn plan_launch(request: &LaunchRequest) -> Result<LaunchPlan, LaunchError> {
    validate(request)?;
    match request.harness {
        Harness::ClaudeCode => plan_claude(request),
    }
}

fn base_url(request: &LaunchRequest) -> &str {
    request.yulius_url.trim_end_matches('/')
}

fn hook_command(
    request: &LaunchRequest,
    harness: &str,
    event: &str,
) -> Result<String, LaunchError> {
    let exe = utf8(&request.yulius_exe, "yulius_exe")?;
    Ok(format!("{} hook {harness} {event}", shell_quote(exe)))
}

/// Appends the prompt as the final argv element, preceded by `--` when it starts with `-` so
/// the CLI cannot read it as an option.
fn push_prompt(argv: &mut Vec<String>, prompt: Option<&str>) {
    if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
        if prompt.starts_with('-') {
            argv.push("--".into());
        }
        argv.push(prompt.to_owned());
    }
}

fn plan_claude(request: &LaunchRequest) -> Result<LaunchPlan, LaunchError> {
    let Credential::ClaudeOauthToken(token) = &request.credential;
    check(
        !token.expose().is_empty() && !token.expose().contains(['\0', '\n', '\r']),
        "credential",
    )?;
    let settings_path = request.run_dir.join(CLAUDE_SETTINGS_FILE);
    let mcp_path = request.run_dir.join(CLAUDE_MCP_FILE);
    let config_dir = request.sandbox_home.join(".claude");

    let mut hooks = Map::new();
    for event in CLAUDE_HOOK_EVENTS {
        let timeout = if CLAUDE_HELD_HOOK_EVENTS.contains(&event) {
            CLAUDE_HELD_HOOK_TIMEOUT_SECONDS
        } else {
            CLAUDE_HOOK_TIMEOUT_SECONDS
        };
        hooks.insert(
            event.to_owned(),
            json!([{ "hooks": [{
                "type": "command",
                "command": hook_command(request, "claude", event)?,
                "timeout": timeout
            }] }]),
        );
    }
    let settings = json!({
        "hooks": hooks,
        "statusLine": {
            "type": "command",
            "command": hook_command(request, "claude", CLAUDE_STATUS_LINE_EVENT)?,
            "padding": 0
        }
    });

    let mut servers = Map::new();
    servers.insert(
        YULIUS_MCP_NAME.to_owned(),
        json!({
            "type": "http",
            "url": format!("{}/mcp", base_url(request)),
            "headers": { "Authorization": format!("Bearer ${{{TASK_TOKEN_ENV}}}") }
        }),
    );
    for server in &request.mcp_servers {
        let entry = match &server.transport {
            McpTransport::Stdio {
                command,
                args,
                env_names,
            } => {
                let env: Map<String, Value> = env_names
                    .iter()
                    .map(|n| (n.clone(), Value::String(format!("${{{n}}}"))))
                    .collect();
                json!({ "type": "stdio", "command": command, "args": args, "env": env })
            }
            McpTransport::Http { url, bearer_env } => {
                let mut entry = json!({ "type": "http", "url": url });
                if let (Some(name), Some(object)) = (bearer_env, entry.as_object_mut()) {
                    object.insert(
                        "headers".into(),
                        json!({ "Authorization": format!("Bearer ${{{name}}}") }),
                    );
                }
                entry
            }
        };
        servers.insert(server.name.clone(), entry);
    }
    let mcp = json!({ "mcpServers": servers });

    let mut argv = vec![CLAUDE_PROGRAM.to_owned()];
    let (session_id, prompt) = match &request.mode {
        LaunchMode::New { session_id, prompt } => {
            if let Some(id) = session_id {
                argv.extend(["--session-id".into(), id.clone()]);
            }
            (session_id.clone(), prompt.as_deref())
        }
        LaunchMode::Resume { session_id, prompt } => {
            argv.extend(["--resume".into(), session_id.clone()]);
            (Some(session_id.clone()), prompt.as_deref())
        }
    };
    // `--mcp-config` takes several values; a following option ends the list, so it is never
    // directly followed by the prompt.
    argv.extend([
        "--settings".into(),
        utf8(&settings_path, "run_dir")?.to_owned(),
        "--mcp-config".into(),
        utf8(&mcp_path, "run_dir")?.to_owned(),
        "--strict-mcp-config".into(),
    ]);
    if let Some(model) = &request.model {
        argv.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &request.effort {
        argv.extend(["--effort".into(), effort.clone()]);
    }
    if request.contained {
        argv.push("--dangerously-skip-permissions".into());
    }
    push_prompt(&mut argv, prompt);

    Ok(LaunchPlan {
        argv,
        cwd: request.worktree.clone(),
        env: vec![
            (
                "CLAUDE_CONFIG_DIR".into(),
                utf8(&config_dir, "sandbox_home")?.to_owned(),
            ),
            (YULIUS_URL_ENV.into(), base_url(request).to_owned()),
            (
                DISABLE_AUTOUPDATER_ENV.0.into(),
                DISABLE_AUTOUPDATER_ENV.1.into(),
            ),
        ],
        secret_env: vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), token.expose().clone())],
        files: vec![
            PlannedFile {
                path: settings_path,
                contents: pretty(&settings),
                mode: PRIVATE_FILE_MODE,
            },
            PlannedFile {
                path: mcp_path,
                contents: pretty(&mcp),
                mode: PRIVATE_FILE_MODE,
            },
        ],
        session_id,
    })
}

fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_handles_quotes() {
        assert_eq!(shell_quote("/opt/yulius"), "'/opt/yulius'");
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
    }

    #[test]
    fn validators() {
        assert!(valid_model("claude-opus-4-1[1m]"));
        assert!(valid_model("claude-sonnet-5-5"));
        assert!(!valid_model("--help"));
        assert!(!valid_model("a b"));
        assert!(valid_effort("xhigh"));
        assert!(!valid_effort("-x"));
        assert!(valid_env_name("APIFY_TOKEN"));
        assert!(!valid_env_name("1X"));
        assert!(!valid_env_name("A-B"));
        assert!(valid_url("http://127.0.0.1:1/x"));
        assert!(!valid_url("ftp://x"));
        assert!(!valid_url("http://x\"y"));
    }
}
