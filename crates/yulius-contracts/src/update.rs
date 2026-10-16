//! The update helper's file protocol (SPEC-002 ADR-18, version 1).
//!
//! The server, running as the unprivileged service user, writes one [`HelperRequest`] to
//! `<data>/update/request.json` (mode 0600, written atomically). A root-owned systemd path unit
//! starts `yulius update-helper`, which accepts nothing but "build" or "install" one commit of
//! `main` of the fixed repository, and reports in [`HelperStatus`] at
//! `<helper state>/status.json` (root-owned, readable by the service). Both files are bounded
//! and carry [`VERSION`]; anything else is refused.

use serde::{Deserialize, Serialize};

/// Protocol version.
pub const VERSION: u32 = 1;
/// Largest request file.
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024;
/// Largest status file.
pub const MAX_STATUS_BYTES: u64 = 256 * 1024;
/// Lines of helper output kept in the status.
pub const LOG_TAIL_LINES: usize = 40;
/// A request older than this is stale and refused (a reboot must not replay it).
pub const REQUEST_TTL_MS: i64 = 15 * 60 * 1000;

/// What the helper is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperAction {
    /// Fetch, verify and build the commit; stage the binary. Touches no service.
    Build,
    /// Stop the service, back up, install the staged build, start, check; roll back on failure.
    Install,
}

impl HelperAction {
    /// Stable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Install => "install",
        }
    }
}

/// `<data>/update/request.json`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequest {
    /// [`VERSION`].
    pub version: u32,
    /// Request id (8–64 of `[A-Za-z0-9-]`); the status echoes it, and an id is acted on once.
    pub id: String,
    /// Build or install.
    pub action: HelperAction,
    /// Full commit id (40 lowercase hex).
    pub commit: String,
    /// A short-lived token that may read the repository (the App's installation token or the
    /// configured read token), for the fetch only; never stored by the helper.
    #[serde(default)]
    pub token: Option<String>,
    /// When it was written (ms since the epoch).
    pub requested_at: i64,
}

impl std::fmt::Debug for HelperRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelperRequest")
            .field("version", &self.version)
            .field("id", &self.id)
            .field("action", &self.action)
            .field("commit", &self.commit)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("requested_at", &self.requested_at)
            .finish()
    }
}

/// Where a helper run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    /// Working on the request.
    Running,
    /// Finished: the build is staged (build) or the new build runs (install).
    Done,
    /// Nothing changed, or the service is in an unknown state (see the error).
    Failed,
    /// The new build failed its check; the previous binary and database are back and running.
    RolledBack,
}

/// Compiled units so far and in total (cargo's own count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperProgress {
    /// Done.
    pub done: u32,
    /// Total.
    pub total: u32,
}

/// `<helper state>/status.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperStatus {
    /// [`VERSION`].
    pub version: u32,
    /// The request it reports on.
    pub id: String,
    /// Its action.
    pub action: HelperAction,
    /// Its commit.
    pub commit: String,
    /// Running, done, failed or rolled back.
    pub state: HelperState,
    /// Current step: `fetching`, `verifying`, `compiling`, `built`, `stopping`, `backing_up`,
    /// `installing`, `starting`, `checking`, `succeeded`, `rolling_back`, `rolled_back`,
    /// `failed`.
    pub step: String,
    /// Compilation progress.
    #[serde(default)]
    pub progress: Option<HelperProgress>,
    /// Why it failed (no credentials).
    #[serde(default)]
    pub error: Option<String>,
    /// The commit that ran before an install (from its health endpoint), when known.
    #[serde(default)]
    pub previous: Option<String>,
    /// The last lines of output (no progress bars, no credentials).
    #[serde(default)]
    pub log_tail: Vec<String>,
    /// After a healthy install: the installed binary's `host check` (SPEC-203 HST-012).
    #[serde(default)]
    pub host_check: Option<HostCheckReport>,
    /// Last write (ms since the epoch).
    pub updated_at: i64,
}

/// What one item of `yulius host setup` or `yulius host check` did (SPEC-203).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOutcome {
    /// It holds; left alone.
    Ok,
    /// It was changed (or, in a dry run, would be).
    Changed,
    /// Left to the operator, for the reason given.
    Manual,
    /// It does not hold and could not be fixed.
    Failed,
}

/// One item of a host setup or check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLine {
    /// What was checked ("Docker Engine", "kernel modules", "sandbox Docker", …).
    pub name: String,
    /// What happened.
    pub outcome: HostOutcome,
    /// What holds, what changed, or why not.
    pub detail: String,
}

/// `yulius host check --json`: the host's prerequisites of Docker in sandboxes after an update,
/// with only safe fixes applied (SPEC-203 HST-012). The update helper keeps the last one in
/// `<helper state>/host-check.json` and in [`HelperStatus::host_check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCheckReport {
    /// Every item failed nothing and the "sandbox Docker" verdict holds.
    pub ok: bool,
    /// One line per item, the verdict last.
    #[serde(default)]
    pub lines: Vec<HostLine>,
    /// Why the check could not run (the installed binary has no `host check`, it timed out);
    /// `ok` is false then.
    #[serde(default)]
    pub error: Option<String>,
    /// The commit whose binary ran it, when known.
    #[serde(default)]
    pub commit: Option<String>,
    /// When it ran (ms since the epoch).
    #[serde(default)]
    pub checked_at: i64,
}

impl HostCheckReport {
    /// The items that are not fine, as one sentence each (empty when `ok`).
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self.error.iter().cloned().collect();
        problems.extend(
            self.lines
                .iter()
                .filter(|l| l.outcome == HostOutcome::Failed)
                .map(|l| format!("{}: {}", l.name, l.detail)),
        );
        problems
    }
}

/// `GET /internal/version` on the internal listener: what the helper's health check reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The commit this server was built from (`None` when unknown).
    pub commit: Option<String>,
    /// The crate version.
    pub version: String,
}
