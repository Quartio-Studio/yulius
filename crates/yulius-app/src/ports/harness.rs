//! Harness port: launch planning, transcript observation and login helpers for Claude Code
//! (SPEC-002 §7.2). Implemented over `yulius-harness`.

use std::path::{Path, PathBuf};

use yulius_domain::harness::Harness;
use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::TokenUsage;

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
    },
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
    /// Provider credential (the Claude OAuth token).
    pub credential: Secret,
}

/// A user's Cockpit launch (SPEC-021): plain Claude Code outside any task.
#[derive(Debug, Clone)]
pub struct CockpitLaunch {
    /// Working directory (the sandbox repository).
    pub cwd: PathBuf,
    /// Claude's config dir as the session sees it.
    pub config_dir: PathBuf,
    /// The same directory on the host, prepared before the launch.
    pub config_dir_host: PathBuf,
    /// Conversation to resume.
    pub resume: Option<String>,
    /// The resumed conversation continues the turn a usage limit cut (SPEC-074 SUP-004): its
    /// transcript steps back over the limit and Claude Code redoes the turn by itself.
    pub continue_interrupted: bool,
    /// Provider credential (the Claude OAuth token).
    pub credential: Secret,
    /// The Yulius MCP server it manages the project with (SPEC-037): the internal listener's
    /// URL and the Cockpit's bearer.
    pub yulius: Option<(String, Secret)>,
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
    /// Claude Code recorded the session goal's state (TSK-014).
    Goal(GoalOutcome),
    /// Claude Code closed a turn after its Stop hooks (only used to reduce a replay).
    TurnEnded {
        /// Background agents Claude Code still waits for; `None` when it does not say.
        background_agents: Option<u32>,
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
    /// A task notification said the background work `id` ended.
    BackgroundEnded {
        /// Claude Code's task id.
        id: String,
    },
    /// The API rejected the prompt as too long: the session's context is full (#213).
    ContextFull,
    /// Claude Code compacted the conversation.
    Compacted,
    /// A compaction failed.
    CompactFailed,
    /// The provider reported a usage limit.
    LimitReached {
        /// Reset time if known.
        resets_at: Option<Timestamp>,
        /// Provider text.
        message: String,
        /// The limited window in minutes (300 session, 10080 weekly), if known.
        window_minutes: Option<u32>,
        /// When the CLI recorded it, if known (tells a new limit from history, SPEC-074).
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
    /// Completed turns (Claude Code's turn records).
    pub turns: u64,
    /// Active agent time (ms): the union of the turns' spans, a turn in progress up to the
    /// newest line (MET-001). `None` before the first turn.
    pub active_ms: Option<u64>,
    /// When the turn in progress started, if one is.
    pub turn_open_since: Option<Timestamp>,
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

/// Harness operations.
pub trait HarnessPort: Send + Sync {
    /// Plans a launch.
    ///
    /// # Errors
    /// `Invalid` for impossible requests.
    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan>;
    /// Plans a Cockpit: plain Claude Code without hooks, MCP or settings, its first-run screens
    /// and the folder trust settled in its config dir.
    ///
    /// # Errors
    /// `Invalid` for impossible requests, `Upstream` when the config cannot be prepared.
    fn plan_cockpit(&self, launch: &CockpitLaunch) -> AppResult<LaunchPlan>;
    /// Path of a Claude session transcript.
    fn claude_transcript(&self, sandbox_home: &Path, worktree: &Path, session_id: &str) -> PathBuf;
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
    /// Newest Claude session for `worktree` modified after `since` (session-id drift, HRN-004).
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
    /// Counts an ended run's figures again from its window of a session transcript, by the live
    /// watch's rules: the lines `from` reads, up to the first one recorded at or after `until`,
    /// its sub-agents' transcripts over the same window (SPEC-091 RCT-001, RCT-002). `None` when
    /// the transcript does not exist. Reads whole files: call it off the async workers.
    ///
    /// # Errors
    /// I/O failures.
    fn recount(
        &self,
        harness: Harness,
        path: &Path,
        from: WatchFrom,
        until: Option<Timestamp>,
    ) -> AppResult<Option<UsageSnapshot>>;
    /// Login command for a provider account.
    fn login_argv(&self, harness: Harness) -> Vec<String>;
    /// Extracts a Claude OAuth token from login output.
    fn extract_claude_token(&self, output: &str) -> Option<String>;
    /// A new output masker.
    fn masker(&self) -> Box<dyn OutputMasker>;
    /// Installs skill directories into the Claude home.
    ///
    /// # Errors
    /// I/O failures.
    fn install_skills(&self, skills: &[(String, PathBuf)], claude_dir: &Path) -> AppResult<()>;
}
