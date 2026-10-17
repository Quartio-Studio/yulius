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
/// Environment variable holding the task's worktree: `yulius remove` removes the task's own files
/// there (SPEC-192 RMV-001).
pub const TASK_WORKTREE_ENV: &str = "YULIUS_TASK_WORKTREE";
/// A run's temporary directory: unset, `$TMPDIR/x` is `/x`, which Claude Code's removal check
/// refuses to remove without a person (SPEC-192). In a contained run `/tmp` is the run's own.
pub const TMPDIR_ENV: (&str, &str) = ("TMPDIR", "/tmp");
/// Turns off Claude Code's background self-update: the operator installs and updates the CLI
/// on the host, and a contained session cannot write there, so the update could only fail
/// (#45). Manual `claude update` is unaffected.
pub const DISABLE_AUTOUPDATER_ENV: (&str, &str) = ("DISABLE_AUTOUPDATER", "1");
/// Claude Code's Bash timeouts for agents: 10 minutes by default, 30 at most.
pub const BASH_TIMEOUT_ENV: [(&str, &str); 2] = [
    ("BASH_DEFAULT_TIMEOUT_MS", "600000"),
    ("BASH_MAX_TIMEOUT_MS", "1800000"),
];
/// The model and effort of a task that names none (Opus 5.5 at high effort).
pub use yulius_domain::harness::{DEFAULT_CLAUDE_EFFORT, DEFAULT_CLAUDE_MODEL};
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
/// Timeout of the `PostToolUse` redaction hook (large tool results take a moment to scan).
pub const CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS: u64 = 60;
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
        /// Continue the turn the transcript left unfinished (SPEC-034): Claude Code resubmits an
        /// unanswered prompt or goes on after a tool result by itself
        /// ([`RESUME_INTERRUPTED_TURN_ENV`]); the prompt is not sent. The caller steps the
        /// transcript back over a usage limit first ([`crate::step_back`]).
        continue_interrupted: bool,
    },
}

/// Claude Code 2.1.292 continues an interrupted turn on `--resume` when this is set: an
/// unanswered prompt at the end of the transcript is submitted again, and a turn cut after a tool
/// result or a task notification goes on with a hidden "Continue from where you left off."
/// (undocumented; verified against the CLI with a local fake API, SPEC-034).
pub const RESUME_INTERRUPTED_TURN_ENV: (&str, &str) = ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN", "1");

/// Transport of an attached MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// A local process speaking MCP on stdio.
    Stdio {
        /// Executable.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// The server's environment: each variable it reads with the run environment variable
        /// holding its value (supplied by the supervisor in the child environment). The two
        /// differ so that a server's variable never clashes with the agent's own environment,
        /// such as an agent secret's placeholder of the same name (SPEC-095).
        env_vars: Vec<(String, String)>,
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
        LaunchMode::Resume {
            session_id, prompt, ..
        } => (Some(session_id.as_str()), prompt.as_deref()),
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
    validate_mcp_servers(&request.mcp_servers)
}

/// Checks MCP server definitions: valid names, unique and never `yulius`, and a command, URL and
/// variable names that can be passed safely.
fn validate_mcp_servers(servers: &[McpServerSpec]) -> Result<(), LaunchError> {
    let mut names = vec![YULIUS_MCP_NAME];
    for server in servers {
        check(valid_server_name(&server.name), "mcp server name")?;
        if names.contains(&server.name.as_str()) {
            return Err(LaunchError::DuplicateMcpServer(server.name.clone()));
        }
        names.push(&server.name);
        match &server.transport {
            McpTransport::Stdio {
                command, env_vars, ..
            } => {
                check(!command.is_empty(), "mcp server command")?;
                check(
                    env_vars
                        .iter()
                        .all(|(name, var)| valid_env_name(name) && valid_env_name(var)),
                    "mcp env name",
                )?;
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
    hook_line(&request.yulius_exe, harness, event)
}

fn hook_line(exe: &Path, harness: &str, event: &str) -> Result<String, LaunchError> {
    let exe = utf8(exe, "yulius_exe")?;
    Ok(format!("{} hook {harness} {event}", shell_quote(exe)))
}

/// The `PostToolUse` redaction hook of agent secrets (SPEC-002 ADR-20): every tool result passes
/// `yulius hook claude PostToolUse`, which replaces one holding a value with its redacted copy.
fn redaction_hook(exe: &Path) -> Result<Value, LaunchError> {
    Ok(json!([{ "matcher": "*", "hooks": [{
        "type": "command",
        "command": hook_line(exe, "claude", "PostToolUse")?,
        "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
    }] }]))
}

/// A server's entry in Claude Code's MCP configuration: secret values only as `${VAR}`
/// references to the environment.
fn mcp_entry(server: &McpServerSpec) -> Value {
    match &server.transport {
        McpTransport::Stdio {
            command,
            args,
            env_vars,
        } => {
            let env: Map<String, Value> = env_vars
                .iter()
                .map(|(name, var)| (name.clone(), Value::String(format!("${{{var}}}"))))
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
    }
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
    // Every tool result passes the in-sandbox redaction of agent secrets (SPEC-002 ADR-20):
    // a result holding one is replaced by its redacted copy before the model reads it.
    hooks.insert(
        "PostToolUse".to_owned(),
        redaction_hook(&request.yulius_exe)?,
    );
    // A Bash command that waits on or kills itself (`pgrep -f`/`pkill -f` matching its own
    // `bash -c` line) is refused before it runs (complaint 1, F 2026-10-12).
    hooks.insert(
        "PreToolUse".to_owned(),
        json!([{ "matcher": "Bash", "hooks": [{
            "type": "command",
            "command": hook_command(request, "claude", "PreToolUse")?,
            "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
        }] }]),
    );
    let settings = json!({
        "hooks": hooks,
        // A session whose context fills up compacts itself instead of answering "Prompt is too
        // long" to everything (F 2026-10-12: "auto compact can be enabled"). These run settings
        // outrank the sandbox's own settings.json, where `/config` may have turned it off.
        "autoCompactEnabled": true,
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
        servers.insert(server.name.clone(), mcp_entry(server));
    }
    let mcp = json!({ "mcpServers": servers });

    let mut argv = vec![CLAUDE_PROGRAM.to_owned()];
    let (session_id, prompt, continue_interrupted) = match &request.mode {
        LaunchMode::New { session_id, prompt } => {
            if let Some(id) = session_id {
                argv.extend(["--session-id".into(), id.clone()]);
            }
            (session_id.clone(), prompt.as_deref(), false)
        }
        LaunchMode::Resume {
            session_id,
            prompt,
            continue_interrupted,
        } => {
            argv.extend(["--resume".into(), session_id.clone()]);
            // Claude Code continues the cut turn itself: nothing is sent.
            let prompt = prompt.as_deref().filter(|_| !continue_interrupted);
            (Some(session_id.clone()), prompt, *continue_interrupted)
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
    let model = request.model.as_deref().unwrap_or(DEFAULT_CLAUDE_MODEL);
    argv.extend(["--model".into(), model.to_owned()]);
    if let Some(effort) = yulius_domain::harness::claude_effort(model, request.effort.as_deref()) {
        argv.extend(["--effort".into(), effort.to_owned()]);
    }
    if request.contained {
        argv.push("--dangerously-skip-permissions".into());
    }
    push_prompt(&mut argv, prompt);
    let mut env = vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            utf8(&config_dir, "sandbox_home")?.to_owned(),
        ),
        (YULIUS_URL_ENV.into(), base_url(request).to_owned()),
        (
            DISABLE_AUTOUPDATER_ENV.0.into(),
            DISABLE_AUTOUPDATER_ENV.1.into(),
        ),
        // A cold build or a test run takes minutes: a 2-minute Bash default pushed such
        // commands into the background mid-run, and agents then polled them (F 2026-10-12).
        (BASH_TIMEOUT_ENV[0].0.into(), BASH_TIMEOUT_ENV[0].1.into()),
        (BASH_TIMEOUT_ENV[1].0.into(), BASH_TIMEOUT_ENV[1].1.into()),
        (
            TASK_WORKTREE_ENV.into(),
            utf8(&request.worktree, "worktree")?.to_owned(),
        ),
        (TMPDIR_ENV.0.into(), TMPDIR_ENV.1.into()),
    ];
    if continue_interrupted {
        env.push((
            RESUME_INTERRUPTED_TURN_ENV.0.into(),
            RESUME_INTERRUPTED_TURN_ENV.1.into(),
        ));
    }

    Ok(LaunchPlan {
        argv,
        cwd: request.worktree.clone(),
        env,
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

/// A user's Cockpit (SPEC-021): a plain Claude Code session outside any task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CockpitRequest {
    /// Working directory (the sandbox repository).
    pub cwd: PathBuf,
    /// Claude's config dir as the session sees it (in the user's own home).
    pub config_dir: PathBuf,
    /// Conversation to resume.
    pub resume: Option<String>,
    /// The resumed conversation continues the turn a usage limit cut (SPEC-074 SUP-004,
    /// SPEC-034 LIM-002): the caller has stepped its transcript back.
    pub continue_interrupted: bool,
    /// Provider credential of the pool subscription.
    pub credential: Credential,
    /// The Yulius MCP server the Cockpit manages the project with (SPEC-037); `None` for none.
    pub yulius: Option<CockpitYulius>,
    /// Every server name in the repository's `.mcp.json`. Yulius attaches them itself, so Claude
    /// Code rejects them (`disabledMcpjsonServers`) instead of asking (SPEC-095 PMC-004).
    pub project_mcp_servers: Vec<String>,
    /// The sandbox's MCP servers, as a task run of the sandbox gets them (SPEC-147 SAM-001): the
    /// caller supplies their values in the secret environment.
    pub mcp_servers: Vec<McpServerSpec>,
    /// The `yulius` executable of the `PostToolUse` redaction hook (SPEC-147 SAM-002); `None` for
    /// no hook.
    pub redaction_exe: Option<PathBuf>,
    /// Bypass-permissions mode (`--dangerously-skip-permissions`, with `IS_SANDBOX=1`: a contained
    /// Cockpit is root of its sandbox), as a task run of a contained server has (SPEC-208).
    pub bypass_permissions: bool,
}

/// The Yulius MCP server of a Cockpit (SPEC-037 MGT-004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CockpitYulius {
    /// Base URL of the internal listener (`/mcp` is appended).
    pub url: String,
    /// The Cockpit's bearer (`ck_…`), passed in the secret environment only.
    pub token: Secret<String>,
}

/// Declares the session a sandbox, so Claude Code accepts `--dangerously-skip-permissions` as root.
pub const IS_SANDBOX_ENV: (&str, &str) = ("IS_SANDBOX", "1");
/// Environment variable holding a Cockpit's Yulius MCP bearer.
pub const COCKPIT_TOKEN_ENV: &str = "YULIUS_COCKPIT_TOKEN";
/// The Yulius tools a Cockpit may call without asking: they only read (`secret_verify` writes
/// nothing but its audit entry; SPEC-178 VAU-003).
pub const COCKPIT_READ_ONLY_TOOLS: [&str; 8] = [
    "tasks_list",
    "task_get",
    "automations_list",
    "workflows_list",
    "sessions_search",
    "session_read",
    "secrets_list",
    "secret_verify",
];

/// Builds the launch plan of a Cockpit: `claude [--resume <id>] [--mcp-config <yulius and the
/// sandbox's servers>] [--allowedTools <read-only tools>] [--settings <redaction hook, rejected
/// .mcp.json servers>] --model <default> --effort <default> [--dangerously-skip-permissions]`
/// (SPEC-023 DEF-004) with the subscription's token (and the Cockpit's Yulius bearer) in the secret
/// environment and none of a task's lifecycle hooks, status line or briefing (SPEC-021 CKP-002,
/// SPEC-037 MGT-004 as amended by SPEC-147, ADR-0121). The approval bypass comes with
/// `bypass_permissions`, as a contained task run's does (SPEC-208, decision 0165). The MCP config is inline JSON naming
/// `${YULIUS_COCKPIT_TOKEN}` and the servers' `${VAR}` references, never a value; it is not
/// strict, so the person's own MCP servers keep working. The settings are the `PostToolUse`
/// redaction hook and `disabledMcpjsonServers` (SPEC-095 PMC-004).
///
/// # Errors
/// [`LaunchError`] for a non-UTF-8 config dir or executable, an unsafe session id, credential,
/// URL or token, or an MCP server a task run could not have.
pub fn plan_cockpit(request: &CockpitRequest) -> Result<LaunchPlan, LaunchError> {
    let Credential::ClaudeOauthToken(token) = &request.credential;
    check(
        !token.expose().is_empty() && !token.expose().contains(['\0', '\n', '\r']),
        "credential",
    )?;
    let mut argv = vec![CLAUDE_PROGRAM.to_owned()];
    if let Some(id) = &request.resume {
        check(valid_session_id(id), "session_id")?;
        argv.extend(["--resume".into(), id.clone()]);
    }
    validate_mcp_servers(&request.mcp_servers)?;
    let mut secret_env = vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), token.expose().clone())];
    let mut servers = Map::new();
    let mut allowed = None;
    if let Some(yulius) = &request.yulius {
        check(valid_url(&yulius.url), "yulius url")?;
        let bearer = yulius.token.expose();
        check(
            !bearer.is_empty() && bearer.chars().all(|c| c.is_ascii_graphic()),
            "yulius token",
        )?;
        servers.insert(
            YULIUS_MCP_NAME.to_owned(),
            json!({
                "headers": { "Authorization": format!("Bearer ${{{COCKPIT_TOKEN_ENV}}}") },
                "type": "http",
                "url": format!("{}/mcp", yulius.url.trim_end_matches('/')),
            }),
        );
        allowed = Some(
            COCKPIT_READ_ONLY_TOOLS
                .iter()
                .map(|tool| format!("mcp__{YULIUS_MCP_NAME}__{tool}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        secret_env.push((COCKPIT_TOKEN_ENV.into(), bearer.clone()));
    }
    // The sandbox's servers, as a task run gets them (SPEC-147 SAM-001).
    for server in &request.mcp_servers {
        servers.insert(server.name.clone(), mcp_entry(server));
    }
    if !servers.is_empty() {
        let config = json!({ "mcpServers": servers });
        argv.extend(["--mcp-config".into(), config.to_string()]);
    }
    if let Some(allowed) = allowed {
        argv.extend(["--allowedTools".into(), allowed]);
    }
    let mut settings = Map::new();
    // The task runs' redaction of agent secrets, the Cockpit's only hook (SPEC-147 SAM-002).
    if let Some(exe) = &request.redaction_exe {
        settings.insert(
            "hooks".into(),
            json!({ "PostToolUse": redaction_hook(exe)? }),
        );
    }
    // Claude Code asks about every `.mcp.json` server it has neither approved nor rejected, and
    // an explicit `enableAllProjectMcpServers: false` still asks (measured on 2.1.292, SPEC-095).
    if !request.project_mcp_servers.is_empty() {
        settings.insert(
            "disabledMcpjsonServers".into(),
            json!(request.project_mcp_servers),
        );
    }
    if !settings.is_empty() {
        argv.extend(["--settings".into(), Value::Object(settings).to_string()]);
    }
    // The person's own session still starts on Yulius's default model and effort (SPEC-023).
    argv.extend([
        "--model".into(),
        DEFAULT_CLAUDE_MODEL.into(),
        "--effort".into(),
        DEFAULT_CLAUDE_EFFORT.into(),
    ]);
    let mut env = vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            utf8(&request.config_dir, "config_dir")?.to_owned(),
        ),
        (
            DISABLE_AUTOUPDATER_ENV.0.into(),
            DISABLE_AUTOUPDATER_ENV.1.into(),
        ),
    ];
    // Bypass-permissions mode rather than Claude Code's auto mode (SPEC-208 BYP-001); as uid 0
    // Claude Code takes the flag only inside a declared sandbox.
    if request.bypass_permissions {
        argv.push("--dangerously-skip-permissions".into());
        env.push((IS_SANDBOX_ENV.0.into(), IS_SANDBOX_ENV.1.into()));
    }
    if request.continue_interrupted && request.resume.is_some() {
        env.push((
            RESUME_INTERRUPTED_TURN_ENV.0.into(),
            RESUME_INTERRUPTED_TURN_ENV.1.into(),
        ));
    }
    Ok(LaunchPlan {
        argv,
        cwd: request.cwd.clone(),
        env,
        secret_env,
        files: Vec::new(),
        session_id: request.resume.clone(),
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
