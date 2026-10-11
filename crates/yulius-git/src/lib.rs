//! Git command-line adapter: clones, worktrees, branches, bundles, diffs and hook installation
//! with safe argument boundaries (SPEC-002 GIT-001).
//!
//! The adapter is synchronous; async callers run it on blocking tasks
//! (`tokio::task::spawn_blocking`). Every command:
//!
//! - is spawned with an executable plus argv (never a shell), revisions are resolved with
//!   `rev-parse --verify --end-of-options` before reuse, paths are absolute or follow `--`,
//!   and branch names pass [`validate_branch_name`];
//! - gets a cleared environment plus a small allowlist (`PATH`, `HOME`, …), the caller's
//!   [`Git::env`], `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C`; `-c` options are constants, never
//!   input;
//! - captures at most [`Git::max_output_bytes`] of stdout (64 KiB of stderr) and is killed
//!   with its whole process group, then reaped, after its timeout.
//!
//! Identity is worktree-scoped through `extensions.worktreeConfig` (see
//! [`Git::set_local_identity`]) so agents can run plain `git commit` inside their worktree.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

mod bundle;
mod ci;
mod commit;
pub mod credential;
mod error;
mod hooks;
mod redact;
mod repo;
mod review;
mod runner;
mod status;
mod validate;
mod worktree;

pub use bundle::{BundleHead, BundleSpec, IMPORT_PREFIX, ImportRef};
pub use ci::BranchTip;
pub use commit::{Author, MAX_COMMIT_MESSAGE};
pub use credential::{run_credential_helper, run_gh_token};
pub use error::GitError;
pub use hooks::HOOKS_DIR_NAME;
pub use redact::redact;
pub use review::{
    CommitStat, DiffHunk, DiffLine, FileDiff, FileStatus, LineKind, MAX_REVIEW_COMMITS,
    MAX_REVIEW_FILES, MAX_REVIEW_LINES, WorktreeDiff,
};
pub use status::{AheadBehind, Commit, DiffStat, MAX_LOG_ENTRIES, StatusEntry};
pub use validate::{MAX_BRANCH_LEN, validate_branch_name, validate_relative_path};
pub use worktree::{RemovalRefusal, Worktree, WorktreeRemoval};

/// Default deadline for local commands.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Default deadline for network commands (clone, fetch, push).
pub const DEFAULT_NETWORK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Default bound on captured standard output.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Handle to a Git executable with its environment and bounds.
///
/// `env` is for non-secret configuration only (for example `GIT_CONFIG_COUNT`/`KEY`/`VALUE`
/// entries that register the `yulius git-credential` helper); its values are not shown by
/// `Debug`.
#[derive(Clone)]
pub struct Git {
    /// Path or name of the `git` executable.
    pub exe: PathBuf,
    /// Extra environment variables for every command (applied after the allowlist).
    pub env: Vec<(String, String)>,
    /// Deadline for local commands.
    pub timeout: Duration,
    /// Deadline for clone, fetch, push and bundle fetches.
    pub network_timeout: Duration,
    /// Bound on captured stdout; larger output kills the command.
    pub max_output_bytes: usize,
}

impl Git {
    /// Creates a handle for `exe` with default bounds and no extra environment.
    #[must_use]
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            env: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Adds one environment variable for every command.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

impl Default for Git {
    fn default() -> Self {
        Self::new("git")
    }
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&str> = self.env.iter().map(|(key, _)| key.as_str()).collect();
        f.debug_struct("Git")
            .field("exe", &self.exe)
            .field("env_keys", &keys)
            .field("timeout", &self.timeout)
            .field("network_timeout", &self.network_timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .finish()
    }
}
