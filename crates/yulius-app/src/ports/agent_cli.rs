//! The Claude Code sandboxes get (SPEC-461): what the instance page shows of it, and what a
//! superadmin changes. The composition owns the registry client, the installed copies and the
//! reading of binaries.

use async_trait::async_trait;
use yulius_domain::agent_cli::{Channel, Version};
use yulius_domain::time::Timestamp;

/// Where the Claude Code runs get comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSource {
    /// Yulius's copy under `<data>/claude-code`, kept current by the checks.
    Yulius,
    /// The host's npm install.
    Host,
}

/// What a check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// This release was installed and is what runs get.
    Installed(Version),
    /// Runs already get this version (or a newer one than the channel's).
    UpToDate(Version),
    /// Releases refused because they do not know the default model, in the order tried (the
    /// channel's, then `latest`'s on `stable`), each with the offered models it lacks.
    Refused(Vec<(Version, Vec<String>)>),
    /// The check failed (the registry unreachable, a download refused), in words.
    Failed(String),
}

/// The state of the Claude Code sandboxes get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCliStatus {
    /// Why Yulius keeps nothing current on this server (no npm install on the host, another
    /// package layout, no platform package); `None` when it does.
    pub unmanaged: Option<String>,
    /// The channel in effect.
    pub channel: Channel,
    /// The version runs get, when its package names one.
    pub version: Option<Version>,
    /// Where it comes from; `None` without a Claude Code to bind.
    pub source: Option<CliSource>,
    /// The host's own version.
    pub host_version: Option<Version>,
    /// The offered models the version runs get does not know, the default first; `None` when
    /// its binary cannot be read.
    pub missing: Option<Vec<String>>,
    /// npm's `latest` at the last check.
    pub latest: Option<Version>,
    /// npm's `stable` at the last check.
    pub stable: Option<Version>,
    /// When the registry was last asked.
    pub checked_at: Option<Timestamp>,
    /// A check runs now.
    pub checking: bool,
    /// What the last check did; `None` before the first, or when nothing is checked (`off`).
    pub result: Option<CheckResult>,
}

/// The Claude Code sandboxes get, kept current by the composition.
#[async_trait]
pub trait AgentCli: Send + Sync {
    /// Its state now. Reads the binary runs get once per file (about 340 MB).
    async fn status(&self) -> AgentCliStatus;
    /// Starts a check now; one that runs already is the check.
    fn check_now(&self);
    /// Follows `channel` from now on: runs started from here bind as it says, and a check runs
    /// at once. `chosen` says it was chosen in Settings, not the server's configuration.
    fn set_channel(&self, channel: Channel, chosen: bool);
    /// The version a run starting now gets; `None` when unknown.
    fn version_for_new_runs(&self) -> Option<Version>;
}
