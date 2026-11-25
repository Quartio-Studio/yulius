//! Launch plans for Claude Code, Codex and Antigravity sessions (SPEC-002 §7.2, ADR-05, ADR-07;
//! SPEC-503; SPEC-505).
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

use crate::json::{toml_string, toml_string_array};
use crate::paths::valid_session_id;
use crate::secret::Secret;

/// Claude Code executable name (resolved through `PATH` by the spawner).
pub const CLAUDE_PROGRAM: &str = "claude";
/// Codex CLI executable name (resolved through `PATH` by the spawner).
pub const CODEX_PROGRAM: &str = "codex";
/// Codex's turn boundaries and the review subagent lifecycle (SPEC-503, SPEC-605).
pub const CODEX_HOOK_EVENTS: [&str; 6] = [
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
];
/// The prompt a Codex session resumed after a usage limit gets: Codex has no way to continue the
/// cut turn by itself.
pub const CODEX_CONTINUE_PROMPT: &str = "Continue from where you left off.";
/// Antigravity CLI executable name (resolved through `PATH` by the spawner; SPEC-505).
pub const ANTIGRAVITY_PROGRAM: &str = "agy";
/// Hook events registered for Antigravity (SPEC-505 AGY-006): its session start, the model call
/// that opens a turn (its `UserPromptSubmit`) and the end of a turn; `PreToolUse` apart.
pub const ANTIGRAVITY_HOOK_EVENTS: [&str; 3] = ["SessionStart", "PreInvocation", "Stop"];
/// The variable and value that keep the Antigravity CLI from updating itself (1.5.10 reads
/// `true` only).
pub const ANTIGRAVITY_NO_UPDATE_ENV: (&str, &str) =
    yulius_domain::agent_cli::antigravity::NO_UPDATE_ENV;
/// The variable an API-key run of the Antigravity CLI reads its Gemini API key from.
pub const GEMINI_API_KEY_ENV: &str = "GEMINI_API_KEY";
/// The Antigravity CLI's folder below its home.
pub const ANTIGRAVITY_DIR: &str = ".gemini";
/// Where the Antigravity CLI keeps a Google sign-in when no keyring is reachable, below its home
/// (1.5.10, seen with `strace`): `yulius_domain::agent_cli::antigravity`'s `TOKEN_DIRS` and
/// `TOKEN_NAME` (a test keeps them equal).
pub const ANTIGRAVITY_TOKEN_FILE: &str = ".gemini/antigravity-cli/antigravity-oauth-token";
pub use yulius_domain::harness::{DEFAULT_ANTIGRAVITY_EFFORT, DEFAULT_ANTIGRAVITY_MODEL};
/// The default Codex model and the fallback effort for other models.
pub use yulius_domain::harness::{DEFAULT_CODEX_EFFORT, DEFAULT_CODEX_MODEL};
/// Codex caps the timeout of its `SessionEnd` hook at 3 s (0.167).
pub const CODEX_SESSION_END_TIMEOUT_SECONDS: u64 = 3;
/// Environment variable holding the per-attempt Yulius bearer token (set by the supervisor).
pub const TASK_TOKEN_ENV: &str = yulius_domain::agent_endpoint::TOKEN_ENV;
/// Environment variable holding the internal Yulius listener URL.
pub const YULIUS_URL_ENV: &str = yulius_domain::agent_endpoint::URL_ENV;
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
/// Keeps Claude Code on its full-screen renderer in task runs and the Cockpit, which Yulius's
/// terminal needs: the wheel reaches it as mouse reports, and a late viewer gets the screen, never
/// a scrollback (SPEC-280). Tasks of a project share one config dir and are each pid 2 in their
/// own PID namespace, so Claude Code's boot canary took a sibling's start for its own failed one
/// and turned full screen off for the project after two (#898, ADR-0218).
pub const FULL_SCREEN_ENV: (&str, &str) = ("CLAUDE_CODE_NO_FLICKER", "1");
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
/// (`/exit`, Ctrl+C twice, `/clear`, a signal), before the process is gone. `SubagentStart` and
/// `SubagentStop` tell it which subagent a review round runs as and its report (SPEC-493).
pub const CLAUDE_HOOK_EVENTS: [&str; 7] = [
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
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
        /// Continue a copy of the session under a new id instead (SPEC-628 FRK-006): Claude Code
        /// `--fork-session`, Codex `fork`; the Antigravity CLI cannot.
        fork: Option<ForkSession>,
    },
}

/// A forked resume (SPEC-628): the session continues as a copy, and the original is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkSession {
    /// The new session's id where the CLI lets Yulius name it (Claude Code `--session-id`);
    /// `None` lets the CLI name it (Codex), learnt from its hook or rollout.
    pub session_id: Option<String>,
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
    /// Contents of a Codex `auth.json` (SPEC-503 CDX-005).
    CodexAuthJson(Secret<Vec<u8>>),
    /// An Antigravity credential, as [`crate::login::antigravity_api_key_credential`] or
    /// [`crate::login::antigravity_sign_in_credential`] made it (SPEC-505 AGY-008).
    Antigravity(Secret<Vec<u8>>),
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
    /// Codex home of the (sandbox, account) pair; required for Codex (SPEC-503 CDX-003).
    pub codex_home: Option<PathBuf>,
    /// The Codex CLI Yulius provides, an absolute path (SPEC-503 CLI-002); `None`: [`CODEX_PROGRAM`]
    /// from `PATH`.
    pub codex_program: Option<PathBuf>,
    /// Antigravity home of the (sandbox, account) pair, the folder holding its `.gemini`;
    /// required for Antigravity (SPEC-505 AGY-003).
    pub antigravity_home: Option<PathBuf>,
    /// The Antigravity CLI Yulius provides, an absolute path (SPEC-505 AGY-007); `None`:
    /// [`ANTIGRAVITY_PROGRAM`] from `PATH`.
    pub antigravity_program: Option<PathBuf>,
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
    /// Plugins of the sandbox home (`name@marketplace`) this run switches off whatever a settings
    /// file says: the ones that bring a language server (SPEC-485 LSP-003, ADR-0327).
    pub disabled_plugins: Vec<String>,
    /// Claude Code's auto-memory folder of the session, which Yulius keeps read-only (SPEC-652
    /// MEM-002, MEM-006): the run settings name it as `autoMemoryDirectory` and deny edits there,
    /// to `CLAUDE.md` and to `rules` in the configuration folder. `None`: neither.
    pub claude_memory_dir: Option<PathBuf>,
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
    /// The credential kind does not belong to the harness.
    #[error("credential does not match harness {0}")]
    CredentialMismatch(Harness),
    /// Codex needs a `CODEX_HOME`.
    #[error("codex launch requires codex_home")]
    MissingCodexHome,
    /// Antigravity needs its home.
    #[error("antigravity launch requires antigravity_home")]
    MissingAntigravityHome,
    /// A path that must appear in argv or config is not valid UTF-8.
    #[error("path {0} is not valid UTF-8")]
    NonUtf8Path(&'static str),
    /// A field has a value that cannot be passed safely.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// Two MCP servers share a name, or a server uses the reserved name.
    #[error("duplicate or reserved MCP server name {0:?}")]
    DuplicateMcpServer(String),
    /// The harness's CLI cannot fork a session at launch (SPEC-628 FRK-003).
    #[error("{0} cannot fork a session at launch")]
    ForkUnsupported(Harness),
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
    name.len() <= 128 && shell_identifier(name)
}

pub(crate) fn shell_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
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
            session_id,
            prompt,
            fork,
            ..
        } => {
            if let Some(id) = fork.as_ref().and_then(|f| f.session_id.as_deref()) {
                check(valid_session_id(id), "session_id")?;
            }
            (Some(session_id.as_str()), prompt.as_deref())
        }
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
        Harness::Codex => plan_codex(request),
        Harness::Antigravity => plan_antigravity(request),
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

/// Claude Code's settings key of the plugins that are on (`true`) or off (`false`).
const ENABLED_PLUGINS: &str = "enabledPlugins";

/// `enabledPlugins` with `false` for each of `plugins`, `None` for none. In a run's own settings
/// (`--settings`) it outranks the home's `settings.json`, a repository's `.claude/settings.json`
/// and a worktree's `.claude/settings.local.json`, where a `/plugin` in any terminal or the Enter
/// that answers Claude Code's install dialog switches a plugin on for every session: with `true`
/// in the home's file and `false` here no language server of the plugin is loaded (measured on
/// 2.1.323, SPEC-485 LSP-003, ADR-0327).
fn plugins_off(plugins: &[String]) -> Option<Value> {
    (!plugins.is_empty()).then(|| {
        Value::Object(
            plugins
                .iter()
                .map(|plugin| (plugin.clone(), Value::Bool(false)))
                .collect(),
        )
    })
}

/// The independent code review's agents (`--agents`, SPEC-493 REV-001): each by its full model
/// id and at the reviewer's effort, with the reviewer's contract as its prompt. Claude Code
/// 2.1.325 runs a subagent of such a definition on its `model` and `effort`, whatever the
/// session runs, and a definition given at the launch outranks one of the same name in the
/// agent's home (measured with the real CLI, SPEC-493 Context).
fn reviewer_agents() -> Value {
    use yulius_domain::review::{REVIEWER_EFFORT, REVIEWER_MODELS, REVIEWER_PROMPT};
    let mut agents = Map::new();
    for (name, model) in REVIEWER_MODELS {
        agents.insert(
            name.to_owned(),
            json!({
                "description": format!(
                    "Independent code review of a change (a diff range and its spec), read-only, \
                     on {}. Yulius picks the round's model: start every review with \
                     yulius-reviewer.",
                    yulius_domain::harness::claude_model_name(model)
                ),
                "prompt": REVIEWER_PROMPT,
                "model": model,
                "effort": REVIEWER_EFFORT,
            }),
        );
    }
    Value::Object(agents)
}

/// Run settings that keep a Claude Code session to the memory folder Yulius protects (SPEC-652
/// MEM-002, MEM-006): `autoMemoryDirectory` names it, so the session reads exactly the folder its
/// namespace binds read-only, and `permissions.deny` refuses Claude Code's own edits there and to
/// `CLAUDE.md` and `rules` in `config_dir` (an absolute path in a rule starts with `//`).
fn memory_settings(
    settings: &mut Map<String, Value>,
    memory_dir: &Path,
    config_dir: &Path,
) -> Result<(), LaunchError> {
    let memory = utf8(memory_dir, "claude_memory_dir")?;
    let config = utf8(config_dir, "config_dir")?;
    check(
        memory_dir.is_absolute() && config_dir.is_absolute(),
        "claude_memory_dir",
    )?;
    settings.insert("autoMemoryDirectory".into(), json!(memory));
    settings.insert(
        "permissions".into(),
        json!({ "deny": [
            format!("Edit(/{memory}/**)"),
            format!("Edit(/{config}/CLAUDE.md)"),
            format!("Edit(/{config}/rules/**)"),
        ] }),
    );
    Ok(())
}

fn plan_claude(request: &LaunchRequest) -> Result<LaunchPlan, LaunchError> {
    let Credential::ClaudeOauthToken(token) = &request.credential else {
        return Err(LaunchError::CredentialMismatch(request.harness));
    };
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
    if request.contained {
        hooks.insert(
            "PermissionRequest".to_owned(),
            json!([{ "matcher": "Bash", "hooks": [{
                "type": "command",
                "command": hook_command(request, "claude", "PermissionRequest")?,
                "timeout": CLAUDE_HOOK_TIMEOUT_SECONDS,
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
    // `bash -c` line) is refused before it runs (complaint 1, F 2026-10-12); an edit of the
    // task's code waits for its spec's Plan, decided by Yulius (SPEC-426 EDG-001).
    let pre_tool_use = hook_command(request, "claude", "PreToolUse")?;
    hooks.insert(
        "PreToolUse".to_owned(),
        json!([
            { "matcher": "Bash", "hooks": [{
                "type": "command",
                "command": pre_tool_use,
                "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
            }] },
            { "matcher": crate::guard_client::EDIT_TOOLS.map(|(tool, _)| tool).join("|"), "hooks": [{
                "type": "command",
                "command": pre_tool_use,
                "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
            }] },
            // A subagent's start: Yulius picks a review round's reviewer (SPEC-493 REV-002).
            { "matcher": crate::guard_client::AGENT_TOOLS.join("|"), "hooks": [{
                "type": "command",
                "command": pre_tool_use,
                "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
            }] }
        ]),
    );
    let mut settings = json!({
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
    if let (Some(off), Some(settings)) = (
        plugins_off(&request.disabled_plugins),
        settings.as_object_mut(),
    ) {
        settings.insert(ENABLED_PLUGINS.into(), off);
    }
    if let (Some(memory), Some(settings)) = (&request.claude_memory_dir, settings.as_object_mut()) {
        memory_settings(settings, memory, &config_dir)?;
    }

    // Yulius's own tools go through `yulius mcp`, a stdio bridge to `${YULIUS_URL}/mcp` that
    // lives as long as the session: Claude Code gives up on an HTTP server after a minute of
    // failed reconnects, so a Yulius restart would cut the session off until `/mcp`
    // (SPEC-358 MSB-001, ADR-0244).
    let mut servers = Map::new();
    servers.insert(
        YULIUS_MCP_NAME.to_owned(),
        json!({
            "type": "stdio",
            "command": utf8(&request.yulius_exe, "yulius_exe")?,
            "args": ["mcp"],
            "env": {
                YULIUS_URL_ENV: format!("${{{YULIUS_URL_ENV}}}"),
                TASK_TOKEN_ENV: format!("${{{TASK_TOKEN_ENV}}}"),
            }
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
            fork: None,
        } => {
            argv.extend(["--resume".into(), session_id.clone()]);
            // Claude Code continues the cut turn itself: nothing is sent.
            let prompt = prompt.as_deref().filter(|_| !continue_interrupted);
            (Some(session_id.clone()), prompt, *continue_interrupted)
        }
        // A fork continues a copy of the session under the id Yulius names, so the lineage holds
        // the fork's session from the spawn (SPEC-628 FRK-006; `--session-id` with `--resume`
        // needs `--fork-session`, Claude Code 2.1.330).
        LaunchMode::Resume {
            session_id,
            prompt,
            fork: Some(fork),
            ..
        } => {
            argv.extend([
                "--resume".into(),
                session_id.clone(),
                "--fork-session".into(),
            ]);
            if let Some(id) = &fork.session_id {
                argv.extend(["--session-id".into(), id.clone()]);
            }
            (fork.session_id.clone(), prompt.as_deref(), false)
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
        // Yulius's reviewer agents, defined here out of the agent's reach (its home's agents
        // folder is its own): their model and effort are this launch's (SPEC-493 REV-001).
        "--agents".into(),
        reviewer_agents().to_string(),
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
        (FULL_SCREEN_ENV.0.into(), FULL_SCREEN_ENV.1.into()),
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

/// The `config.toml` Yulius writes into a Codex home before each run: credentials stay in its
/// `auth.json` (the vault's copy, written back after the run), and Codex neither checks for
/// updates (the operator installs it, #45), asks to trust the worktree or offers to switch models
/// near the rate limit (SPEC-603). The agent's shell keeps
/// the run's environment (Codex's default) except `CODEX_HOME`: a `codex` the agent runs there
/// uses its own home, never this one with Yulius's hooks (SPEC-281 SSN-002).
fn codex_config(worktree: &str, pro_500: bool) -> Vec<u8> {
    let speed = if pro_500 {
        "service_tier = \"ultrafast\"\n"
    } else {
        ""
    };
    format!(
        "cli_auth_credentials_store = \"file\"\ncheck_for_update_on_startup = false\n{speed}\n\
         [notice]\nhide_rate_limit_model_nudge = true\n\n\
         [shell_environment_policy]\nignore_default_excludes = true\nexclude = [\"CODEX_HOME\"]\n\n\
         [projects.{}]\ntrust_level = \"trusted\"\n",
        toml_string(worktree)
    )
    .into_bytes()
}

/// The command, arguments and passed variables of a stdio MCP server whose variables are renamed
/// (`env_vars`: the server's name, the run variable holding its value): `sh -c` exports each from
/// its run variable and execs the server, its command and arguments being the shell's positional
/// arguments, never part of its script. Names are checked environment variable names.
fn codex_stdio_wrapper(
    server: &str,
    command: &str,
    args: &[String],
    env_vars: &[(String, String)],
) -> (String, Vec<String>, Vec<String>) {
    let exports: Vec<String> = env_vars
        .iter()
        .map(|(name, var)| format!("{name}=\"${var}\""))
        .collect();
    let script = format!("export {}; exec \"$@\"", exports.join(" "));
    let mut shell_args = vec![
        "-c".to_owned(),
        script,
        format!("mcp-{server}"),
        command.to_owned(),
    ];
    shell_args.extend(args.iter().cloned());
    let vars = env_vars.iter().map(|(_, var)| var.clone()).collect();
    ("sh".to_owned(), shell_args, vars)
}

fn plan_codex(request: &LaunchRequest) -> Result<LaunchPlan, LaunchError> {
    let Credential::CodexAuthJson(auth) = &request.credential else {
        return Err(LaunchError::CredentialMismatch(request.harness));
    };
    check(!auth.expose().is_empty(), "credential")?;
    let codex_home = request
        .codex_home
        .as_ref()
        .ok_or(LaunchError::MissingCodexHome)?;
    let worktree = utf8(&request.worktree, "worktree")?;
    let program = match &request.codex_program {
        Some(path) => {
            check(path.is_absolute(), "codex_program")?;
            utf8(path, "codex_program")?.to_owned()
        }
        None => CODEX_PROGRAM.to_owned(),
    };

    // The same hook client as Claude Code's (`yulius hook codex <event>`); the turn boundaries may
    // be held while no subscription has capacity (ADR-17).
    // Codex waits for its Stop and UserPromptSubmit hooks with no upper bound on `timeout`, and
    // ignores a hook that fails or times out (0.167).
    let mut hooks = Map::new();
    for event in CODEX_HOOK_EVENTS {
        let timeout = if CLAUDE_HELD_HOOK_EVENTS.contains(&event) {
            CLAUDE_HELD_HOOK_TIMEOUT_SECONDS
        } else if event == "SessionEnd" {
            CODEX_SESSION_END_TIMEOUT_SECONDS
        } else {
            CLAUDE_HOOK_TIMEOUT_SECONDS
        };
        hooks.insert(
            event.to_owned(),
            json!([{ "hooks": [{
                "type": "command",
                "command": hook_command(request, "codex", event)?,
                "timeout": timeout
            }] }]),
        );
    }
    // A shell command that waits on or kills itself is refused before it runs, as for Claude Code;
    // Codex names its shell tool `Bash` with the command in `tool_input.command` (0.167). Its file
    // edits are `apply_patch`, which Yulius's edit gate does not read.
    hooks.insert(
        "PreToolUse".to_owned(),
        json!([{ "matcher": "^Bash$", "hooks": [{
            "type": "command",
            "command": hook_command(request, "codex", "PreToolUse")?,
            "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
        }] }]),
    );
    let hooks_json = json!({ "hooks": hooks });

    // `--no-daemon`: the session stays inside the process Yulius spawned and supervises; Codex
    // otherwise may run it in a shared background server (0.167 `daemon_auto_start`).
    let mut argv = vec![
        program,
        "--no-daemon".into(),
        "-C".into(),
        worktree.to_owned(),
    ];
    let mut config = |key: &str, value: String| {
        argv.extend(["-c".into(), format!("{key}={value}")]);
    };
    // Yulius's own tools through the `yulius mcp` stdio bridge, as Claude Code gets them
    // (SPEC-358): its URL and the run's token come from the run environment by name.
    let yulius_key = format!("mcp_servers.{YULIUS_MCP_NAME}");
    config(
        &format!("{yulius_key}.command"),
        toml_string(utf8(&request.yulius_exe, "yulius_exe")?),
    );
    config(&format!("{yulius_key}.args"), toml_string_array(&["mcp"]));
    config(
        &format!("{yulius_key}.env_vars"),
        toml_string_array(&[YULIUS_URL_ENV, TASK_TOKEN_ENV]),
    );
    for server in &request.mcp_servers {
        let key = format!("mcp_servers.{}", server.name);
        match &server.transport {
            McpTransport::Stdio {
                command,
                args,
                env_vars,
            } if env_vars.is_empty() => {
                config(&format!("{key}.command"), toml_string(command));
                config(&format!("{key}.args"), toml_string_array(args));
            }
            // Codex passes a server only variables of its own name: a shell gives each one the
            // run variable holding its value. Names are checked, and the command and its
            // arguments are the shell's arguments, never its script.
            McpTransport::Stdio {
                command,
                args,
                env_vars,
            } => {
                let (shell, shell_args, vars) =
                    codex_stdio_wrapper(&server.name, command, args, env_vars);
                config(&format!("{key}.command"), toml_string(&shell));
                config(&format!("{key}.args"), toml_string_array(&shell_args));
                config(&format!("{key}.env_vars"), toml_string_array(&vars));
            }
            McpTransport::Http { url, bearer_env } => {
                config(&format!("{key}.url"), toml_string(url));
                if let Some(name) = bearer_env {
                    config(&format!("{key}.bearer_token_env_var"), toml_string(name));
                }
            }
        }
    }
    // The model and effort on every launch: without them Codex runs the account's default model
    // at its own default effort (SPEC-511 CXD-001).
    let model = request.model.as_deref().unwrap_or(DEFAULT_CODEX_MODEL);
    let effort = yulius_domain::harness::codex_effort(request.effort.as_deref());
    config("model_reasoning_effort", toml_string(effort));
    argv.extend(["-m".into(), model.to_owned()]);
    if request.contained {
        argv.push("--dangerously-bypass-approvals-and-sandbox".into());
    }
    // Yulius writes the hooks itself, before every run: they need no trust a person grants.
    argv.push("--dangerously-bypass-hook-trust".into());
    let (session_id, prompt) = match &request.mode {
        LaunchMode::New { prompt, .. } => (None, prompt.clone()),
        // `codex fork <id>` writes a new rollout under an id of its own (SPEC-628 FRK-006).
        LaunchMode::Resume {
            session_id,
            prompt,
            fork: Some(_),
            ..
        } => {
            argv.extend(["fork".into(), session_id.clone()]);
            (None, prompt.clone())
        }
        LaunchMode::Resume {
            session_id,
            prompt,
            continue_interrupted,
            fork: None,
        } => {
            argv.extend(["resume".into(), session_id.clone()]);
            let prompt = if *continue_interrupted {
                Some(CODEX_CONTINUE_PROMPT.to_owned())
            } else {
                prompt.clone()
            };
            (Some(session_id.clone()), prompt)
        }
    };
    // Always after `--`: a prompt that is one of Codex's subcommand words (`completion`,
    // `review`, …) would run that subcommand (0.169.0).
    if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
        argv.extend(["--".into(), prompt]);
    }

    Ok(LaunchPlan {
        argv,
        cwd: request.worktree.clone(),
        env: vec![
            (
                "CODEX_HOME".into(),
                utf8(codex_home, "codex_home")?.to_owned(),
            ),
            (YULIUS_URL_ENV.into(), base_url(request).to_owned()),
            (TASK_WORKTREE_ENV.into(), worktree.to_owned()),
            (TMPDIR_ENV.0.into(), TMPDIR_ENV.1.into()),
        ],
        secret_env: Vec::new(),
        files: vec![
            PlannedFile {
                path: codex_home.join("auth.json"),
                contents: auth.expose().clone(),
                mode: PRIVATE_FILE_MODE,
            },
            PlannedFile {
                path: codex_home.join("config.toml"),
                contents: codex_config(worktree, crate::login::codex_pro_500(auth.expose())),
                mode: PRIVATE_FILE_MODE,
            },
            PlannedFile {
                path: codex_home.join("hooks.json"),
                contents: pretty(&hooks_json),
                mode: PRIVATE_FILE_MODE,
            },
        ],
        // Codex names its own session (no `--session-id`): a new one is learnt from its hook or
        // its rollout.
        session_id,
    })
}

/// The `settings.json` of an Antigravity home: the worktree trusted and the first-run screens
/// (colour scheme, tips, survey) left out, so the session starts at its prompt; an API key's
/// provider (SPEC-505 AGY-002). The CLI rewrites the file at its start; Yulius writes it again
/// before each run.
fn antigravity_settings(worktree: &str, api_key: bool) -> Value {
    let mut settings = Map::new();
    if api_key {
        settings.insert("modelProvider".into(), json!("gemini"));
    }
    settings.insert("colorScheme".into(), json!("terminal"));
    settings.insert("showTips".into(), json!(false));
    settings.insert("showFeedbackSurvey".into(), json!(false));
    settings.insert("notifications".into(), json!(false));
    settings.insert("trustedWorkspaces".into(), json!([worktree]));
    Value::Object(settings)
}

/// A sandbox MCP server in the Antigravity CLI's `mcp_config.json`: a stdio server inherits the
/// run's environment, so one with renamed variables is started through the same `sh` wrapper as
/// Codex's; an HTTP server that needs a bearer token is left out, since the CLI expands no
/// variable in a header (1.5.10) and its value would be written to the file.
fn antigravity_mcp_entry(server: &McpServerSpec) -> Option<Value> {
    match &server.transport {
        McpTransport::Stdio {
            command,
            args,
            env_vars,
        } if env_vars.is_empty() => Some(json!({ "command": command, "args": args })),
        McpTransport::Stdio {
            command,
            args,
            env_vars,
        } => {
            let (shell, shell_args, _) = codex_stdio_wrapper(&server.name, command, args, env_vars);
            Some(json!({ "command": shell, "args": shell_args }))
        }
        McpTransport::Http {
            url,
            bearer_env: None,
        } => Some(json!({ "url": url })),
        McpTransport::Http { .. } => None,
    }
}

fn plan_antigravity(request: &LaunchRequest) -> Result<LaunchPlan, LaunchError> {
    use crate::login::{AntigravityCredential, parse_antigravity_credential};
    let Credential::Antigravity(envelope) = &request.credential else {
        return Err(LaunchError::CredentialMismatch(request.harness));
    };
    let credential = parse_antigravity_credential(envelope.expose())
        .ok_or(LaunchError::Invalid("credential"))?;
    let home = request
        .antigravity_home
        .as_ref()
        .ok_or(LaunchError::MissingAntigravityHome)?;
    let home_text = utf8(home, "antigravity_home")?;
    let worktree = utf8(&request.worktree, "worktree")?;
    let program = match &request.antigravity_program {
        Some(path) => {
            check(path.is_absolute(), "antigravity_program")?;
            utf8(path, "antigravity_program")?.to_owned()
        }
        None => ANTIGRAVITY_PROGRAM.to_owned(),
    };

    // The same hook client as Claude Code's (`yulius hook antigravity <event>`), which translates
    // the CLI's payloads and answers; the turn boundaries may be held while no subscription has
    // capacity (ADR-17). The CLI runs each through `sh -c` with its own environment.
    let mut hooks = Map::new();
    for event in ANTIGRAVITY_HOOK_EVENTS {
        let timeout = if matches!(event, "PreInvocation" | "Stop") {
            CLAUDE_HELD_HOOK_TIMEOUT_SECONDS
        } else {
            CLAUDE_HOOK_TIMEOUT_SECONDS
        };
        hooks.insert(
            event.to_owned(),
            json!([{
                "type": "command",
                "command": hook_command(request, "antigravity", event)?,
                "timeout": timeout
            }]),
        );
    }
    // Its shell tool is `run_command` with the command in `CommandLine` (1.5.10).
    hooks.insert(
        "PreToolUse".to_owned(),
        json!([{ "matcher": "^run_command$", "hooks": [{
            "type": "command",
            // agy denies a tool whose hook command fails (exit 127 when the client is
            // missing, 1.5.10): one that cannot run lets the agent work, as the client does.
            "command": format!(
                "{} || printf '{{\"decision\":\"allow\"}}'",
                hook_command(request, "antigravity", "PreToolUse")?
            ),
            "timeout": CLAUDE_TOOL_HOOK_TIMEOUT_SECONDS
        }] }]),
    );
    let hooks_json = json!({ YULIUS_MCP_NAME: hooks });

    // Yulius's own tools through the `yulius mcp` stdio bridge, which reads its URL and the
    // run's token from the environment the CLI passes on.
    let mut servers = Map::new();
    servers.insert(
        YULIUS_MCP_NAME.to_owned(),
        json!({ "command": utf8(&request.yulius_exe, "yulius_exe")?, "args": ["mcp"] }),
    );
    for server in &request.mcp_servers {
        if let Some(entry) = antigravity_mcp_entry(server) {
            servers.insert(server.name.clone(), entry);
        }
    }
    let mcp_json = json!({ "mcpServers": servers });

    // The model on every launch: without it the CLI runs the account's saved one (1.5.10).
    let model = request
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_MODEL.to_owned());
    let effort = request
        .effort
        .clone()
        .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_EFFORT.to_owned());
    let mut argv = vec![program, "--model".into(), model, "--effort".into(), effort];
    if request.contained {
        argv.push("--dangerously-skip-permissions".into());
    }
    let (session_id, prompt) = match &request.mode {
        LaunchMode::New { prompt, .. } => (None, prompt.clone()),
        // Its `/fork` works only inside a running session (1.5.12): no launch forks.
        LaunchMode::Resume { fork: Some(_), .. } => {
            return Err(LaunchError::ForkUnsupported(Harness::Antigravity));
        }
        LaunchMode::Resume {
            session_id,
            prompt,
            continue_interrupted,
            fork: None,
        } => {
            argv.extend(["--conversation".into(), session_id.clone()]);
            let prompt = if *continue_interrupted {
                Some(CODEX_CONTINUE_PROMPT.to_owned())
            } else {
                prompt.clone()
            };
            (Some(session_id.clone()), prompt)
        }
    };
    // One argument with `=`: a prompt starting with `-` is never read as an option.
    if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
        argv.push(format!("--prompt-interactive={prompt}"));
    }

    let mut env = vec![
        (
            ANTIGRAVITY_NO_UPDATE_ENV.0.into(),
            ANTIGRAVITY_NO_UPDATE_ENV.1.into(),
        ),
        (YULIUS_URL_ENV.into(), base_url(request).to_owned()),
        (TASK_WORKTREE_ENV.into(), worktree.to_owned()),
        (TMPDIR_ENV.0.into(), TMPDIR_ENV.1.into()),
    ];
    // The CLI reads only `$HOME/.gemini`. A contained run binds the home's `.gemini` over the
    // sandbox home's, so the agent's shell keeps the sandbox's HOME; an uncontained one has no
    // such bind (SPEC-505 AGY-003).
    if !request.contained {
        env.push(("HOME".into(), home_text.to_owned()));
    }
    let mut secret_env = Vec::new();
    let gemini = home.join(ANTIGRAVITY_DIR);
    let mut files = vec![
        PlannedFile {
            path: gemini.join("antigravity-cli/settings.json"),
            contents: pretty(&antigravity_settings(
                worktree,
                matches!(credential, AntigravityCredential::ApiKey(_)),
            )),
            mode: PRIVATE_FILE_MODE,
        },
        PlannedFile {
            path: gemini.join("antigravity-cli/cache/onboarding.json"),
            contents: pretty(&json!({
                "consumerOnboardingComplete": true,
                "enterpriseOnboardingComplete": true,
                "onboardingComplete": true,
            })),
            mode: PRIVATE_FILE_MODE,
        },
        PlannedFile {
            path: gemini.join("config/hooks.json"),
            contents: pretty(&hooks_json),
            mode: PRIVATE_FILE_MODE,
        },
        PlannedFile {
            path: gemini.join("config/mcp_config.json"),
            contents: pretty(&mcp_json),
            mode: PRIVATE_FILE_MODE,
        },
    ];
    match credential {
        AntigravityCredential::ApiKey(key) => {
            secret_env.push((GEMINI_API_KEY_ENV.to_owned(), key));
        }
        AntigravityCredential::SignIn(token) => files.push(PlannedFile {
            path: home.join(ANTIGRAVITY_TOKEN_FILE),
            contents: token,
            mode: PRIVATE_FILE_MODE,
        }),
    }
    Ok(LaunchPlan {
        argv,
        cwd: request.worktree.clone(),
        env,
        secret_env,
        files,
        // The CLI names its own conversation: a new one is learnt from its hook or its cache.
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
    /// The model it is launched with instead of the default one: the fallback while no
    /// subscription can run the default (SPEC-482 FBK-011). `None` for the default.
    pub model: Option<String>,
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
    /// Only the Yulius MCP and no built-in tools (`--strict-mcp-config --tools ""`), with these of
    /// its tools pre-allowed instead of the Cockpit's read tools: the Global Superagent (SPEC-427
    /// GSA-002). `None` for a sandbox's Cockpit.
    pub only_yulius: Option<Vec<String>>,
    /// Plugins of the config dir (`name@marketplace`) this session switches off whatever a
    /// settings file says: the ones that bring a language server (SPEC-485 LSP-003, ADR-0327).
    pub disabled_plugins: Vec<String>,
    /// Claude Code's auto-memory folder of the session, which Yulius keeps read-only (SPEC-652
    /// MEM-002, MEM-006): the run settings name it as `autoMemoryDirectory` and deny edits there,
    /// to `CLAUDE.md` and to `rules` in the configuration folder. `None`: neither.
    pub claude_memory_dir: Option<PathBuf>,
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
/// nothing but its audit entry, SPEC-178 VAU-003; `task_delivery` is the task page's reading,
/// which may store the delivery stage it finds on GitHub, as opening that page does).
pub const COCKPIT_READ_ONLY_TOOLS: [&str; 30] = [
    "tasks_list",
    "task_get",
    "automations_list",
    "workflows_list",
    "sessions_search",
    "session_read",
    "secrets_list",
    "commands_list",
    "watches_list",
    "secret_verify",
    "sandboxes_list",
    "sandbox_task_get",
    "sandbox_tasks_list",
    "sandbox_requests",
    "superagent_checks",
    "browser_tabs",
    "browser_snapshot",
    "browser_screenshot",
    // The reading ones of the person's project tools (SPEC-487).
    "task_delivery",
    "merge_queue",
    "subscriptions",
    "instance",
    "notices_list",
    "ci_runs",
    "ci_run",
    "cd_status",
    "variables_list",
    "task_screen",
    "task_usage",
    "checkout",
];

/// Builds the launch plan of a Cockpit: `claude [--resume <id>] [--mcp-config <yulius and the
/// sandbox's servers>] [--allowedTools <read-only tools>] [--settings <redaction hook, rejected
/// .mcp.json servers>] --model <default> --effort <default> [--dangerously-skip-permissions]`
/// (SPEC-023 DEF-004) with the subscription's token (and the Cockpit's Yulius bearer) in the secret
/// environment and none of a task's lifecycle hooks, status line or briefing (SPEC-021 CKP-002,
/// SPEC-037 MGT-004 as amended by SPEC-147, ADR-0121). The approval bypass comes with
/// `bypass_permissions`, as a contained task run's does (SPEC-208, decision 0165). The MCP config
/// is inline JSON naming `${YULIUS_COCKPIT_TOKEN}` and the servers' `${VAR}` references, never a
/// value; it is not strict, so the person's own MCP servers keep working. The settings are the
/// `PostToolUse` redaction hook and `disabledMcpjsonServers` (SPEC-095 PMC-004).
///
/// # Errors
/// [`LaunchError`] for a non-UTF-8 config dir or executable, an unsafe session id, credential,
/// URL or token, or an MCP server a task run could not have.
pub fn plan_cockpit(request: &CockpitRequest) -> Result<LaunchPlan, LaunchError> {
    let Credential::ClaudeOauthToken(token) = &request.credential else {
        return Err(LaunchError::CredentialMismatch(Harness::ClaudeCode));
    };
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
        let read_only = request.only_yulius.as_ref().map_or_else(
            || {
                COCKPIT_READ_ONLY_TOOLS
                    .iter()
                    .map(|t| (*t).to_owned())
                    .collect()
            },
            Clone::clone,
        );
        allowed = Some(
            read_only
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
    // It reads and changes nothing but through Yulius (SPEC-427 GSA-002): no hook or status line
    // runs, and no settings file is read (hooks, plugins and permissions a person could add
    // there), so its Concise style comes from here.
    if request.only_yulius.is_some() {
        settings.insert("disableAllHooks".into(), json!(true));
        settings.insert("outputStyle".into(), json!("Concise"));
        // Claude Code keeps the accepted Bypass Permissions warning in the user settings file,
        // which it does not read here, so the warning would wait at every start (SPEC-430
        // GKY-004); flag settings are read whatever the setting sources (2.1.311).
        if request.bypass_permissions {
            settings.insert("skipDangerousModePermissionPrompt".into(), json!(true));
        }
    }
    if let Some(off) = plugins_off(&request.disabled_plugins) {
        settings.insert(ENABLED_PLUGINS.into(), off);
    }
    if let Some(memory) = &request.claude_memory_dir {
        memory_settings(&mut settings, memory, &request.config_dir)?;
    }
    if !settings.is_empty() {
        argv.extend(["--settings".into(), Value::Object(settings).to_string()]);
    }
    if request.only_yulius.is_some() {
        argv.extend([
            "--setting-sources".into(),
            String::new(),
            "--strict-mcp-config".into(),
            "--tools".into(),
            String::new(),
        ]);
    }
    // The person's own session still starts on Yulius's default model and effort (SPEC-023), or
    // on the fallback while no subscription can run the default (SPEC-482).
    let model = request.model.as_deref().unwrap_or(DEFAULT_CLAUDE_MODEL);
    check(valid_model(model), "model")?;
    argv.extend(["--model".into(), model.to_owned()]);
    if let Some(effort) = yulius_domain::harness::claude_effort(model, None) {
        argv.extend(["--effort".into(), effort.to_owned()]);
    }
    let mut env = vec![
        (
            "CLAUDE_CONFIG_DIR".into(),
            utf8(&request.config_dir, "config_dir")?.to_owned(),
        ),
        (
            DISABLE_AUTOUPDATER_ENV.0.into(),
            DISABLE_AUTOUPDATER_ENV.1.into(),
        ),
        (FULL_SCREEN_ENV.0.into(), FULL_SCREEN_ENV.1.into()),
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

    /// SPEC-505: the token file the plan writes is where the server reads it back.
    #[test]
    fn the_antigravity_token_file_is_the_domains() {
        use yulius_domain::agent_cli::antigravity;
        let parts = [
            antigravity::TOKEN_DIRS[0],
            antigravity::TOKEN_DIRS[1],
            antigravity::TOKEN_NAME,
        ];
        assert_eq!(ANTIGRAVITY_TOKEN_FILE, parts.join("/"));
        assert_eq!(ANTIGRAVITY_DIR, antigravity::TOKEN_DIRS[0]);
    }

    /// SPEC-503 CDX-002: the wrapper really gives the server its variable under the server's
    /// name, from the run variable, and passes its arguments untouched.
    #[test]
    fn the_codex_stdio_wrapper_renames_variables_for_the_server()
    -> Result<(), Box<dyn std::error::Error>> {
        let (shell, args, vars) = codex_stdio_wrapper(
            "apify",
            "sh",
            &[
                "-c".into(),
                "printf '%s|%s' \"$APIFY_TOKEN\" \"$1\"".into(),
                "x".into(),
                "a b; $(id)".into(),
            ],
            &[("APIFY_TOKEN".into(), "YULIUS_MCP_APIFY_APIFY_TOKEN".into())],
        );
        assert_eq!(vars, ["YULIUS_MCP_APIFY_APIFY_TOKEN"]);
        let out = std::process::Command::new(shell)
            .args(&args)
            .env_clear()
            .env("YULIUS_MCP_APIFY_APIFY_TOKEN", "synthetic-value")
            .output()?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8(out.stdout)?, "synthetic-value|a b; $(id)");
        Ok(())
    }

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
