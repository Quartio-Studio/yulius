//! Harness port: launch planning, transcript observation, translation and login helpers for
//! Claude Code and Codex (SPEC-002 §7.2–§7.3). Implemented over `yulius-harness`.

use std::path::{Path, PathBuf};

use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, TokenUsage};

use crate::error::AppResult;
use crate::model::Secret;
use crate::ports::session::LaunchPlan;

/// New session or native resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMode {
    /// New session (Claude gets `session_id`; Codex assigns its own).
    New {
        /// Session id to assign (Claude).
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
    /// Codex home for the (sandbox, account) pair.
    pub codex_home: Option<PathBuf>,
    /// Model.
    pub model: Option<String>,
    /// Effort.
    pub effort: Option<String>,
    /// Contained run.
    pub contained: bool,
    /// Extra MCP servers.
    pub mcp: Vec<McpAttachment>,
    /// Provider credential (Claude token or Codex auth.json text).
    pub credential: Secret,
}

/// A launch plan plus the session id known up front.
#[derive(Debug, Clone)]
pub struct HarnessPlan {
    /// Process plan (containment filled in by the supervisor).
    pub plan: LaunchPlan,
    /// Session id (Claude new/resume, Codex resume); `None` for a new Codex session.
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
    UserTurn,
    /// A turn completed.
    TurnComplete,
    /// Provider rate-limit snapshot.
    RateLimits(AccountUsage),
    /// The provider reported a usage limit.
    LimitReached {
        /// Reset time if known.
        resets_at: Option<Timestamp>,
        /// Provider text.
        message: String,
    },
}

/// Accumulated usage of one attempt.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageSnapshot {
    /// Token totals (unknown fields stay `None`).
    pub tokens: TokenUsage,
    /// Model seen.
    pub model: Option<String>,
    /// Completed turns.
    pub turns: u64,
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
    /// Path of a Claude session transcript.
    fn claude_transcript(&self, sandbox_home: &Path, worktree: &Path, session_id: &str) -> PathBuf;
    /// Finds a Codex rollout by session id.
    ///
    /// # Errors
    /// I/O failures.
    fn find_codex_rollout(
        &self,
        sessions_dir: &Path,
        session_id: &str,
    ) -> AppResult<Option<PathBuf>>;
    /// Discovers the session id of a Codex rollout created after `since` for `worktree`.
    ///
    /// # Errors
    /// I/O failures.
    fn discover_codex_session(
        &self,
        sessions_dir: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>>;
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
        from_start: bool,
    ) -> AppResult<Box<dyn TranscriptWatch>>;
    /// Writes a transcript of `source` (harness `from`) as a native `to` session with
    /// `new_session_id`; returns the path written.
    ///
    /// # Errors
    /// I/O failures or unreadable sources.
    fn translate(
        &self,
        from: Harness,
        source: &Path,
        to: Harness,
        target: &TranslationTarget,
    ) -> AppResult<PathBuf>;
    /// Login command for a provider account.
    fn login_argv(&self, harness: Harness) -> Vec<String>;
    /// Extracts a Claude OAuth token from login output.
    fn extract_claude_token(&self, output: &str) -> Option<String>;
    /// A new output masker.
    fn masker(&self) -> Box<dyn OutputMasker>;
    /// Installs skill directories into both harness homes.
    ///
    /// # Errors
    /// I/O failures.
    fn install_skills(
        &self,
        skills: &[(String, PathBuf)],
        claude_dir: &Path,
        codex_dir: &Path,
    ) -> AppResult<()>;
}

/// Where a translated transcript goes.
#[derive(Debug, Clone)]
pub struct TranslationTarget {
    /// New provider session id.
    pub session_id: String,
    /// Working directory recorded in the transcript.
    pub cwd: PathBuf,
    /// Sandbox home (Claude config lives under it).
    pub sandbox_home: PathBuf,
    /// Codex sessions directory.
    pub codex_sessions: PathBuf,
    /// Model to record.
    pub model: Option<String>,
    /// Effort to record.
    pub effort: Option<String>,
    /// Git branch.
    pub git_branch: Option<String>,
    /// Current time.
    pub now: Timestamp,
}
