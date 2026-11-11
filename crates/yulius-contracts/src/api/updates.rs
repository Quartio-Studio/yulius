//! Instance updates from `main` of Quartio-Studio/yulius (SPEC-001 OPS-006..OPS-009, SPEC-002
//! ADR-18) and the Jev key (ADM-001, ADM-002). No response ever carries a credential.

use serde::{Deserialize, Serialize};

/// A build of Yulius.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BuildView {
    /// The full commit id it was built from; `None` when unknown (built outside Git).
    pub commit: Option<String>,
    /// Its first 7 characters.
    pub short: Option<String>,
}

/// One new commit of `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitView {
    /// Full id.
    pub sha: String,
    /// First 7 characters.
    pub short: String,
    /// Readable title (conventional prefix and pull request number removed).
    pub title: String,
    /// GitHub login of the author, when known.
    pub author: Option<String>,
    /// Commit time (RFC 3339), when known.
    pub date: Option<String>,
}

/// The newest `main` when it differs from the running build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdate {
    /// Head of `main`.
    pub commit: String,
    /// Its first 7 characters.
    pub short: String,
    /// "A · B · C and 4 more".
    pub summary: String,
    /// New commits that have a title (merges and promotions left out).
    pub count: u32,
    /// Those commits, newest first (at most 50).
    pub commits: Vec<CommitView>,
    /// How the build relates to `main`: `behind`, `diverged` or `unknown`.
    pub relation: String,
}

/// The last check of `main`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateCheckView {
    /// When it ran (RFC 3339); `None` before the first check.
    pub checked_at: Option<String>,
    /// Why it failed, in words.
    pub error: Option<String>,
    /// What read the repository: `app` (the GitHub App), `token` (`YULIUS_UPDATE_TOKEN_FILE`)
    /// or `anonymous`.
    pub via: Option<String>,
    /// How the build relates to `main`: `up_to_date`, `behind`, `ahead`, `diverged`, `unknown`.
    pub relation: Option<String>,
    /// Head of `main` at the check.
    pub head: Option<String>,
}

/// Progress of a step that counts (compiled crates, released tasks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressView {
    /// Done so far.
    pub done: u32,
    /// Total.
    pub total: u32,
}

/// A task the pause waits for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingTask {
    /// Task id.
    pub id: String,
    /// Its number in the sandbox.
    pub number: u32,
    /// Title.
    pub title: String,
    /// Sandbox slug (for a link).
    pub sandbox: String,
}

/// A deploy the update waits for before it pauses tasks (SPEC-342).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingDeploy {
    /// Project (sandbox) slug, for a link to its CI/CD page.
    pub sandbox: String,
    /// Environment.
    pub environment: String,
    /// `cd` (a CD run) or `environment` (a delivery environment run).
    pub kind: String,
    /// Run id.
    pub run: String,
}

/// How far pausing the tasks got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseProgress {
    /// Live tasks at a pause point.
    pub paused: u32,
    /// Live tasks.
    pub total: u32,
    /// Tasks still in a turn (at most 20).
    pub waiting: Vec<WaitingTask>,
    /// When pausing gives up and cancels the update (RFC 3339).
    pub deadline: Option<String>,
}

/// One update run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRunView {
    /// Id.
    pub id: String,
    /// A restart with the running build, not an update (SPEC-432 RST-002).
    #[serde(default)]
    pub restart: bool,
    /// The commit it installs.
    pub target: BuildView,
    /// The build it started from.
    pub from: BuildView,
    /// Summary of the commits it brings.
    pub summary: String,
    /// `building`, `pausing`, `restarting`, `resuming`, `succeeded`, `failed`, `rolled_back` or
    /// `cancelled`.
    pub state: String,
    /// The helper's current step: `fetching`, `verifying`, `compiling`, `built`, `stopping`,
    /// `backing_up`, `installing`, `starting`, `checking`, `checking_host`, `rolling_back`, …
    pub step: Option<String>,
    /// One calm sentence for the current step.
    pub message: Option<String>,
    /// Compiled crates while building; released tasks while resuming.
    pub progress: Option<ProgressView>,
    /// While pausing: how many tasks are paused.
    pub tasks: Option<PauseProgress>,
    /// Deploys still running that the update waits for before it pauses tasks and restarts (at
    /// most 20; SPEC-342).
    #[serde(default)]
    pub deploys: Vec<WaitingDeploy>,
    /// Why it failed or was rolled back.
    pub error: Option<String>,
    /// The last lines of the helper's output when it failed (no credentials).
    pub log_tail: Vec<String>,
    /// After the install: the new version's re-check of the host for sandboxes' Docker (SPEC-203
    /// HST-012); not `ok` is a regression, said with its reasons.
    #[serde(default)]
    pub docker: Option<crate::update::HostCheckReport>,
    /// Login of the superadmin who started it.
    pub requested_by: String,
    /// When it started (RFC 3339).
    pub started_at: String,
    /// When its current state began (RFC 3339).
    pub state_since: String,
    /// When it ended (RFC 3339).
    pub finished_at: Option<String>,
    /// Whether "Cancel" is possible now (building or pausing).
    pub can_cancel: bool,
    /// Whether "Continue without waiting" is possible now (pausing).
    pub can_proceed: bool,
}

/// `GET /api/update`: what everyone sees (the build and the phase of a running update) plus,
/// for superadmins, the available update, the last check and the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdateView {
    /// The build this server runs.
    pub current: BuildView,
    /// The phase of a running update (`building`, `pausing`, `restarting`, `resuming`), for
    /// everyone; `None` when no update runs.
    pub phase: Option<String>,
    /// Superadmins only: whether this server can update itself (the helper is installed).
    pub helper_installed: Option<bool>,
    /// Superadmins only: the update offered.
    pub available: Option<AvailableUpdate>,
    /// Superadmins only: the last check.
    pub check: Option<UpdateCheckView>,
    /// Superadmins only: the running update, else the last one.
    pub run: Option<UpdateRunView>,
}

/// `POST /api/admin/update`: install exactly this commit (the offered head of `main`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartUpdate {
    /// The full commit id the person confirmed.
    pub commit: String,
    /// Must be `true`: the person confirmed that tasks pause for the restart.
    #[serde(default)]
    pub confirm: bool,
}

/// `POST /api/admin/restart`: restart the server with the build it runs, its tasks frozen
/// meanwhile (SPEC-432 RST-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRestart {
    /// Must be `true`: the person confirmed that tasks pause for the restart.
    #[serde(default)]
    pub confirm: bool,
}

/// `GET /api/admin/jev`: the Jev key's state, never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JevKeyView {
    /// Whether task intake has a key now.
    pub configured: bool,
    /// Where the key in use comes from: `settings` (saved here) or `config`
    /// (`YULIUS_JEV_API_KEY_FILE`); `None` without a key.
    pub source: Option<String>,
    /// When the saved key was set (RFC 3339).
    pub set_at: Option<String>,
    /// Who set it (login).
    pub set_by: Option<String>,
    /// Whether `YULIUS_JEV_API_KEY_FILE` holds a usable key (used when nothing is saved here).
    pub config_present: bool,
    /// Why the configured key file is not usable, in words.
    pub config_error: Option<String>,
}

/// `PUT /api/admin/jev`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetJevKey {
    /// The TypeSafe Jev API key (8–4096 printable characters, no spaces).
    pub key: String,
}

/// `POST /api/admin/jev/test`: one cheap request with the key in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevTestResult {
    /// Jev answered correctly.
    pub ok: bool,
    /// `ok`, `refused` (the key was rejected), `unreachable`, `timeout`, `invalid` (an answer
    /// outside the contract) or `none` (no key).
    pub result: String,
    /// One sentence for the person.
    pub message: String,
    /// Round trip of the request in milliseconds.
    pub latency_ms: Option<u64>,
}

/// `GET /api/admin/vidoc`: the instance Vidoc key's state, never the key (SPEC-195 VDK-002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VidocKeyView {
    /// Whether the instance has a Vidoc key now (without one, the security scan is off).
    pub configured: bool,
    /// Where the key in use comes from: `settings` (saved here) or `config`
    /// (`YULIUS_VIDOC_API_KEY_FILE`); `None` without a key.
    pub source: Option<String>,
    /// When the saved key was set (RFC 3339).
    pub set_at: Option<String>,
    /// Who set it (login).
    pub set_by: Option<String>,
    /// Whether `YULIUS_VIDOC_API_KEY_FILE` holds a usable key (used when nothing is saved here).
    pub config_present: bool,
    /// Why the configured key file is not usable, in words.
    pub config_error: Option<String>,
    /// Whether Vidoc scans are on for the instance (SPEC-356 VIO-001): off, no project scans,
    /// whatever its own switch or key.
    pub enabled: bool,
    /// While off: when it was switched off (RFC 3339).
    pub disabled_at: Option<String>,
    /// While off: who switched it off (login).
    pub disabled_by: Option<String>,
}

/// `PUT /api/admin/vidoc/enabled` (SPEC-356 VIO-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetVidocEnabled {
    /// `false` switches Vidoc off for every project of the instance, `true` on again.
    pub enabled: bool,
}

/// `PUT /api/admin/vidoc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetVidocKey {
    /// The Vidoc API key from <https://model.vidocsecurity.com/#key> (8–4096 printable
    /// characters, no spaces).
    pub key: String,
}

/// `POST /api/admin/vidoc/test`: one `GET /v1/models` with the key in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VidocTestResult {
    /// Vidoc answered and lists `vidoc-1`.
    pub ok: bool,
    /// `ok`, `refused` (the key was rejected), `unreachable`, `timeout`, `invalid` (no `vidoc-1`
    /// in the answer) or `none` (no key).
    pub result: String,
    /// One sentence for the person.
    pub message: String,
    /// Round trip of the request in milliseconds.
    pub latency_ms: Option<u64>,
}

/// `GET /api/admin/claude-code` (superadmin; SPEC-461 CCM-008): the Claude Code sandboxes get,
/// whether it knows the models Yulius offers, the release channel and the last check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeView {
    /// Yulius keeps Claude Code current for sandboxes on this server.
    pub managed: bool,
    /// Why not, when it does not (no isolation, no npm install on the host, another layout).
    pub unmanaged: Option<String>,
    /// The channel in effect: `latest`, `stable` or `off`.
    pub channel: String,
    /// Where it is set: `settings` (chosen here) or `config` (`YULIUS_CLAUDE_UPDATES`).
    pub channel_source: String,
    /// The channel of the server's configuration, followed when none is chosen here.
    pub config_channel: String,
    /// Who chose it here (login).
    pub channel_set_by: Option<String>,
    /// When it was chosen here (RFC 3339).
    pub channel_set_at: Option<String>,
    /// The version a run starting now gets.
    pub version: Option<String>,
    /// Where it comes from: `yulius` (kept current in the data directory) or `host`.
    pub source: Option<String>,
    /// The host's own version.
    pub host_version: Option<String>,
    /// npm's `latest` at the last check.
    pub latest: Option<String>,
    /// npm's `stable` at the last check.
    pub stable: Option<String>,
    /// When the registry was last asked (RFC 3339).
    pub checked_at: Option<String>,
    /// A check runs now.
    pub checking: bool,
    /// What the last check did: `installed`, `up_to_date`, `refused` or `failed`; none before
    /// the first, or while nothing is checked (`off`).
    pub result: Option<String>,
    /// The same in one sentence.
    pub result_text: Option<String>,
    /// Yulius's default model: the one a release must know.
    pub default_model: String,
    /// Whether the version in use could be read for the models it knows.
    pub models_read: bool,
    /// The offered models the version in use does not know, the default first.
    pub unknown_models: Vec<String>,
    /// What is wrong while it does not know the default model.
    pub problem: Option<String>,
    /// And the fix.
    pub fix: Option<String>,
    /// The releases the last check refused, in the order tried.
    pub refused: Vec<ClaudeCodeRefused>,
    /// Tasks whose running session still runs an older Claude Code (SPEC-461 CCM-013): each
    /// moves when its turn ends, or at its next start.
    pub behind: Vec<ClaudeCodeBehind>,
    /// Why running tasks are no longer moved to the version in use, when a session could not
    /// continue on it.
    pub moves_stopped: Option<String>,
}

/// A task whose running session runs an older Claude Code than runs get now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeBehind {
    /// Its sandbox's slug.
    pub sandbox: String,
    /// Its number in the sandbox.
    pub number: u32,
    /// Its title.
    pub title: String,
    /// The Claude Code version its session runs.
    pub version: String,
}

/// A Claude Code release that was not taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeRefused {
    /// The release.
    pub version: String,
    /// The offered models it does not know, the default first.
    pub missing: Vec<String>,
}

/// `PUT /api/admin/claude-code/channel` (SPEC-461 CCM-009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetClaudeChannel {
    /// `latest`, `stable` or `off`; `null` follows the server's configuration again. The field
    /// must be there: a body without it (a misspelt name) changes nothing.
    #[serde(deserialize_with = "Option::deserialize")]
    pub channel: Option<String>,
}

/// `GET /api/admin/sandbox-docker` (superadmin; SPEC-203 HST-014): whether isolated sandboxes
/// get Docker on this server, why not and the fix, in `yulius doctor`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxDockerView {
    /// Sandboxes get an engine (or Docker is deliberately off).
    pub ok: bool,
    /// The verdict: what holds, or what is missing and the fix.
    pub detail: String,
    /// Sandboxes are isolated on this server (without isolation there is no Docker).
    pub isolated: bool,
    /// The current base of sandboxes' systems.
    pub base: Option<SandboxBaseView>,
    /// The host has `dockerd` (new bases copy it).
    pub host_dockerd: bool,
    /// A base build that runs or waits.
    pub rebuild: Option<BaseRebuildView>,
    /// Sandboxes that run Docker installed in them instead of their base's (HST-017).
    pub own_engines: Vec<OwnEngineView>,
    /// How such a sandbox goes back to Docker's packages.
    pub own_engine_fix: String,
    /// The last update's re-check of the host (HST-012).
    pub host_check: Option<crate::update::HostCheckReport>,
    /// The last smoke test run from here (HST-015).
    pub smoke: Option<DockerSmokeView>,
}

/// A base of sandboxes' systems.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxBaseView {
    /// `base-<n>`.
    pub id: String,
    /// It has `dockerd`.
    pub dockerd: bool,
}

/// A base build that runs or waits for the host to settle (RFC 3339 times).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseRebuildView {
    /// A build runs, since then.
    pub building_since: Option<String>,
    /// The host changed then; the build waits until it has been unchanged for 10 minutes.
    pub pending_since: Option<String>,
    /// When that build starts.
    pub starts_at: Option<String>,
    /// When the new base should be ready (from the last build's duration), when known.
    pub ready_by: Option<String>,
    /// How long the last build took.
    pub last_build_secs: Option<u64>,
    /// The last build's error.
    pub error: Option<String>,
}

/// A sandbox on its own Docker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnEngineView {
    /// The sandbox's id.
    pub sandbox: String,
    /// Its name, when known.
    pub name: Option<String>,
    /// The `dockerd` its engine runs (in its system).
    pub dockerd: String,
}

/// One check of the Docker smoke test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerSmokeCheck {
    /// What was checked (`yulius doctor --docker-smoke`'s names).
    pub name: String,
    /// It held.
    pub ok: bool,
    /// What was seen.
    pub detail: String,
}

/// A smoke test started from Settings → Instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerSmokeView {
    /// `running`, `passed` or `failed`.
    pub state: String,
    /// Who started it.
    pub started_by: String,
    /// When (RFC 3339).
    pub started_at: String,
    /// When it ended.
    pub finished_at: Option<String>,
    /// Each check as it ended.
    pub checks: Vec<DockerSmokeCheck>,
    /// Why it could not run or did not finish.
    pub error: Option<String>,
}
