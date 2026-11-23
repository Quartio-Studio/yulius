//! Harness port: launch planning, transcript observation and login helpers for Claude Code
//! (SPEC-002 §7.2) and Codex (SPEC-503). Implemented over `yulius-harness`.

use std::path::{Path, PathBuf};

use yulius_domain::harness::Harness;
use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, LimitScope, ModelTokens, TokenUsage};

use crate::error::AppResult;
use crate::model::Secret;
use crate::ports::session::LaunchPlan;

/// New session or native resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMode {
    /// New session.
    New {
        /// Session id to assign.
        session_id: Option<String>,
        /// Initial prompt.
        prompt: Option<String>,
    },
    /// Resume an existing provider session.
    Resume {
        /// Provider session id.
        session_id: String,
        /// Continuation prompt.
        prompt: Option<String>,
        /// The run continues the turn a usage limit cut (SPEC-034): the transcript steps back
        /// over the limit and the agent redoes the lost turn by itself; no prompt is sent.
        continue_interrupted: bool,
        /// Continue a copy of the session under a new id instead: a forked task's first launch
        /// (SPEC-628 FRK-006). The original session is never written.
        fork: Option<SessionFork>,
    },
}

/// A forked resume (SPEC-628 FRK-006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFork {
    /// The new session's id where the harness lets Yulius name it (Claude Code); `None`: the CLI
    /// names it (Codex) and Yulius learns it from the hook or rollout.
    pub session_id: Option<String>,
}

/// An MCP server to attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAttachment {
    /// stdio server; values of `env` are supplied in the child environment.
    Stdio {
        /// Name.
        name: String,
        /// Executable.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// Environment (secret values).
        env: Vec<(String, String)>,
    },
    /// Streamable HTTP server with an optional bearer token.
    Http {
        /// Name.
        name: String,
        /// URL.
        url: String,
        /// Bearer token (secret).
        bearer: Option<String>,
    },
}

/// One launch request.
#[derive(Debug, Clone)]
pub struct HarnessLaunch {
    /// Harness.
    pub harness: Harness,
    /// Mode.
    pub mode: SessionMode,
    /// Worktree.
    pub worktree: PathBuf,
    /// Sandbox home.
    pub sandbox_home: PathBuf,
    /// Run directory.
    pub run_dir: PathBuf,
    /// Model.
    pub model: Option<String>,
    /// Effort.
    pub effort: Option<String>,
    /// Contained run.
    pub contained: bool,
    /// Extra MCP servers.
    pub mcp: Vec<McpAttachment>,
    /// Provider credential (the Claude OAuth token, or the Codex `auth.json` text).
    pub credential: Secret,
    /// Codex home of the (sandbox, account) pair; required for Codex (SPEC-503 CDX-003).
    pub codex_home: Option<PathBuf>,
    /// The Codex CLI Yulius provides, started by its absolute path (SPEC-503 CLI-002); `None`:
    /// `codex` from `PATH`.
    pub codex_program: Option<PathBuf>,
    /// Antigravity home of the (sandbox, account) pair, the folder holding its `.gemini` (SPEC-505
    /// AGY-003); `None` for other harnesses.
    pub antigravity_home: Option<PathBuf>,
    /// The Antigravity CLI Yulius provides, an absolute path (SPEC-505 AGY-007); `None`: `agy`
    /// from `PATH`.
    pub antigravity_program: Option<PathBuf>,
}

/// A user's Cockpit launch (SPEC-021): plain Claude Code outside any task.
#[derive(Debug, Clone)]
pub struct CockpitLaunch {
    /// Working directory (the sandbox repository).
    pub cwd: PathBuf,
    /// The sandbox directory on the host: the folder above `cwd` that the agent cannot replace,
    /// from which the launch preparation reaches `cwd` without following a link (#801).
    pub sandbox_dir: PathBuf,
    /// Claude's config dir as the session sees it.
    pub config_dir: PathBuf,
    /// The same directory on the host, prepared before the launch.
    pub config_dir_host: PathBuf,
    /// Conversation to resume.
    pub resume: Option<String>,
    /// The resumed conversation continues the turn a usage limit cut (SPEC-074 SUP-004): its
    /// transcript steps back over the limit and Claude Code redoes the turn by itself.
    pub continue_interrupted: bool,
    /// The model it is launched with instead of the default one: the fallback while no
    /// subscription can run the default (SPEC-482 FBK-011). `None` for the default.
    pub model: Option<String>,
    /// Provider credential (the Claude OAuth token).
    pub credential: Secret,
    /// The Yulius MCP server it manages the project with (SPEC-037): the internal listener's
    /// URL and the Cockpit's bearer.
    pub yulius: Option<(String, Secret)>,
    /// Every server name in the repository's `.mcp.json`: Yulius attaches them itself, so Claude
    /// Code rejects them instead of asking (SPEC-095 PMC-004).
    pub project_mcp_servers: Vec<String>,
    /// The sandbox's MCP servers, as a task run of the sandbox gets them (SPEC-147 SAM-001); the
    /// adapter puts their values in the secret environment.
    pub mcp: Vec<McpAttachment>,
    /// A contained server: the session bypasses approvals, as a task run there does (SPEC-208).
    pub contained: bool,
    /// The Global Superagent (SPEC-427 GSA-002): only the Yulius MCP, no built-in tools and no
    /// redaction hook, with these of its tools pre-allowed. `None` for a sandbox's Cockpit.
    pub only_yulius: Option<Vec<String>>,
}

/// A launch plan plus the session id known up front.
#[derive(Debug, Clone)]
pub struct HarnessPlan {
    /// Process plan (containment filled in by the supervisor).
    pub plan: LaunchPlan,
    /// Session id, when known before the run starts.
    pub session_id: Option<String>,
}

/// Normalized transcript facts.
#[derive(Debug, Clone, PartialEq)]
pub enum Observation {
    /// Session identity (and working directory) seen.
    Session {
        /// Session id.
        session_id: String,
        /// Working directory.
        cwd: Option<String>,
    },
    /// A user turn started.
    UserTurn {
        /// When the prompt was recorded, if known.
        at: Option<Timestamp>,
        /// Claude Code woke the session itself (a task notification), nobody typed it.
        wake: bool,
    },
    /// The person interrupted the turn (Esc / Ctrl+C); the agent waits for input.
    TurnInterrupted {
        /// When it was recorded, if known.
        at: Option<Timestamp>,
    },
    /// A turn completed.
    TurnComplete {
        /// When the session's own turn ended, if known (`None` for a sub-agent's turn).
        at: Option<Timestamp>,
    },
    /// Claude Code recorded the session goal's state (TSK-014), at that time if known.
    Goal(GoalOutcome, Option<Timestamp>),
    /// Claude Code closed a turn after its Stop hooks (only used to reduce a replay).
    TurnEnded {
        /// Background agents Claude Code still waits for; `None` when it does not say.
        background_agents: Option<u32>,
        /// When it was recorded, if known: a record read late belongs to the turn before a newer
        /// prompt (SPEC-290 GDW-001).
        at: Option<Timestamp>,
    },
    /// The agent works: an assistant line that is not a turn's end.
    AgentWork {
        /// When it was recorded, if known.
        at: Option<Timestamp>,
    },
    /// The session started background work it is woken by: a Monitor stream or a background
    /// agent (SPEC-079 BGW-002).
    BackgroundStarted {
        /// Claude Code's task id.
        id: String,
        /// A background agent; otherwise a Monitor stream.
        agent: bool,
        /// How long a Monitor lives at most; `None` for an agent.
        expires_in_ms: Option<u64>,
        /// When it was recorded, if known.
        at: Option<Timestamp>,
    },
    /// A usage limit refused an agent of one of the session's dynamic workflows (SPEC-486
    /// WFL-002): Claude Code wrote it to that agent's transcript only, paused the workflow and
    /// did not wake the session's thread. A task's session is at that limit when it binds the
    /// model the session runs; a session whose relaunch cannot tell its agent to resume the
    /// workflow (the Superagent's) leaves it to Claude Code's own continuation at the reset.
    WorkflowLimitReached {
        /// When the limit resets, if the record says so.
        resets_at: Option<Timestamp>,
        /// What Claude Code said.
        message: String,
        /// The window the limit is of, in minutes, if known.
        window_minutes: Option<u32>,
        /// What it binds: the subscription, or one model family (that of the agent's model
        /// when the record names none and the agent runs a model of its own; `Model(None)`
        /// when the agent runs the session's).
        scope: LimitScope,
        /// When it was recorded, if known.
        at: Option<Timestamp>,
    },
    /// The actual model of each subagent answer. Consecutive answers on the same model
    /// can belong to different rounds (SPEC-493 REV-004).
    SubagentAnswered {
        /// Claude Code's id of the subagent (its transcript `agent-<id>.jsonl`).
        agent_id: String,
        /// The model of its answer.
        model: String,
        /// When the answer was written, if the transcript says.
        at: Option<Timestamp>,
    },
    /// A usage limit refused a subagent of the session's own thread: Claude Code ends it with
    /// no report (and no `SubagentStop`); a message to it resumes it (SPEC-493 REV-004). The
    /// limit is the session's news as a workflow agent's is (SPEC-493 RLM-001).
    SubagentCutOff {
        /// Claude Code's id of the subagent.
        agent_id: String,
        /// When it was refused, if known.
        at: Option<Timestamp>,
        /// When the limit resets, if the record says so.
        resets_at: Option<Timestamp>,
        /// What Claude Code said.
        message: String,
        /// The window the limit is of, in minutes, if known.
        window_minutes: Option<u32>,
        /// What it binds: the subscription, or one model family (that of the model that
        /// answered the subagent when the record names none; `Model(None)` when that is not
        /// known, which reads as the session's model).
        scope: LimitScope,
    },
    /// The session launched a dynamic workflow (Claude Code's Workflow tool; SPEC-486 WFL-003),
    /// beside the [`Observation::BackgroundStarted`] of the same `task`.
    WorkflowLaunched {
        /// Claude Code's task id of the run: the notification of its end names it.
        task: String,
        /// The run id the workflow is resumed by (letters, digits, `_` and `-` only).
        run: String,
    },
    /// A task notification said the background work `id` ended.
    BackgroundEnded {
        /// Claude Code's task id.
        id: String,
    },
    /// A task notification said the background agent `id` stopped with background work of its
    /// own still running: it resumes by itself and notifies again (SPEC-079 BGW-006).
    BackgroundResumable {
        /// Claude Code's task id.
        id: String,
    },
    /// Claude Code's goal mode answered a turn an error ended (SPEC-333): it armed a retry, or it
    /// stopped (retries spent, or an API error it does not retry).
    GoalRetry {
        /// A retry is armed: Claude Code continues the goal by itself.
        armed: bool,
        /// What Claude Code said (its notice without the closing advice).
        notice: String,
    },
    /// The API rejected the prompt as too long: the session's context is full (#213).
    ContextFull,
    /// Claude Code compacted the conversation.
    Compacted,
    /// A compaction failed.
    CompactFailed,
    /// Claude Code recorded a change of its queue of waiting prompts, at that time if known
    /// (SPEC-471 CCQ-001).
    Queue(yulius_domain::agent_cli::QueueChange, Option<Timestamp>),
    /// The provider's rate-limit windows of the run's account, as the CLI recorded them (Codex
    /// `token_count` events, SPEC-503 CDX-004).
    RateLimits(AccountUsage),
    /// The provider reported a usage limit.
    LimitReached {
        /// Reset time if known.
        resets_at: Option<Timestamp>,
        /// Provider text.
        message: String,
        /// The limited window in minutes (300 session, 10080 weekly), if known.
        window_minutes: Option<u32>,
        /// What it binds: the subscription, or the models of one family (SPEC-464).
        scope: LimitScope,
        /// When the CLI recorded it, if known (tells a new limit from history, SPEC-074).
        at: Option<Timestamp>,
    },
    /// The provider refused the session's credential for good (a cancelled or downgraded
    /// subscription, a revoked sign-in, a billing failure): the subscription needs a new sign-in.
    AccessLost {
        /// Provider text.
        message: String,
        /// When the CLI recorded it, if known.
        at: Option<Timestamp>,
    },
}

/// Accumulated usage of one attempt, from its transcript lines alone, so reading them again after
/// a restart gives the same figures (SPEC-087).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageSnapshot {
    /// Token totals, its sub-agents' included (unknown fields stay `None`).
    pub tokens: TokenUsage,
    /// Model seen.
    pub model: Option<String>,
    /// The agent CLI's version, as the run's lines name it (SPEC-461 CCM-011).
    pub cli_version: Option<String>,
    /// Completed turns (Claude Code's turn records).
    pub turns: u64,
    /// Active agent time (ms): the union of the turns' spans, a turn in progress up to the
    /// newest line (MET-001). `None` before the first turn.
    pub active_ms: Option<u64>,
    /// The parts of `tokens` that another model than `model` answered: a Codex subagent's
    /// (SPEC-619 CSA-4).
    pub parts: Vec<ModelTokens>,
}

/// Which lines of a session transcript a watch reads (SPEC-087 MET-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchFrom {
    /// All of them: the run started the session.
    Start,
    /// The lines recorded from the run's start on: the run resumed a session whose earlier lines
    /// are earlier runs'. The same lines are read whether the watch starts with the run or after
    /// a restart, so the run's figures are the same; its sub-agents' transcripts count from the
    /// same time.
    Since(Timestamp),
}

/// A live transcript observer.
pub trait TranscriptWatch: Send {
    /// New observations since the last poll.
    ///
    /// # Errors
    /// I/O failures reading the transcript.
    fn poll(&mut self) -> AppResult<Vec<Observation>>;
    /// Accumulated usage.
    fn usage(&self) -> UsageSnapshot;
    /// Whether the last poll read everything the transcript held: until then the observations
    /// are history being replayed (after a restart), not new events.
    fn caught_up(&self) -> bool {
        true
    }
}

/// Streaming filter that hides provider tokens in login terminal output.
pub trait OutputMasker: Send {
    /// Filters a chunk.
    fn push(&mut self, chunk: &[u8]) -> Vec<u8>;
    /// Flushes buffered bytes.
    fn finish(&mut self) -> Vec<u8>;
}

/// An agent CLI Yulius provides itself (SPEC-503 CLI-001): its program, started by this absolute
/// path, and the directory holding it, which a contained run binds read-only at the same path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidedCli {
    /// The executable.
    pub program: PathBuf,
    /// Its release's directory.
    pub dir: PathBuf,
}

/// What [`HarnessPort::provide_codex`] and [`HarnessPort::provide_antigravity`] return.
pub type ProvideFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = AppResult<Option<ProvidedCli>>> + Send + 'a>,
>;

/// Harness operations.
pub trait HarnessPort: Send + Sync {
    /// The Codex CLI Codex runs and sign-ins start (SPEC-503 CLI-002), installed first when
    /// `install` (else only what is ready: a caller holding a lock never waits for a download);
    /// `None`: the `codex` on the run's `PATH`.
    ///
    /// # Errors
    /// `Upstream` when the pinned CLI cannot be installed (or, without `install`, is not
    /// installed yet) and the host has no `codex` either.
    fn provide_codex(&self, install: bool) -> ProvideFuture<'_> {
        let _ = install;
        Box::pin(async { Ok(None) })
    }
    /// The Antigravity CLI Antigravity runs and sign-ins start (SPEC-505 AGY-007), installed
    /// first when `install` (else only what is ready); `None`: the `agy` on the run's `PATH`.
    ///
    /// # Errors
    /// `Upstream` when the pinned CLI cannot be installed (or, without `install`, is not
    /// installed yet) and the host has no `agy` either.
    fn provide_antigravity(&self, install: bool) -> ProvideFuture<'_> {
        let _ = install;
        Box::pin(async { Ok(None) })
    }
    /// Plans a launch.
    ///
    /// # Errors
    /// `Invalid` for impossible requests.
    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan>;
    /// Plans a Cockpit: Claude Code with the Yulius and the sandbox's MCP servers and the
    /// redaction hook as its only hook (SPEC-147), its first-run screens and the folder trust
    /// settled in its config dir.
    ///
    /// # Errors
    /// `Invalid` for impossible requests, `Upstream` when the config cannot be prepared.
    fn plan_cockpit(&self, launch: &CockpitLaunch) -> AppResult<LaunchPlan>;
    /// Path of a Claude session transcript.
    fn claude_transcript(&self, sandbox_home: &Path, worktree: &Path, session_id: &str) -> PathBuf;
    /// Reads the last goal status from the conversation's own complete records.
    ///
    /// # Errors
    /// I/O errors opening or reading the transcript below its sandbox anchor.
    fn last_claude_goal(&self, anchor: &Path, path: &Path) -> AppResult<Option<GoalOutcome>>;

    /// Whether the authenticated session's native transcript identifies a scheduled submit.
    /// The file is opened inside its sandbox anchor. Reads a bounded tail on the blocking pool.
    fn submit_origin(
        &self,
        file: std::fs::File,
        session: &str,
        id: &str,
        prompt: &str,
        run_to_submit: std::ops::RangeInclusive<Timestamp>,
    ) -> yulius_domain::harness::SubmitOrigin;
    /// Steps a Claude session's transcript back over a usage limit (SPEC-034 LIM-001); `true`
    /// when it rewrote the file. Blocking file I/O bounded by the transcript's size: callers run
    /// it off the async runtime (SPEC-072 LRS-003).
    ///
    /// # Errors
    /// I/O failures reading or replacing the transcript.
    fn step_back_transcript(
        &self,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
    ) -> AppResult<bool>;
    /// Newest Claude session for `worktree` modified after `since` (the cockpit's folder). Never a
    /// task's session: any `claude` in the folder writes there (SPEC-281).
    ///
    /// # Errors
    /// I/O failures.
    fn newest_claude_session(
        &self,
        sandbox_home: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>>;
    /// Opens a transcript observer.
    ///
    /// # Errors
    /// I/O failures.
    fn watch(
        &self,
        harness: Harness,
        path: &Path,
        from: WatchFrom,
    ) -> AppResult<Box<dyn TranscriptWatch>>;
    /// Counts an ended run's figures again from its window of its session transcript (the
    /// session `session_id` of `worktree` in `sandbox_home`), by the live watch's rules: the lines
    /// `from` reads, up to the first one recorded at or after `until`, its sub-agents' transcripts
    /// over the same window (SPEC-091 RCT-001, RCT-002). `None` when the transcript does not
    /// exist, or `session_id` is not a session id (it came from the agent's hook). Reads whole
    /// files: call it off the async workers.
    ///
    /// # Errors
    /// I/O failures.
    fn recount(
        &self,
        harness: Harness,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
        from: WatchFrom,
        until: Option<Timestamp>,
    ) -> AppResult<Option<UsageSnapshot>>;
    /// The transcript of the Antigravity conversation `session_id` in the conversations folder
    /// `brain` (SPEC-505 AGY-004).
    fn antigravity_transcript(&self, brain: &Path, session_id: &str) -> PathBuf;
    /// The conversation the Antigravity CLI last ran in `worktree` with the `.gemini` folder
    /// `gemini_dir`, when its transcript in `brain` was written at or after `since` (SPEC-505
    /// AGY-005: for a run whose SessionStart hook named none).
    fn discover_antigravity_session(
        &self,
        gemini_dir: &Path,
        brain: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> Option<String> {
        let _ = (gemini_dir, brain, worktree, since);
        None
    }
    /// The credential to keep after an Antigravity run of a Google sign-in left `fresh` as its
    /// token file, `stored` being the account's (SPEC-505 AGY-008); `None` keeps the stored one.
    fn antigravity_refreshed_credential(&self, fresh: &[u8], stored: &[u8]) -> Option<Vec<u8>> {
        let _ = (fresh, stored);
        None
    }
    /// The credential of an Antigravity account made of a Gemini API key, `None` for text that
    /// cannot be one (SPEC-505 AGY-008).
    fn antigravity_api_key_credential(&self, key: &str) -> Option<Vec<u8>> {
        let _ = key;
        None
    }
    /// The credential of an Antigravity account made of the token file its Google sign-in wrote,
    /// `None` for an empty or oversized one (SPEC-505 AGY-008).
    fn antigravity_sign_in_credential(&self, token_file: &[u8]) -> Option<Vec<u8>> {
        let _ = token_file;
        None
    }
    /// The rollout of the Codex session `session_id` below `sessions_dir` (SPEC-503 CDX-003).
    ///
    /// # Errors
    /// I/O failures; an invalid session id.
    fn find_codex_rollout(
        &self,
        sessions_dir: &Path,
        session_id: &str,
    ) -> AppResult<Option<PathBuf>>;
    /// The id and rollout of the Codex session started for `worktree` at or after `since`, read
    /// from the `session_meta` of the newest rollouts below `sessions_dir` (when no hook named it).
    ///
    /// # Errors
    /// I/O failures.
    fn discover_codex_session(
        &self,
        sessions_dir: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>>;
    /// Why a Codex `auth.json` is not a usable credential, `None` when it is (SPEC-503 CDX-005).
    fn codex_auth_problem(&self, auth_json: &[u8]) -> Option<String>;
    /// Why an imported `auth.json` is not a full ChatGPT sign-in of Codex, `None` when it is
    /// (SPEC-503 CDX-011). The reason never quotes the file.
    fn codex_import_problem(&self, auth_json: &[u8]) -> Option<String>;
    /// The `auth.json` of an OpenAI API key, `None` when the key is not one.
    fn codex_api_key_auth(&self, api_key: &str) -> Option<Vec<u8>>;
    /// Whether the `auth.json` a Codex run left (`fresh`, written where the agent can write)
    /// replaces the account's `stored` one: a genuine refresh of the same sign-in, not dated after
    /// `now` (SPEC-503 CDX-006).
    fn codex_auth_replaces(&self, fresh: &[u8], stored: &[u8], now: Timestamp) -> bool;
    /// The `config.toml` of a Codex home Yulius prepares for a sign-in: credentials in a file.
    fn codex_login_config(&self) -> Vec<u8>;
    /// Login command for a provider account.
    fn login_argv(&self, harness: Harness) -> Vec<String>;
    /// Extracts a Claude OAuth token from login output.
    fn extract_claude_token(&self, output: &str) -> Option<String>;
    /// A new output masker.
    fn masker(&self) -> Box<dyn OutputMasker>;
    /// Installs skill directories into `skills_dir`, a path relative to `root` (the sandbox
    /// directory, which the agent cannot replace; it can change everything below it).
    ///
    /// # Errors
    /// I/O failures, or a link on the way from `root` to `skills_dir`.
    fn install_skills(
        &self,
        skills: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<()>;
    /// [`HarnessPort::install_skills`], writing only what differs from the installed copies
    /// (SPEC-507 SKR-001): the skills installed again and the managed copies removed, both empty
    /// when every copy matched.
    ///
    /// # Errors
    /// As [`HarnessPort::install_skills`].
    fn refresh_skills(
        &self,
        skills: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<SkillsRefresh>;
}

/// What [`HarnessPort::refresh_skills`] changed in a skills folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillsRefresh {
    /// Skills whose copy was missing or differed, installed again.
    pub changed: Vec<String>,
    /// Managed copies removed because their skill left the set.
    pub removed: Vec<String>,
}

impl SkillsRefresh {
    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}
