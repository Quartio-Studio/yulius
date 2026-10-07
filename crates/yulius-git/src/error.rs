//! Typed Git adapter failures.

use std::time::Duration;

/// A Git adapter failure. Messages never contain credentials: captured stderr is redacted
/// before it is stored (see [`crate::redact`]).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
    /// A branch name failed `git check-ref-format --branch` equivalent rules.
    #[error("invalid branch name {name:?}: {reason}")]
    InvalidBranchName {
        /// The rejected name (truncated).
        name: String,
        /// Which rule it broke.
        reason: &'static str,
    },
    /// A repository-relative path was absolute, escaped the tree or targeted `.git`.
    #[error("invalid relative path {path:?}: {reason}")]
    InvalidPath {
        /// The rejected path (truncated).
        path: String,
        /// Which rule it broke.
        reason: &'static str,
    },
    /// Another argument (remote, URL, refspec, revision, identity, message) was rejected.
    #[error("invalid {what}: {reason}")]
    InvalidInput {
        /// The kind of argument.
        what: &'static str,
        /// Why it was rejected.
        reason: String,
    },
    /// The Git executable could not be started.
    #[error("could not start git: {0}")]
    Spawn(#[source] std::io::Error),
    /// Local file I/O failed (hooks, temporary files, canonicalization).
    #[error("I/O error during {context}: {source}")]
    Io {
        /// What the adapter was doing.
        context: &'static str,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The command exceeded its deadline; its process group was killed and reaped.
    #[error("git {command} timed out after {after:?}")]
    Timeout {
        /// The Git subcommand.
        command: &'static str,
        /// The configured limit.
        after: Duration,
    },
    /// The command produced more output than the configured bound; it was killed and reaped.
    #[error("git {command} produced more than {limit} bytes of output")]
    OutputTooLarge {
        /// The Git subcommand.
        command: &'static str,
        /// The configured bound.
        limit: usize,
    },
    /// The command exited unsuccessfully.
    #[error("git {command} failed (exit {code:?}): {stderr}")]
    Failed {
        /// The Git subcommand.
        command: &'static str,
        /// Exit code, `None` when terminated by a signal.
        code: Option<i32>,
        /// Redacted, truncated standard error.
        stderr: String,
    },
    /// A revision or ref did not resolve to a commit.
    #[error("revision not found: {0}")]
    RevisionNotFound(String),
    /// The branch to create already exists.
    #[error("branch already exists: {0}")]
    BranchExists(String),
    /// Git produced output the adapter could not parse.
    #[error("unexpected git output from {command}: {detail}")]
    Parse {
        /// The Git subcommand.
        command: &'static str,
        /// What was wrong.
        detail: String,
    },
}

pub(crate) fn truncate_for_error(text: &str) -> String {
    const MAX: usize = 200;
    if text.len() <= MAX {
        return text.to_owned();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
