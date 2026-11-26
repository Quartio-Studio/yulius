//! Yulius-run CI (SPEC-001 DLV-006, SPEC-002 ADR-21) and repository hygiene (DLV-007).
//!
//! Yulius is the projects' only CI (F 2026-10-10: no GitHub Actions; "local blacksmith
//! instance"). For every sandbox with CI configured — the profile's `gate`, else `[[ci.jobs]]`
//! or `[commands] gate` of `yulius.toml` on the integration branch — it tests each open pull
//! request head into the integration branch and the integration and production heads, once per
//! commit, and publishes the commit status `yulius/ci`. A pull request into an integration
//! branch without CI is tested with its own configuration (bootstrap, so the onboarding pull
//! request that replaces GitHub Actions is tested by what it adds).
//! A playground (SPEC-099), which has no GitHub, gets its integration head tested and, when a
//! task delivers, that task's head merged into it (`task/<n>`, SPEC-163); nothing is published.
//!
//! Ownership and bounds (RUN-001..003):
//! - the **sync loop** (every [`CiSettings::poll`]) lists open pull requests, fetches each
//!   sandbox clone and plans with [`yulius_domain::ci::plan`]: queues new heads and cancels runs whose
//!   reference moved (superseded) or disappeared;
//! - the **dispatcher** starts the oldest queued run of each sandbox that has none executing
//!   (up to [`CiSettings::lanes`] runs per sandbox at a time, each lane with its own persistent
//!   job checkouts);
//! - a run's jobs run in parallel, each holding one of [`CiSettings::max_jobs`] instance-wide
//!   slots; a job runs through the process host ([`crate::ports::session::SessionHost`]) inside
//!   the sandbox's containment, with no secret in its environment (only the placeholders and
//!   egress proxy of the agent secrets its integration branch entry grants, SPEC-142), for at most
//!   its `timeout_minutes`, else [`CiSettings::job_timeout`], counted from its process's start
//!   (SPEC-435); an `exclusive` job first waits for its turn, one run of it per sandbox at a time;
//!   cancellation and timeouts kill the job's process group, and
//!   the host is terminated when the exit is not seen within [`KILL_GRACE`];
//! - output is the job's log file, read while it runs, kept as the last [`OUTPUT_BYTES`]
//!   (escape sequences removed); a log beyond [`MAX_LOG_FILE_BYTES`] ends the job;
//! - retention: [`KEEP_RUNS`] runs per sandbox and [`RETAIN_MS`]; commit statuses are retried
//!   [`STATUS_MAX_ATTEMPTS`] times within a day.
//!
//! Restart: runs recorded as running lose their executor; their hosts are terminated and they
//! go back to the queue (a run is repeatable).

pub(crate) mod brokered;
pub mod cd;
mod disk_wait;
mod environment;
mod exec;
mod watch_run;
pub(crate) use watch_run::WatchRunOutput;
mod findings;
mod flaky;
mod gate_history;
pub use flaky::Noted;
pub use gate_history::{GateJob, GateRead, GateVerdict, RepairRecipient};
pub mod hygiene;
mod opening;
pub use opening::PullOpening;
pub(crate) mod playground;
mod rollback;
mod slots;
pub(crate) mod staging;
mod tools;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{Notify, watch};
use yulius_contracts::api::ci::{CiGate, CiJob, CiJobSpec, CiOverview, CiRun};
use yulius_domain::ci::{
    self as dom, CiRef, Gate, GateResult, GateSource, Job, RunFacts, RunState, RunSummary, Target,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::resources::Need;
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::delivery::{delivery_profile, profile_gate};
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::{NewCommitStatus, STATUS_PERMISSION_MISSING};

/// Output kept per job (the tail).
pub const OUTPUT_BYTES: usize = 512 * 1024;
/// A job whose log grows beyond this is ended (runaway output would fill the disk).
pub const MAX_LOG_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Runs listed per sandbox.
pub const RUN_LIST_LIMIT: u32 = 30;
/// Runs kept per sandbox.
pub const KEEP_RUNS: u32 = 50;
/// Runs older than this are deleted.
pub const RETAIN_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// Attempts to publish one commit status.
pub const STATUS_MAX_ATTEMPTS: u32 = 5;
/// Open pull requests tested per sandbox.
pub const MAX_PULLS_TESTED: usize = 30;
/// Time to see a killed job's exit before its host is terminated.
pub const KILL_GRACE: Duration = Duration::from_secs(10);
/// GitHub Actions workflow files listed.
pub const MAX_WORKFLOW_FILES: usize = 20;
/// Where workflows live.
pub const WORKFLOWS_DIR: &str = ".github/workflows";
/// Why a playground's run has no commit status (SPEC-163 PCI-005).
pub const PLAYGROUND_STATUS_NOTE: &str = "not published: a playground has no GitHub repository";
const GATE_CACHE_MS: i64 = 60_000;
const STATUS_RETRY_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const PLAN_RUNS: u32 = 200;
const DISPATCH_TICK: Duration = Duration::from_secs(15);
const PRUNE_EVERY_MS: i64 = 60 * 60 * 1000;

/// CI settings of the instance (`yulius serve` flags; SPEC-002 §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiSettings {
    /// Run CI at all.
    pub enabled: bool,
    /// Jobs running at once on this machine (all sandboxes).
    pub max_jobs: u32,
    /// Runs of one sandbox executing at once, each in its own job checkouts (F 2026-10-12: one
    /// run at a time made parallel tasks wait behind each other for hours).
    pub lanes: u32,
    /// Wall-clock limit of one job's run, from its process's start, unless its `timeout_minutes`
    /// says otherwise (SPEC-435).
    pub job_timeout: Duration,
    /// How often each sandbox is checked for new heads.
    pub poll: Duration,
    /// Bytes a CD or environment run's files and cache may use (SPEC-300, ADR-0216).
    pub run_disk_max: u64,
    /// The longest a job waits for network room before its second run after a full address
    /// pool; past it, it ends error for the infrastructure (SPEC-497 OFC-001).
    pub rerun_network_wait: Duration,
}

impl Default for CiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_jobs: 4,
            lanes: 2,
            job_timeout: Duration::from_secs(30 * 60),
            poll: Duration::from_secs(60),
            run_disk_max: 20_000_000_000,
            rerun_network_wait: Duration::from_secs(10 * 60),
        }
    }
}

/// A Vidoc finding outside a change that Yulius CI filed as an issue (SPEC-217 VSI-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledFinding {
    /// The scanner's fingerprint.
    pub fingerprint: String,
    /// The file it is in.
    pub file: String,
    /// The fingerprint names the text of a line of `file` (else the finding's title).
    pub anchored: bool,
    /// Its issue.
    pub issue: u64,
    /// Yulius closed the issue: the code is gone from the integration branch.
    pub resolved: bool,
}

/// One CI run (one commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRunRecord {
    /// Id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// What it tests.
    pub reference: CiRef,
    /// Commit.
    pub sha: String,
    /// Where its configuration came from.
    pub source: GateSource,
    /// State (from its jobs).
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// `yulius` or a login / `task:<n>`.
    pub requested_by: String,
    /// The run state whose commit status GitHub last accepted.
    pub status_for: Option<RunState>,
    /// Why the last publication failed.
    pub status_note: Option<String>,
    /// Publication attempts.
    pub status_attempts: u32,
    /// Queued at.
    pub queued_at: Timestamp,
    /// Started at.
    pub started_at: Option<Timestamp>,
    /// Ended at.
    pub ended_at: Option<Timestamp>,
}

/// One job of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiJobRecord {
    /// Id.
    pub id: String,
    /// Run.
    pub run_id: String,
    /// Position.
    pub ordinal: u32,
    /// Display name.
    pub name: String,
    /// Definition name (checkout and cache).
    pub key: String,
    /// Shell text.
    pub command: String,
    /// Matrix value.
    pub matrix: Option<String>,
    /// State.
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Output tail (empty unless read with outputs).
    pub output: String,
    /// Its failing tests' output, kept from its whole output (SPEC-145; read with outputs).
    pub failures: Option<String>,
    /// Started at.
    pub started_at: Option<Timestamp>,
    /// Ended at.
    pub ended_at: Option<Timestamp>,
    /// The CPUs and memory it declared (SPEC-082 RES-002).
    pub need: Need,
    /// Its own time limit in minutes (SPEC-435 JTL-003); `None`: the instance's.
    pub timeout_minutes: Option<u32>,
    /// One run of it at a time in the sandbox (SPEC-435 JTL-001).
    pub exclusive: bool,
    /// How long it waited before it started (SPEC-435 JTL-004); `None` until it started.
    pub waited_ms: Option<i64>,
}

/// A commit supplied by the host, or user input that may abbreviate one.
#[derive(Debug, Clone, Copy)]
pub enum PullHead<'a> {
    /// Git or the hosting service supplied the full object name.
    Exact(&'a str),
    /// The wait endpoint accepts a full object name or a unique prefix.
    WaitArgument(&'a str),
}

/// Train and playground runs belong to their private queues. Other references
/// participate in the ordinary commit scheduler, including a staging candidate.
fn ordinary_scheduler_ref(reference: &CiRef) -> bool {
    !matches!(reference, CiRef::Train(_) | CiRef::Task(_))
}

fn integration_config_refusal(run: &CiRunRecord) -> bool {
    run.reason
        .as_deref()
        .is_some_and(|why| why.starts_with(DEV_INVALID_PREFIX))
}

/// Active work and a reference’s explicit cancellation survive a configuration refusal.
fn survives_config_refusal(state: RunState) -> bool {
    state.is_active() || state == RunState::Cancelled
}

/// Select the ordinary verdict from one commit’s runs, newest admission first.
/// A configuration refusal invalidates earlier completed verdicts at this commit.
#[must_use]
pub fn run_for_pull_head(runs: &[CiRunRecord], pr: u64) -> Option<&CiRunRecord> {
    let shared = |r: &&CiRunRecord| {
        ordinary_scheduler_ref(&r.reference) && yulius_domain::ci::covers_shared_commit(r.state)
    };
    if let Some(index) = runs
        .iter()
        .position(|r| r.reference == CiRef::PullRequest(pr))
    {
        let owner = &runs[index];
        let newer = &runs[..index];
        let invalidated = !survives_config_refusal(owner.state)
            && (integration_config_refusal(owner)
                || newer.iter().any(|run| {
                    ordinary_scheduler_ref(&run.reference) && integration_config_refusal(run)
                }));
        if invalidated && let Some(replacement) = newer.iter().find(shared) {
            return Some(replacement);
        }
        Some(owner)
    } else {
        runs.iter().find(shared)
    }
}

/// Test names retained by a cancelled sibling of a failed job (SPEC-566).
/// The reason is written by the CI runner; other cancellation causes carry no observation.
#[must_use]
pub fn sibling_failure_names(reason: Option<&str>, kept: &str) -> Vec<String> {
    match reason
        .and_then(|why| why.strip_prefix("stopped early: job "))
        .and_then(|why| why.strip_suffix(" failed"))
    {
        Some(name) if !name.is_empty() => yulius_domain::ci::failures::failed_test_names(kept),
        _ => Vec::new(),
    }
}

/// Fields set with a run transition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiRunUpdate {
    /// Reason.
    pub reason: Option<String>,
    /// Start.
    pub started_at: Option<Timestamp>,
    /// End.
    pub ended_at: Option<Timestamp>,
}

/// One line of a sandbox's tally of CI minutes (SPEC-497 OFC-003): a UTC day, how the runs
/// ended (`passed`, `failed`, `failed_outside`, `cancelled`), how many, their minutes start to end
/// and their jobs' time thrown away by infrastructure failures, in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiMinutesRow {
    /// `YYYY-MM-DD`.
    pub day: String,
    /// How the runs ended.
    pub outcome: String,
    /// Runs.
    pub runs: i64,
    /// Their time, start to end.
    pub run_ms: i64,
    /// Their jobs' thrown-away time.
    pub rerun_ms: i64,
}

/// A test quarantined as flaky in a sandbox (SPEC-497 OFC-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantined {
    /// Its sandbox.
    pub sandbox_id: SandboxId,
    /// Its name as the failing tests' blocks name it.
    pub test: String,
    /// The job it failed in.
    pub job_key: String,
    /// The issue filed for it, if one could be.
    pub issue: Option<(u64, String)>,
    /// When it was quarantined.
    pub since: Timestamp,
}

/// Fields set with a job transition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiJobUpdate {
    /// Reason.
    pub reason: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Output.
    pub output: Option<String>,
    /// Its failing tests' output (SPEC-145).
    pub failures: Option<String>,
    /// Start.
    pub started_at: Option<Timestamp>,
    /// End.
    pub ended_at: Option<Timestamp>,
    /// What it used, with its end when its process host measured it (SPEC-154).
    pub usage: Option<yulius_domain::stages::ResourceUse>,
    /// How long it waited before it started, with its start (SPEC-435).
    pub waited_ms: Option<i64>,
    /// With its end, after an infrastructure failure (SPEC-497 OFC-001): its kind
    /// ([`yulius_domain::ci::infra::Infra::as_str`]), the milliseconds of the runs thrown away,
    /// and whether its end itself was the infrastructure's (it met one again).
    pub infra: Option<(&'static str, i64, bool)>,
}

/// The CI configuration of a sandbox as last read.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateView {
    read_at: i64,
    base_sha: Option<String>,
    /// The CI in force, if any.
    pub(crate) gate: Option<Gate>,
    /// Why the integration branch's configuration is unusable.
    pub(crate) problem: Option<String>,
    /// While it is: the newest first-parent commit of the integration branch whose
    /// configuration is valid, whose gate is the one in force (SPEC-504 CFG-003).
    pub(crate) fallback: Option<String>,
    /// While it is: whether looking for that commit failed (Git), so it is not known whether
    /// one exists.
    pub(crate) fallback_unknown: bool,
    /// GitHub Actions workflow files on the integration branch.
    pub(crate) workflows: Vec<String>,
}

/// A claim of a sandbox's CI checkouts ([`App::ci_claim_owned`]): dropping it ends the claim and
/// wakes the dispatcher, as [`App::ci_release`] does.
pub(crate) struct CiClaim {
    busy: Arc<Mutex<HashSet<String>>>,
    wake: Arc<Notify>,
    sandbox: String,
}

impl Drop for CiClaim {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.busy.lock() {
            busy.remove(&self.sandbox);
        }
        self.wake.notify_one();
    }
}

/// The minute of a reading of the host for CI and the host's CPU ticks then, `(busy, total)`
/// (SPEC-497).
pub(crate) type LoadReading = (i64, Option<(u64, u64)>);

/// A REST inventory can wait on GitHub while a signed opening schedules its own head.
/// Full inventories stay ordered, and every writer of an ordinary scheduling plan is serialized.
#[derive(Default)]
struct SandboxSync {
    inventory: tokio::sync::Mutex<()>,
    // Only observations since the current REST inventory started are process-local.
    schedule: tokio::sync::Mutex<HashMap<u64, String>>,
}

/// Process-local CI state.
#[derive(Default)]
pub struct CiState {
    slots: OnceLock<Arc<slots::JobSlots>>,
    /// The CPUs and MiB of memory CI jobs share, sized from the sandbox capacity at the first
    /// job (SPEC-082 RES-003).
    pub(crate) budget: OnceLock<Arc<crate::resources::CiAdmission>>,
    /// CI jobs running per sandbox and the CPUs they hold (SPEC-362 CSW-002, SPEC-497 HBA-004),
    /// told to the isolation ([`JobWeight`]).
    running_jobs: Mutex<HashMap<SandboxId, (u32, u32)>>,
    /// Executing runs: their cancellation (the reason, once cancelled).
    executing: Mutex<HashMap<String, watch::Sender<Option<String>>>>,
    /// Lanes in use (`<sandbox>/<lane>` for an executing run) and whole-sandbox claims
    /// (`<sandbox>`, disk upkeep). Shared with [`CiClaim`], which ends a claim where it is
    /// dropped.
    busy: Arc<Mutex<HashSet<String>>>,
    gates: Mutex<HashMap<String, GateView>>,
    /// Whether each sandbox's CI has no gate because of its configuration, as last recorded
    /// (SPEC-504 CFG-004); a sandbox not in it is read from the audit first.
    config_blocked: Mutex<HashMap<String, bool>>,
    pub(crate) hygiene: Mutex<HashMap<String, yulius_contracts::api::ci::HygieneReport>>,
    /// Each sandbox's last cleanup (SPEC-269 HSC-002), shown on every report.
    pub(crate) hygiene_cleanups:
        Mutex<HashMap<String, yulius_contracts::api::ci::HygieneLastCleanup>>,
    /// When the automatic hygiene round runs next (ms), once it is scheduled.
    pub(crate) hygiene_next: Mutex<Option<i64>>,
    /// One hygiene cleanup per sandbox at a time (the round and a person's Clean up).
    pub(crate) hygiene_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One CI sync of a sandbox at a time (the 60 s loop, a person's sync, a review's start):
    /// each reads the runs before it queues, so two at once would queue one head twice.
    sync_locks: Mutex<HashMap<String, Arc<SandboxSync>>>,
    /// One quarantine of a sandbox decided at a time (SPEC-497 OFC-002): two jobs failing the
    /// same test at once would each file an issue for it (review round 1); per sandbox, so a
    /// slow GitHub holds only that project's job ends (review round 2).
    pub(crate) quarantine_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// The reference of the last run each lane (`<sandbox>/<lane>`) took (SPEC-497 SHR-002).
    pub(crate) lane_runs: Mutex<HashMap<String, CiRef>>,
    /// What each executing run's lane last built, for its jobs' output (SPEC-497 SHR-002).
    pub(crate) lane_notes: Mutex<HashMap<String, String>>,
    /// Sandboxes with a run that passed when it was queued (every job skipped for its
    /// `paths`): the dispatcher starts their merge pass, as a run's end does.
    passed_when_queued: Mutex<Vec<SandboxId>>,
    wake: Arc<Notify>,
    started: AtomicBool,
    last_prune: Mutex<i64>,
    /// The minute of the last reading of the host for CI and its CPU ticks then (SPEC-497).
    pub(crate) load: Mutex<Option<LoadReading>>,
    /// Sandboxes whose runs wait for disk (SPEC-059 CI-301).
    disk_waits: Mutex<HashMap<String, disk_wait::DiskWait>>,
    /// One pass deciding a sandbox's armed merges (and its merge train) at a time (SPEC-084
    /// TRN-010): passes started by runs that end together wait for each other.
    pub(crate) armed_passes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// The turn of each `exclusive` job (`<sandbox>/<job name>`; SPEC-435 JTL-001): its runs
    /// take it in the order they asked; an entry nobody holds or waits for goes ([`TurnEntry`]).
    turns: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// An `exclusive` job's claim on its turn's entry (SPEC-435): dropped after the job's turn and
/// its wait, it forgets the entry when nobody else holds or waits for it.
pub(crate) struct TurnEntry {
    app: Arc<App>,
    key: String,
}

impl Drop for TurnEntry {
    fn drop(&mut self) {
        let mut turns = self
            .app
            .ci
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        forget_idle_turn(&mut turns, &self.key);
    }
}

/// Removes the turn of `key` when the map holds its only handle. Every holder and waiter took its
/// handle under the map's lock, which the caller holds, so none can appear meanwhile.
fn forget_idle_turn(turns: &mut HashMap<String, Arc<tokio::sync::Mutex<()>>>, key: &str) {
    if turns
        .get(key)
        .is_some_and(|turn| Arc::strong_count(turn) == 1)
    {
        turns.remove(key);
    }
}

/// A running CI job's weight on its sandbox (SPEC-362 CSW-002): counted from its start until it
/// is dropped, on every end, so the isolation weighs the sandbox on the host by its running jobs.
pub(crate) struct JobWeight {
    app: Arc<App>,
    sandbox: SandboxId,
    cpus: u32,
}

impl JobWeight {
    pub(crate) fn new(app: Arc<App>, sandbox: &SandboxId, cpus: u32) -> Self {
        app.ci_weigh(sandbox, cpus, true);
        Self {
            app,
            sandbox: sandbox.clone(),
            cpus,
        }
    }
}

impl Drop for JobWeight {
    fn drop(&mut self) {
        self.app.ci_weigh(&self.sandbox, self.cpus, false);
    }
}

pub(crate) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// What a run judged by the integration branch's last valid gate says (SPEC-504 CFG-006).
pub const LAST_VALID_NOTE: &str = "judged by the integration branch's last valid yulius.toml: \
     its own is invalid, and that gate runs until a fix merges";

/// How a run refused for the integration branch's invalid `yulius.toml` begins its reason: it
/// is queued again once that file is valid (SPEC-504 CFG-002).
pub const DEV_INVALID_PREFIX: &str = "yulius.toml is invalid on ";

/// The job of a run refused for its `yulius.toml` (SPEC-504 CFG-002).
pub const CONFIG_JOB: &str = "yulius.toml";

/// First-parent commits of the integration branch read back for its last valid configuration
/// (SPEC-504 CFG-003).
const LAST_VALID_BACK: usize = 100;

/// The ordinary scheduler’s view of bounded run history, newest admission first.
fn ordinary_run_facts(runs: &[CiRunRecord], dev_valid: bool) -> Vec<RunFacts> {
    // A merge train's run is the train's own (SPEC-084), a playground task's its delivery's
    // (SPEC-163): they start and cancel them.
    // A head refused only because the integration branch's file was invalid is tested again
    // once it is valid (review round 1): its refusal is no verdict then.
    let mut invalidated = HashSet::new();
    runs.iter()
        // A candidate run (SPEC-575) stays in the facts: it tests its commit once for the
        // branch too, and `plan` never supersedes it.
        .filter(|run| ordinary_scheduler_ref(&run.reference))
        .filter(|run| {
            if dev_valid && integration_config_refusal(run) {
                // From this refusal back, completed verdicts at the SHA predate
                // the configuration failure. A newer run already covers the repair.
                invalidated.insert(run.sha.as_str());
                return false;
            }
            !invalidated.contains(run.sha.as_str()) || survives_config_refusal(run.state)
        })
        .map(|run| RunFacts {
            id: run.id.clone(),
            reference: run.reference.clone(),
            sha: run.sha.clone(),
            state: run.state,
        })
        .collect()
}

/// Whether anything of `sandbox` is in `busy`: a lane (`<sandbox>/<n>`) or a claim.
fn busy_in(busy: &HashSet<String>, sandbox: &str) -> bool {
    busy.iter().any(|key| {
        key == sandbox
            || key
                .strip_prefix(sandbox)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// A free lane of `sandbox`, unless the sandbox is claimed or every lane runs.
fn free_lane(busy: &HashSet<String>, sandbox: &str, lanes: u32) -> Option<u32> {
    if busy.contains(sandbox) {
        return None;
    }
    (0..lanes.max(1)).find(|lane| !busy.contains(&format!("{sandbox}/{lane}")))
}

/// [`free_lane`], and marks it.
#[cfg(test)]
fn take_lane(busy: &mut HashSet<String>, sandbox: &str, lanes: u32) -> Option<u32> {
    take_lane_preferring(busy, sandbox, lanes, &[])
}

/// [`take_lane`], the first free lane of `preferred` first (SPEC-497 SHR-002: the lane whose
/// checkouts last built what the run builds on).
fn take_lane_preferring(
    busy: &mut HashSet<String>,
    sandbox: &str,
    lanes: u32,
    preferred: &[u32],
) -> Option<u32> {
    free_lane(busy, sandbox, lanes)?;
    let lane = preferred
        .iter()
        .copied()
        .find(|lane| *lane < lanes.max(1) && !busy.contains(&format!("{sandbox}/{lane}")))
        .or_else(|| free_lane(busy, sandbox, lanes))?;
    busy.insert(format!("{sandbox}/{lane}"));
    Some(lane)
}

/// What a run's jobs say about the lane it took (SPEC-497 SHR-002): the lane, the run that last
/// took it (`before`) and whether it was one this run preferred, first choice or not. The
/// dispatcher knows the lane's last run, not that it built (it may have been cancelled before
/// its jobs started): "taken" (review round 1).
fn lane_note(lane: u32, before: Option<&str>, preferred: &[u32]) -> String {
    match before {
        Some(key) if preferred.contains(&lane) => {
            format!("lane {lane}, last taken by {key}, which this run builds on")
        }
        Some(key) => format!("lane {lane}, last taken by {key}"),
        None => format!("lane {lane}"),
    }
}

/// Whether `ran` is the record of `job` as it is defined now: the same key, matrix value and
/// command, and the same need, time limit and `exclusive` (SPEC-504 review rounds 3 and 4).
fn ran_as(ran: &CiJobRecord, job: &Job) -> bool {
    ran.key == job.key
        && ran.matrix == job.matrix
        && ran.command == job.command
        && ran.need == job.need
        && ran.timeout_minutes == job.timeout_minutes
        && ran.exclusive == job.exclusive
}

/// Whether the recorded jobs cover this reference's gate, including jobs a PR may skip.
fn jobs_cover_target(reference: &CiRef, gate: &Gate, jobs: &[CiJobRecord]) -> bool {
    let branch = matches!(
        reference,
        CiRef::Branch(_) | CiRef::Train(_) | CiRef::Candidate(_)
    );
    let skipped_a_paths_job = gate.jobs.iter().any(|job| {
        !job.paths.is_empty()
            && jobs
                .iter()
                .any(|t| t.key == job.key && t.matrix == job.matrix && t.state == RunState::Skipped)
    });
    // Likewise a source that skipped a job as part of the full suite (SPEC-575 MFS-002)
    // never ran it, whatever the gate says of the job now (a change may have just turned
    // the move off, review round 1): no source for a run of the merged revision.
    let merged = matches!(reference, CiRef::Branch(_) | CiRef::Candidate(_));
    let skipped_the_full_suite = jobs
        .iter()
        .any(|t| dom::skipped_after_merge(t.state, t.reason.as_deref()));
    // A train still requires its path-scoped jobs to have passed. Branches and merge
    // candidates use the execution evidence below, including an actual skip exit.
    if matches!(reference, CiRef::Train(_)) && skipped_a_paths_job
        || merged && skipped_the_full_suite
    {
        return false;
    }
    // Every job of the gate ran there as it is defined now: a pull request judged by the
    // jobs its merge then changed never ran the new ones (SPEC-504, review round 1).
    let all_ran = gate.jobs.iter().all(|job| {
        jobs.iter().any(|ran| {
            ran_as(ran, job)
                && (!merged
                    || match ran.state {
                        RunState::Passed => true,
                        RunState::Skipped => {
                            ran.exit_code == Some(dom::SKIP_EXIT_CODE)
                                && ran
                                    .reason
                                    .as_deref()
                                    .is_none_or(|why| why.starts_with(dom::EXECUTED_SKIP_REASON))
                        }
                        _ => false,
                    })
        })
    });
    !branch || all_ran
}

/// Whether `run` may lend its verdict to `target`, whose tree it tested (the trees are compared
/// apart): a passed run, not a train's step verdict, and another commit, unless `target` is a
/// merge candidate formed again on that very commit (slice D review: within one second the
/// same members on the same base give the same commit).
fn reuse_source(run: &CiRunRecord, target: &Target) -> bool {
    run.state == RunState::Passed
        && (run.sha != target.sha || matches!(target.reference, CiRef::Train(_)))
        && run.requested_by != crate::merge_train::TRAIN_VERDICT_BY
}

/// The lanes whose checkouts last built what `reference` builds on, best first (SPEC-497
/// SHR-002): for a pull request's run, the lane of its own last run (its previous head); for a
/// merge candidate's, the lanes of its members' own runs, the last member first, then of an
/// earlier candidate sharing a member. `last` maps each lane to the reference of its last run.
fn preferred_lanes(reference: &CiRef, last: &[(u32, CiRef)]) -> Vec<u32> {
    let lanes_of = |wanted: &dyn Fn(&CiRef) -> bool| -> Vec<u32> {
        last.iter()
            .filter(|(_, r)| wanted(r))
            .map(|(lane, _)| *lane)
            .collect()
    };
    let mut lanes = Vec::new();
    match reference {
        CiRef::PullRequest(n) => {
            lanes.extend(lanes_of(&|r| *r == CiRef::PullRequest(*n)));
        }
        CiRef::Train(members) => {
            for member in members.iter().rev() {
                lanes.extend(lanes_of(&|r| *r == CiRef::PullRequest(*member)));
            }
            lanes.extend(lanes_of(&|r| {
                matches!(r, CiRef::Train(others) if others.iter().any(|o| members.contains(o)))
            }));
        }
        CiRef::Branch(_) | CiRef::Task(_) | CiRef::Candidate(_) => {}
    }
    let mut seen = Vec::new();
    lanes.retain(|lane| {
        let new = !seen.contains(lane);
        seen.push(*lane);
        new
    });
    lanes
}

/// Takes a lane of `sandbox` for `run_id` and registers its executor, under the caller's locks;
/// `None` for a run that already has an executor (#738): a dispatch can read it still queued
/// before that executor marks it running, and a second one would replace its cancellation.
fn reserve_run<T>(
    busy: &mut HashSet<String>,
    executing: &mut HashMap<String, T>,
    (run_id, sandbox, lanes): (&str, &str, u32),
    preferred: &[u32],
    executor: T,
) -> Option<u32> {
    if executing.contains_key(run_id) {
        return None;
    }
    let lane = take_lane_preferring(busy, sandbox, lanes, preferred)?;
    executing.insert(run_id.to_owned(), executor);
    Some(lane)
}

fn job_view(job: &CiJobRecord, with_output: bool) -> CiJob {
    CiJob {
        id: job.id.clone(),
        name: job.name.clone(),
        command: job.command.clone(),
        matrix: job.matrix.clone(),
        state: job.state.as_str().to_owned(),
        reason: job.reason.clone(),
        exit_code: job.exit_code,
        duration_ms: job
            .started_at
            .zip(job.ended_at)
            .map(|(start, end)| end.millis_since(start)),
        waited_ms: job.waited_ms,
        output: with_output.then(|| job.output.clone()),
        failures: with_output.then(|| job.failures.clone()).flatten(),
    }
}

fn status_word(run: &CiRunRecord) -> String {
    if run.status_for == Some(run.state) {
        "published".to_owned()
    } else if let Some(note) = &run.status_note {
        note.clone()
    } else {
        "pending".to_owned()
    }
}

/// The API form of a run.
#[must_use]
pub fn run_view(run: &CiRunRecord, jobs: &[CiJobRecord], with_output: bool) -> CiRun {
    CiRun {
        id: run.id.clone(),
        reference: run.reference.key(),
        label: run.reference.label(),
        pr_number: match run.reference {
            CiRef::PullRequest(n) => Some(n),
            CiRef::Branch(_) | CiRef::Train(_) | CiRef::Task(_) | CiRef::Candidate(_) => None,
        },
        sha: run.sha.clone(),
        state: run.state.as_str().to_owned(),
        source: run.source.as_str().to_owned(),
        // A run judged by the last valid gate says so (SPEC-504 CFG-006).
        reason: run
            .reason
            .clone()
            .or_else(|| (run.source == GateSource::LastValid).then(|| LAST_VALID_NOTE.to_owned())),
        jobs: jobs.iter().map(|job| job_view(job, with_output)).collect(),
        queued_at: rfc3339(run.queued_at),
        started_at: run.started_at.map(rfc3339),
        ended_at: run.ended_at.map(rfc3339),
        duration_ms: run
            .started_at
            .zip(run.ended_at)
            .map(|(start, end)| end.millis_since(start)),
        requested_by: run.requested_by.clone(),
        status: status_word(run),
    }
}

fn gate_view(gate: &Gate) -> CiGate {
    CiGate {
        jobs: gate
            .jobs
            .iter()
            .map(|job| CiJobSpec {
                name: job.name.clone(),
                command: job.command.clone(),
                matrix: job.matrix.clone(),
                timeout_minutes: job.timeout_minutes,
                exclusive: job.exclusive,
            })
            .collect(),
        source: gate.source.as_str().to_owned(),
    }
}

impl App {
    fn ci_run_view(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        jobs: &[CiJobRecord],
        output: bool,
    ) -> CiRun {
        let mut view = run_view(run, jobs, output);
        if !sandbox.is_playground()
            && run.status_for != Some(run.state)
            && run.status_attempts < STATUS_MAX_ATTEMPTS
            && !matches!(run.reference, CiRef::Train(_))
            && let Some(wait) = self.github_wait()
        {
            view.status = wait;
        }
        view
    }

    /// The turn of `job` in `sandbox`, for an `exclusive` job (SPEC-435 JTL-001), and the entry
    /// that forgets it once nobody holds or waits for it: drop that after the turn.
    fn ci_turn(
        self: &Arc<Self>,
        sandbox: &SandboxId,
        job: &str,
    ) -> (Arc<tokio::sync::Mutex<()>>, TurnEntry) {
        let key = format!("{}/{job}", sandbox.as_str());
        let turn = self
            .ci
            .turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key.clone())
            .or_default()
            .clone();
        (
            turn,
            TurnEntry {
                app: Arc::clone(self),
                key,
            },
        )
    }

    /// The time limit of `job`'s run: its own `timeout_minutes`, else the instance's (SPEC-435).
    fn ci_job_limit(&self, job: &CiJobRecord) -> Duration {
        job.timeout_minutes
            .map_or(self.config.ci.job_timeout, |minutes| {
                Duration::from_secs(u64::from(minutes) * 60)
            })
    }

    fn ci_slots(&self) -> Arc<slots::JobSlots> {
        self.ci
            .slots
            .get_or_init(|| slots::JobSlots::new(self.ci_job_slots()))
            .clone()
    }

    pub(crate) fn ci_job_slots(&self) -> usize {
        usize::try_from(self.ci_max_jobs()).unwrap_or(1)
    }

    /// Makes the job slots as many as the limit in force says (SPEC-459 HSL-006): more at once,
    /// fewer as running jobs end.
    pub(crate) fn ci_resize_slots(&self) {
        self.ci_slots().resize(self.ci_job_slots());
        // Waiting jobs decide again at once, not at their next look (review of the CI
        // admission fix).
        self.ci_admission_changed();
    }

    /// Counts one more (`started`) or one fewer running CI job of `sandbox` and tells the
    /// isolation the new count, under the lock so the last count told is the current one. The
    /// isolation writes one cgroup file there and may wait for a preparation of the sandbox's
    /// groups (moving at most 4096 processes): bounded, and other jobs' starts and ends wait.
    fn ci_weigh(&self, sandbox: &SandboxId, cpus: u32, started: bool) {
        let mut running = self
            .ci
            .running_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (jobs, held) = running.entry(sandbox.clone()).or_insert((0, 0));
        if started {
            *jobs = jobs.saturating_add(1);
            *held = held.saturating_add(cpus);
        } else {
            *jobs = jobs.saturating_sub(1);
            *held = held.saturating_sub(cpus);
        }
        let (jobs, held) = (*jobs, *held);
        if jobs == 0 {
            running.remove(sandbox);
        }
        if let Some(isolation) = self.isolation.get() {
            isolation.ci_jobs_running(sandbox, jobs, held);
        }
    }

    /// The sandboxes with CI jobs running now.
    pub(crate) fn ci_sandboxes_running(&self) -> Vec<SandboxId> {
        self.ci
            .running_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    pub(crate) fn ci_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("ci")
    }

    /// Whether a run of `sandbox` is executing (in any lane) or its checkouts are claimed.
    pub(crate) fn ci_busy(&self, sandbox: &SandboxId) -> bool {
        self.ci
            .busy
            .lock()
            .map_or(true, |busy| busy_in(&busy, sandbox.as_str()))
    }

    /// Claims `sandbox`'s CI checkouts (the dispatcher starts no run there while claimed), for
    /// work that removes files in them (disk upkeep); `false` when a run executes or another
    /// claim holds them. End it with [`App::ci_release`].
    pub fn ci_claim(&self, sandbox: &SandboxId) -> bool {
        self.ci.busy.lock().is_ok_and(|mut busy| {
            !busy_in(&busy, sandbox.as_str()) && busy.insert(sandbox.as_str().to_owned())
        })
    }

    /// [`App::ci_claim`] as a value that ends the claim when it is dropped, wherever that is: a
    /// removal running on a blocking thread keeps the claim until it ends, also when the
    /// future that started it is dropped (review round 2 of SPEC-493 fix 1d). `None` when the
    /// claim was refused.
    pub(crate) fn ci_claim_owned(&self, sandbox: &SandboxId) -> Option<CiClaim> {
        self.ci_claim(sandbox).then(|| CiClaim {
            busy: Arc::clone(&self.ci.busy),
            wake: Arc::clone(&self.ci.wake),
            sandbox: sandbox.as_str().to_owned(),
        })
    }

    /// Ends a claim of [`App::ci_claim`] and lets the dispatcher start queued runs.
    pub fn ci_release(&self, sandbox: &SandboxId) {
        if let Ok(mut busy) = self.ci.busy.lock() {
            busy.remove(sandbox.as_str());
        }
        self.ci_wake();
    }

    /// Wakes the dispatcher (a run was queued or a runner freed).
    pub(crate) fn ci_wake(&self) {
        self.ci.wake.notify_one();
    }

    /// The CI jobs `yulius.toml` defines at `rev`: `Ok(empty)` without a file or CI, `Err`
    /// with the problem when the file is invalid.
    pub(crate) async fn ci_jobs_at(&self, repo: &Path, rev: &str) -> Result<Vec<Job>, String> {
        let text = match self.deps.git.show_file(repo, rev, "yulius.toml").await {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(Vec::new()),
            Err(err) => {
                return Err(format!(
                    "yulius.toml can't be read: {}",
                    err.public_message()
                ));
            }
        };
        if text.len() > MAX_CONFIG_BYTES {
            return Err("yulius.toml is larger than 64 KiB".into());
        }
        let config = parse_yulius_toml(&text).map_err(|err| err.to_string())?;
        let jobs = dom::project_jobs(config.commands.gate.as_deref(), &config.ci)
            .map_err(|e| e.to_string())?;
        // The full suite leaves the pull request only beside a shared staging target (SPEC-575
        // MFS-002); the whole file, its CD tables included, parsed above.
        let merge_first = yulius_domain::spec_format::parse_yulius_toml_cd(&text)
            .is_ok_and(|cd| cd.merge_first().is_some());
        Ok(dom::after_merge_in_force(jobs, merge_first))
    }

    /// The sandbox's CI as configured now (cached for a minute per integration head), with the
    /// built-in security job when the sandbox can reach Vidoc (SPEC-217 VDP-001).
    pub(crate) async fn ci_gate(&self, sandbox: &SandboxRecord) -> GateView {
        let mut view = self.ci_configured_gate(sandbox).await;
        view.gate = self.ci_with_security(sandbox, view.gate).await;
        view
    }

    /// `gate` with the built-in security job when the sandbox, not a playground, has
    /// `VIDOC_API_KEY` among its applied agent secrets (its own, or the instance's Vidoc key);
    /// a sandbox without CI gets none (SPEC-217 VDP-001).
    pub(crate) async fn ci_with_security(
        &self,
        sandbox: &SandboxRecord,
        gate: Option<Gate>,
    ) -> Option<Gate> {
        let gate = gate?;
        Some(if self.ci_scans(sandbox).await {
            dom::with_security_job(gate)
        } else {
            gate
        })
    }

    /// Whether the sandbox's CI runs the Vidoc scan: not a playground, its Vidoc switch and the
    /// instance's on (SPEC-338 VOF-002, SPEC-356 VIO-002), and `VIDOC_API_KEY` among its applied
    /// agent secrets (SPEC-217 VDP-001).
    pub(crate) async fn ci_scans(&self, sandbox: &SandboxRecord) -> bool {
        !sandbox.is_playground()
            && self.vidoc_scans_on(sandbox)
            && self
                .applied_agent_secrets(&sandbox.id, crate::vault::Holder::Nobody)
                .await
                .is_ok_and(|records| records.iter().any(|r| r.name == dom::VIDOC_SECRET))
    }

    async fn ci_configured_gate(&self, sandbox: &SandboxRecord) -> GateView {
        let now = self.now().0;
        let repo = self.sandbox_repo(&sandbox.id);
        let profile = delivery_profile(sandbox);
        let base = if repo.join(".git").exists() {
            self.deps
                .git
                .resolve_commit(&repo, &format!("origin/{}", profile.integration_branch))
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if let Ok(cache) = self.ci.gates.lock()
            && let Some(view) = cache.get(sandbox.id.as_str())
            && view.base_sha == base
            && now - view.read_at < GATE_CACHE_MS
        {
            return view.clone();
        }
        let profile_gate = profile_gate(sandbox);
        let mut view = GateView {
            read_at: now,
            base_sha: base.clone(),
            ..GateView::default()
        };
        let project = match &base {
            Some(sha) => match self.ci_jobs_at(&repo, sha).await {
                Ok(jobs) => jobs,
                Err(problem) => {
                    view.problem = Some(problem);
                    // The last valid gate runs until a fix merges (SPEC-504 CFG-003).
                    match self.ci_last_valid_jobs(&repo, sha).await {
                        Ok(Some((valid, jobs))) => {
                            view.fallback = Some(valid);
                            jobs
                        }
                        Ok(None) => Vec::new(),
                        Err(()) => {
                            view.fallback_unknown = true;
                            Vec::new()
                        }
                    }
                }
            },
            None => Vec::new(),
        };
        view.gate = dom::effective_gate(profile_gate.as_deref(), &project);
        // Every run of it says it runs the last valid gate (SPEC-504 CFG-006); a fallback that
        // gives no gate in force (none there, or a profile's gate instead) is no fallback.
        match view.gate.as_mut() {
            Some(gate) if view.fallback.is_some() && gate.source == GateSource::ProjectConfig => {
                gate.source = GateSource::LastValid;
            }
            _ => view.fallback = None,
        }
        if let Some(sha) = &base
            && let Ok(files) = self.deps.git.list_dir(&repo, sha, WORKFLOWS_DIR).await
        {
            view.workflows = files
                .into_iter()
                .filter(|f| f.ends_with(".yml") || f.ends_with(".yaml"))
                .take(MAX_WORKFLOW_FILES)
                .collect();
        }
        if let Ok(mut cache) = self.ci.gates.lock() {
            cache.insert(sandbox.id.as_str().to_owned(), view.clone());
        }
        view
    }

    /// The jobs of the newest first-parent ancestor of `sha` (at most [`LAST_VALID_BACK`] back)
    /// whose `yulius.toml` is valid, with that commit (SPEC-504 CFG-003): the gate that judged
    /// the integration branch before its configuration broke. `Ok(None)` when none is, `Err`
    /// when Git could not say.
    async fn ci_last_valid_jobs(
        &self,
        repo: &Path,
        sha: &str,
    ) -> Result<Option<(String, Vec<Job>)>, ()> {
        let mut at = sha.to_owned();
        for _ in 0..LAST_VALID_BACK {
            let parents = self.deps.git.parents(repo, &at).await.map_err(|_| ())?;
            let Some(parent) = parents.into_iter().next() else {
                return Ok(None);
            };
            if let Ok(jobs) = self.ci_jobs_at(repo, &parent).await {
                return Ok(Some((parent, jobs)));
            }
            at = parent;
        }
        Ok(None)
    }

    /// Records when `sandbox`'s CI starts and stops having no gate because of its configuration
    /// (SPEC-504 CFG-004): an audit entry `ci.config_blocked` with the problem, and
    /// `ci.config_unblocked` once a gate is in force again. The first look after a start reads
    /// the last entry, and a state is kept only once its entry is written, so a failed write is
    /// tried again at the next look.
    async fn ci_note_config_block(&self, sandbox: &SandboxRecord, view: &GateView) {
        use yulius_domain::merge_queue::{CONFIG_BLOCKED, CONFIG_UNBLOCKED};
        let blocked = view.gate.is_none() && view.problem.is_some();
        let known = self
            .ci
            .config_blocked
            .lock()
            .ok()
            .and_then(|map| map.get(sandbox.id.as_str()).copied());
        // First look since this server started: what the audit last said, so an episode that
        // ended while it was down is closed (review round 1).
        let was = match known {
            Some(was) => was,
            None => self
                .deps
                .store
                .audit_of_sandbox(
                    &sandbox.slug,
                    &[],
                    &[CONFIG_BLOCKED, CONFIG_UNBLOCKED],
                    None,
                    None,
                    20_000,
                )
                .await
                .ok()
                .and_then(|entries| {
                    entries
                        .into_iter()
                        .max_by_key(|(_, entry)| entry.at.0)
                        .map(|(_, entry)| entry.action == CONFIG_BLOCKED)
                })
                .unwrap_or(false),
        };
        if was == blocked {
            if let Ok(mut map) = self.ci.config_blocked.lock() {
                map.insert(sandbox.id.as_str().to_owned(), blocked);
            }
            return;
        }
        let (action, detail) = if blocked {
            (CONFIG_BLOCKED, view.problem.clone().unwrap_or_default())
        } else {
            (CONFIG_UNBLOCKED, String::new())
        };
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "yulius",
                action,
                Some(&sandbox.slug),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "recording CI's configuration state failed");
            return;
        }
        // Kept once written: a failed write is tried again at the next look (review round 2).
        if let Ok(mut map) = self.ci.config_blocked.lock() {
            map.insert(sandbox.id.as_str().to_owned(), blocked);
        }
    }

    /// Forgets the cached CI configuration (after a fetch or a profile change).
    pub(crate) fn ci_forget_gate(&self, sandbox: &SandboxId) {
        if let Ok(mut cache) = self.ci.gates.lock() {
            cache.remove(sandbox.as_str());
        }
    }

    /// The CI's verdict at `sha` for the merge gate: the latest run of that commit when there
    /// is one, else pending when Yulius CI will test it, else not configured (also when CI is
    /// off on this instance).
    pub async fn ci_gate_result(&self, sandbox: &SandboxRecord, sha: &str) -> GateResult {
        self.ci_gate_result_for_ref(sandbox, sha, None).await
    }

    /// An ordinary PR display uses its own run when several refs share a SHA.
    pub(crate) async fn ci_gate_result_for_ref(
        &self,
        sandbox: &SandboxRecord,
        sha: &str,
        reference: Option<&CiRef>,
    ) -> GateResult {
        self.ci_gate_select(
            sandbox,
            sha,
            self.deps.store.ci_runs_for_sha(&sandbox.id, sha),
            |runs| {
                runs.iter()
                    .find(|run| reference.is_none_or(|r| *r == run.reference))
                    .map(|run| run.state)
            },
        )
        .await
    }

    /// The run the queue uses to decide this PR at its exact head.
    pub(crate) async fn ordinary_pull_run(
        &self,
        sandbox: &SandboxRecord,
        pr: u64,
        sha: &str,
    ) -> AppResult<Option<CiRunRecord>> {
        if !self.config.ci.enabled {
            return Ok(None);
        }
        let runs = self
            .deps
            .store
            .ci_runs_for_pull_head(&sandbox.id, PullHead::Exact(sha))
            .await?;
        Ok(run_for_pull_head(&runs, pr).cloned())
    }

    /// The ordinary CI verdict for an armed PR, including its scheduler-shared run.
    pub(crate) async fn ci_gate_for_pull(
        &self,
        sandbox: &SandboxRecord,
        pr: u64,
        sha: &str,
    ) -> GateResult {
        self.ci_gate_select(
            sandbox,
            sha,
            self.deps
                .store
                .ci_runs_for_pull_head(&sandbox.id, PullHead::Exact(sha)),
            |runs| run_for_pull_head(runs, pr).map(|r| r.state),
        )
        .await
    }

    async fn ci_gate_select(
        &self,
        sandbox: &SandboxRecord,
        sha: &str,
        records: impl Future<Output = AppResult<Vec<CiRunRecord>>>,
        select: impl FnOnce(&[CiRunRecord]) -> Option<RunState>,
    ) -> GateResult {
        if !self.config.ci.enabled {
            return GateResult::NotConfigured;
        }
        if let Ok(runs) = records.await
            && let Some(state) = select(&runs)
        {
            return GateResult::Run(Some(state));
        }
        // No run yet: one will come when Yulius CI tests this commit, with the integration
        // branch's jobs or, for a commit that brings CI to a branch without any, with its own
        // (the scheduler's own answer, `ci_gate_for_head`).
        let view = self.ci_gate(sandbox).await;
        // While Git cannot tell whether a last valid gate exists, the commit waits for it: it
        // is never read as having no CI (SPEC-504 review round 2).
        if view.gate.is_some()
            || view.fallback_unknown
            || self.ci_gate_for_head(sandbox, &view, sha).await.is_some()
        {
            GateResult::Run(None)
        } else {
            GateResult::NotConfigured
        }
    }

    /// Queues a run of `gate` at `sha`.
    async fn ci_queue(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        gate: &Gate,
        by: &str,
    ) -> AppResult<CiRunRecord> {
        self.ci_queue_placed(sandbox, reference, sha, gate, by, self.now())
            .await
    }

    /// [`App::ci_queue`] with its place in the queue (`queued_at`).
    pub(crate) async fn ci_queue_placed(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        gate: &Gate,
        by: &str,
        queued_at: Timestamp,
    ) -> AppResult<CiRunRecord> {
        let run = self
            .ci_record_placed(sandbox, reference, sha, gate, by, queued_at)
            .await?;
        self.ci_publish(&run.id).await;
        self.ci_wake();
        Ok(run)
    }

    async fn ci_record_placed(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        gate: &Gate,
        by: &str,
        queued_at: Timestamp,
    ) -> AppResult<CiRunRecord> {
        let run = CiRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            reference,
            sha: sha.to_owned(),
            source: gate.source,
            state: RunState::Queued,
            reason: None,
            requested_by: by.to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at,
            started_at: None,
            ended_at: None,
        };
        // A job naming the files it tests is skipped, with the reason, for a change touching none
        // of them (SPEC-493 CJP-002); the branches' own runs and an unknown change run it.
        let changed = if gate.jobs.iter().any(|job| !job.paths.is_empty()) {
            self.ci_changed_paths(sandbox, &run.reference, sha).await
        } else {
            None
        };
        let jobs: Vec<CiJobRecord> = gate
            .jobs
            .iter()
            .enumerate()
            .map(|(index, job)| {
                let skip = yulius_domain::ci::after_merge_skip(job, &run.reference)
                    .or_else(|| yulius_domain::ci::paths_skip(&job.paths, changed.as_deref()));
                CiJobRecord {
                    id: self.deps.crypto.new_id(),
                    run_id: run.id.clone(),
                    ordinal: u32::try_from(index).unwrap_or(u32::MAX),
                    name: job.name.clone(),
                    key: job.key.clone(),
                    command: job.command.clone(),
                    matrix: job.matrix.clone(),
                    state: if skip.is_some() {
                        RunState::Skipped
                    } else {
                        RunState::Queued
                    },
                    reason: skip.clone(),
                    exit_code: skip.as_ref().map(|_| dom::SKIP_EXIT_CODE),
                    output: skip
                        .as_ref()
                        .map_or_else(String::new, |why| format!("{why}.\n")),
                    failures: None,
                    // Set below, so the job's span is recorded as any job's is.
                    started_at: None,
                    ended_at: None,
                    need: job.need,
                    timeout_minutes: job.timeout_minutes,
                    exclusive: job.exclusive,
                    waited_ms: None,
                }
            })
            .collect();
        // Its `yulius.toml` as CI will read it after the merge is invalid: the run fails now
        // with the parser's message, its jobs not run (SPEC-504 CFG-002).
        let refused = self.ci_config_problem(sandbox, &run.reference, sha).await;
        let (run, jobs) = match refused {
            Some(problem) => self.ci_refused_run(run, jobs, &problem),
            None => (run, jobs),
        };
        // Every job skipped for its paths: nothing to run, so the run takes no lane and passes now.
        let run = if run.state == RunState::Queued
            && !jobs.is_empty()
            && jobs.iter().all(|job| job.state == RunState::Skipped)
        {
            CiRunRecord {
                state: RunState::Passed,
                reason: Some(
                    if jobs
                        .iter()
                        .any(|j| dom::skipped_after_merge(j.state, j.reason.as_deref()))
                    {
                        "every job was skipped: no file the quick checks test changed, and the \
                         full suite runs on the merged revision"
                    } else {
                        "every job was skipped: no file they test changed"
                    }
                    .to_owned(),
                ),
                started_at: Some(self.now()),
                ended_at: Some(self.now()),
                ..run
            }
        } else {
            run
        };
        self.deps.store.insert_ci_run(&run, &jobs).await?;
        // A skipped job's start and end, one after the other: its span reads as a job that was
        // skipped (the stages count runs against skips); so does the refused configuration's.
        for job in jobs
            .iter()
            .filter(|job| matches!(job.state, RunState::Skipped | RunState::Failed))
        {
            for update in [
                CiJobUpdate {
                    started_at: Some(queued_at),
                    ..CiJobUpdate::default()
                },
                CiJobUpdate {
                    ended_at: Some(queued_at),
                    ..CiJobUpdate::default()
                },
            ] {
                self.deps
                    .store
                    .transition_ci_job(&job.id, &[job.state], job.state, &update)
                    .await?;
            }
        }
        tracing::info!(sandbox = %sandbox.slug, run = %run.id, sha = %short(sha), reference = %run.reference.key(), jobs = jobs.len(), "CI run queued");
        // A run that passed here ends without the executor, which starts the merge pass at a
        // run's end: the dispatcher, woken below, starts it instead.
        if run.state == RunState::Passed
            && let Ok(mut ended) = self.ci.passed_when_queued.lock()
        {
            ended.push(sandbox.id.clone());
        }
        Ok(run)
    }

    /// Why the `yulius.toml` of a run at `sha` (`reference`) is invalid as CI will read it after
    /// the merge (SPEC-504 CFG-002): the commit's own file for a branch, a merge candidate and a
    /// playground task's merge; for a pull request its own file when it changed it since its
    /// merge base with the integration branch, else the integration branch's. `None` when it is
    /// valid or cannot be told.
    async fn ci_config_problem(
        &self,
        sandbox: &SandboxRecord,
        reference: &CiRef,
        sha: &str,
    ) -> Option<String> {
        let repo = self.sandbox_repo(&sandbox.id);
        let read = |rev: String| {
            let repo = repo.clone();
            async move {
                self.deps
                    .git
                    .show_file(&repo, &rev, "yulius.toml")
                    .await
                    .ok()
            }
        };
        let at = match reference {
            CiRef::PullRequest(_) => {
                let integration =
                    format!("origin/{}", delivery_profile(sandbox).integration_branch);
                // What Git cannot tell (a merge base, a file) leaves the head's own file to
                // check: a failure never skips the check (review round 1).
                let base = self.deps.git.merge_base(&repo, sha, &integration).await;
                let changed = match base {
                    Ok(Some(base)) => match (read(base).await, read(sha.to_owned()).await) {
                        (Some(before), Some(now)) => before != now,
                        _ => true,
                    },
                    Ok(None) | Err(_) => true,
                };
                if changed {
                    sha.to_owned()
                } else {
                    let problem = self.ci_jobs_at(&repo, &integration).await.err()?;
                    return Some(format!(
                        "{DEV_INVALID_PREFIX}{} and this change does not repair it (its run \
                         goes again once it is valid there): {problem}",
                        delivery_profile(sandbox).integration_branch
                    ));
                }
            }
            _ => sha.to_owned(),
        };
        let problem = self.ci_jobs_at(&repo, &at).await.err()?;
        // The parser's message names the file and its line already.
        Some(if problem.starts_with("yulius.toml") {
            problem
        } else {
            format!("yulius.toml is invalid: {problem}")
        })
    }

    /// `run` and its `jobs` refused for `problem` (SPEC-504 CFG-002): failed now, its jobs
    /// skipped unrun, and a failed job `yulius.toml` saying why.
    fn ci_refused_run(
        &self,
        run: CiRunRecord,
        mut jobs: Vec<CiJobRecord>,
        problem: &str,
    ) -> (CiRunRecord, Vec<CiJobRecord>) {
        for job in &mut jobs {
            job.state = RunState::Skipped;
            job.reason = Some("not run: yulius.toml is invalid".to_owned());
            job.exit_code = Some(dom::SKIP_EXIT_CODE);
            job.output = "Not run: yulius.toml is invalid (see the yulius.toml job).\n".to_owned();
        }
        jobs.push(CiJobRecord {
            id: self.deps.crypto.new_id(),
            run_id: run.id.clone(),
            ordinal: u32::try_from(jobs.len()).unwrap_or(u32::MAX),
            name: CONFIG_JOB.to_owned(),
            key: CONFIG_JOB.to_owned(),
            command: "yulius.toml, read as CI reads it after the merge".to_owned(),
            matrix: None,
            state: RunState::Failed,
            reason: Some(problem.to_owned()),
            exit_code: Some(1),
            output: format!("{problem}\n"),
            failures: None,
            started_at: None,
            ended_at: None,
            need: Default::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        });
        let now = self.now();
        let run = CiRunRecord {
            state: RunState::Failed,
            reason: Some(problem.to_owned()),
            started_at: Some(now),
            ended_at: Some(now),
            ..run
        };
        (run, jobs)
    }

    /// The files the change of a run at `sha` touches (SPEC-493 CJP-002): a pull request's head,
    /// a merge candidate and a playground task's head against their merge base with the
    /// integration branch (a candidate is built on it, so that is its members' changes). `None`
    /// for the branches' own runs (every job tests every merged state, CJP-003) and when the
    /// files cannot be told.
    async fn ci_changed_paths(
        &self,
        sandbox: &SandboxRecord,
        reference: &CiRef,
        sha: &str,
    ) -> Option<Vec<String>> {
        if matches!(reference, CiRef::Branch(_) | CiRef::Candidate(_)) {
            return None;
        }
        let integration = format!("origin/{}", delivery_profile(sandbox).integration_branch);
        self.deps
            .git
            .changed_paths(&self.sandbox_repo(&sandbox.id), &integration, sha)
            .await
            .ok()
            .flatten()
    }

    fn ci_sync_lock(&self, sandbox: &SandboxId) -> Arc<SandboxSync> {
        // Nothing panics while the map is held; a poisoned map is still the one map.
        self.ci
            .sync_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(sandbox.as_str().to_owned())
            .or_default()
            .clone()
    }

    /// One scheduling round for a sandbox: fetch, read the heads, queue and supersede.
    ///
    /// # Errors
    /// GitHub or Git failures (the round is retried at the next poll).
    pub async fn ci_sync_sandbox(&self, sandbox: &SandboxRecord) -> AppResult<()> {
        let lock = self.ci_sync_lock(&sandbox.id);
        let _inventory = lock.inventory.lock().await;
        let repo = self.sandbox_repo(&sandbox.id);
        if sandbox.removed_at.is_some() || !repo.join(".git").exists() {
            return Ok(());
        }
        // A playground has no GitHub (SPEC-163 PCI-001): no token and no pull request; its own
        // `origin` is fetched, and its tasks' heads are tested when they deliver.
        let playground = sandbox.is_playground();
        lock.schedule.lock().await.clear();
        let pulls = if playground {
            Ok(Vec::new())
        } else {
            let token = self.dlv_installation_token(sandbox).await?;
            self.deps
                .github
                .list_pulls(&token, &sandbox.repo_owner, &sandbox.repo_name, None)
                .await
        };
        let mut publications = Vec::new();
        let openings = lock.schedule.lock().await;
        let result = self
            .ci_record_inventory(sandbox, pulls, &openings, &mut publications)
            .await;
        drop(openings);
        for run in publications {
            self.ci_publish(&run).await;
            self.ci_wake();
        }
        result
    }

    async fn ci_record_inventory(
        &self,
        sandbox: &SandboxRecord,
        pulls: AppResult<Vec<crate::ports::github::GhPull>>,
        openings: &HashMap<u64, String>,
        publications: &mut Vec<String>,
    ) -> AppResult<()> {
        let repo = self.sandbox_repo(&sandbox.id);
        let playground = sandbox.is_playground();
        self.dlv_refresh_clone(sandbox).await?;
        self.ci_forget_gate(&sandbox.id);
        let view = self.ci_gate(sandbox).await;
        self.ci_note_config_block(sandbox, &view).await;
        let profile = delivery_profile(sandbox);
        let mut targets: Vec<(Target, Gate)> = Vec::new();
        if let Some(gate) = &view.gate {
            let mut branches = vec![profile.integration_branch.clone()];
            // Only a promotion moves the production branch, and a playground has none.
            if profile.production_branch != profile.integration_branch && !playground {
                branches.push(profile.production_branch.clone());
            }
            for branch in branches {
                if let Ok(Some(sha)) = self
                    .deps
                    .git
                    .resolve_commit(&repo, &format!("origin/{branch}"))
                    .await
                {
                    targets.push((
                        Target {
                            reference: CiRef::Branch(branch),
                            sha,
                        },
                        gate.clone(),
                    ));
                }
            }
        }
        // A fork's pull request is never tested (ADR-0116): its author has no push access, and
        // its head can still be in the clone (an agent fetched `pull/N/head` to review it), so
        // GitHub's word that it comes from another repository decides, not the clone (#842).
        let eligible = pulls?
            .into_iter()
            .filter(|p| p.state == "open" && p.base == profile.integration_branch && !p.from_fork)
            .collect::<Vec<_>>();
        let store = &self.deps.store;
        let admitted = store.ci_admitted_pulls(&sandbox.id).await?;
        let heads = opening::inventory_heads(&admitted, &eligible, openings);
        let numbers: Vec<_> = heads.iter().map(|(number, _)| *number).collect();
        if numbers != admitted {
            store.set_ci_admitted_pulls(&sandbox.id, &numbers).await?;
        }
        for (number, sha) in &heads {
            // A head not in the clone (not fetched yet) is tested at a later round.
            if !matches!(self.deps.git.resolve_commit(&repo, sha).await, Ok(Some(_))) {
                continue;
            }
            let gate = self.ci_gate_for_head(sandbox, &view, sha).await;
            if let Some(gate) = gate {
                targets.push((
                    Target {
                        reference: CiRef::PullRequest(*number),
                        sha: sha.clone(),
                    },
                    gate,
                ));
            }
        }
        let integration_head = targets.iter().find_map(|(target, _)| {
            matches!(&target.reference, CiRef::Branch(branch) if branch == &profile.integration_branch)
                .then_some(target)
        });
        let runs = self
            .deps
            .store
            .ci_planning_runs(&sandbox.id, PLAN_RUNS, integration_head)
            .await?;
        let facts = ordinary_run_facts(&runs, view.problem.is_none());
        let only_targets: Vec<Target> = targets.iter().map(|(t, _)| t.clone()).collect();
        let plan = dom::plan(&only_targets, &facts);
        crate::race::reached(
            crate::race::Point::CiSyncPlanned,
            Path::new(sandbox.id.as_str()),
        );
        // A pull request's new head keeps the place in the queue of the run it supersedes: it
        // went to the back at every push before, behind every other pull request (up to 73
        // minutes of waiting, F 2026-10-12).
        let mut places: HashMap<String, Timestamp> = HashMap::new();
        for (run_id, _) in &plan.supersede {
            if let Some(old) = runs
                .iter()
                .find(|r| &r.id == run_id && r.state == RunState::Queued)
            {
                places.insert(old.reference.key(), old.queued_at);
            }
        }
        for (run_id, successor) in plan.supersede {
            let reason = match successor {
                Some(sha) => format!("superseded by {}", short(&sha)),
                None => "its pull request or branch is gone".to_owned(),
            };
            if let Err(err) = self
                .ci_record_cancellation(&run_id, &reason, publications)
                .await
            {
                tracing::warn!(error = %err, run = %run_id, "superseding a CI run failed");
            }
        }
        for target in plan.queue {
            if matches!(&target.reference, CiRef::Branch(branch) if branch == &profile.integration_branch)
            {
                let cancelled = facts.iter().any(|run| {
                    run.sha == target.sha
                        && matches!(run.reference, CiRef::Candidate(_))
                        && run.state == RunState::Cancelled
                });
                if cancelled
                    || self.deps.store.active_ci_runs().await?.iter().any(|r| {
                        r.sandbox_id == sandbox.id && matches!(r.reference, CiRef::Candidate(_))
                    })
                {
                    // An active candidate owns the slot. A cancelled candidate at this head
                    // stays stopped until a person asks for a rerun.
                    continue;
                }
            }
            if let Some((_, gate)) = targets.iter().find(|(t, _)| *t == target) {
                // A branch commit whose tree a run already passed (a squash merge of an
                // up-to-date pull request) reuses that result instead of running again: every
                // merge used to cost a second 12-20 minute run in the sandbox's one lane.
                if matches!(target.reference, CiRef::Branch(_))
                    && let Some(id) = self
                        .ci_record_reused_tree(sandbox, &target, gate, &runs)
                        .await?
                {
                    publications.push(id);
                    continue;
                }
                let place = places
                    .get(&target.reference.key())
                    .copied()
                    .unwrap_or_else(|| self.now());
                let queued = self
                    .ci_record_placed(
                        sandbox,
                        target.reference.clone(),
                        &target.sha,
                        gate,
                        "yulius",
                        place,
                    )
                    .await?;
                publications.push(queued.id);
            }
        }
        Ok(())
    }

    /// Reuses a recent verdict for the candidate's tree. A failed candidate lends its original
    /// run and job logs (SPEC-542). Otherwise use the existing passed-tree checks (SPEC-497).
    ///
    /// # Errors
    /// Store or Git failures.
    pub(crate) async fn ci_reuse_for_candidate(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        tree: &str,
        gate: &Gate,
    ) -> AppResult<Option<CiRunRecord>> {
        let runs = self.deps.store.ci_runs(&sandbox.id, PLAN_RUNS).await?;
        let repo = self.sandbox_repo(&sandbox.id);
        for run in runs
            .iter()
            .filter(|r| r.state == RunState::Failed && matches!(r.reference, CiRef::Train(_)))
        {
            if self.deps.git.tree_of(&repo, &run.sha).await?.as_deref() == Some(tree) {
                return Ok(Some(run.clone()));
            }
        }
        let target = Target {
            reference,
            sha: sha.to_owned(),
        };
        let Some(id) = self
            .ci_record_reused_tree(sandbox, &target, gate, &runs)
            .await?
        else {
            return Ok(None);
        };
        self.ci_publish(&id).await;
        self.deps.store.ci_run(&id).await
    }

    /// Records a passed run for `target` when a recent passed run tested a commit with the same
    /// tree (and therefore the same jobs); its id, or `None` when there is none.
    async fn ci_record_reused_tree(
        &self,
        sandbox: &SandboxRecord,
        target: &Target,
        gate: &Gate,
        runs: &[CiRunRecord],
    ) -> AppResult<Option<String>> {
        let repo = self.sandbox_repo(&sandbox.id);
        let Some(tree) = self.deps.git.tree_of(&repo, &target.sha).await? else {
            return Ok(None);
        };
        // A merge train's verdict for a pull request it merged before the last one covers a tree
        // no run tested (SPEC-084 TRN-005): never a source.
        // A branch's own run runs every job (SPEC-493 CJP-003): a source that skipped a job for
        // its `paths` never ran it, so it is no source there, and an earlier one may be. A merge
        // candidate's neither: the same tree on another base may change what `paths` decides.
        let mut found = None;
        for run in runs.iter().filter(|r| reuse_source(r, target)) {
            if self.deps.git.tree_of(&repo, &run.sha).await?.as_deref() != Some(tree.as_str()) {
                continue;
            }
            let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
            if !jobs_cover_target(&target.reference, gate, &jobs) {
                continue;
            }
            found = Some((run, jobs));
            break;
        }
        let Some((source, tested)) = found else {
            return Ok(None);
        };
        // Its jobs keep the verdict they had there: a skipped one stays skipped (SPEC-162).
        let now = self.now();
        let note = format!(
            "not run again: {} has the same tree as {} ({}), which passed",
            short(&target.sha),
            short(&source.sha),
            source.reference.key()
        );
        let run = CiRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            reference: target.reference.clone(),
            sha: target.sha.clone(),
            source: gate.source,
            state: RunState::Passed,
            reason: Some(note.clone()),
            requested_by: "yulius".to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: now,
            started_at: Some(now),
            ended_at: Some(now),
        };
        let jobs: Vec<CiJobRecord> = gate
            .jobs
            .iter()
            .enumerate()
            .map(|(index, job)| {
                // The job that ran as this one is defined, which tells a job from a change's twin
                // of it (SPEC-504 review rounds 2 and 4). A job the source did not have (the gate
                // changed since) is passed, as before SPEC-162.
                let source = tested.iter().find(|t| ran_as(t, job));
                let (state, exit_code) = dom::copied_job_verdict(source.map(|t| t.state));
                let reason =
                    dom::copied_job_reason(state, source.and_then(|t| t.reason.as_deref()), &note);
                CiJobRecord {
                    id: self.deps.crypto.new_id(),
                    run_id: run.id.clone(),
                    ordinal: u32::try_from(index).unwrap_or(u32::MAX),
                    name: job.name.clone(),
                    key: job.key.clone(),
                    command: job.command.clone(),
                    matrix: job.matrix.clone(),
                    state,
                    reason: Some(reason),
                    exit_code: Some(exit_code),
                    output: format!("{note}.\n"),
                    failures: None,
                    started_at: Some(now),
                    ended_at: Some(now),
                    need: job.need,
                    timeout_minutes: job.timeout_minutes,
                    exclusive: job.exclusive,
                    waited_ms: None,
                }
            })
            .collect();
        self.deps.store.insert_ci_run(&run, &jobs).await?;
        tracing::info!(sandbox = %sandbox.slug, run = %run.id, sha = %short(&target.sha), reused = %short(&source.sha), "CI result reused for the same tree");
        Ok(Some(run.id))
    }

    /// Cancels a run: a queued one at once, a running one through its executor (which kills
    /// its jobs and records the end). `false` when it had already ended.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn ci_cancel(&self, run_id: &str, reason: &str) -> AppResult<bool> {
        let mut publications = Vec::new();
        let changed = self
            .ci_record_cancellation(run_id, reason, &mut publications)
            .await?;
        for id in publications {
            self.ci_publish(&id).await;
        }
        Ok(changed)
    }

    async fn ci_record_cancellation(
        &self,
        run_id: &str,
        reason: &str,
        publications: &mut Vec<String>,
    ) -> AppResult<bool> {
        let Some(run) = self.deps.store.ci_run(run_id).await? else {
            return Err(AppError::NotFound);
        };
        match run.state {
            RunState::Queued => {
                let update = CiRunUpdate {
                    reason: Some(reason.to_owned()),
                    ended_at: Some(self.now()),
                    ..CiRunUpdate::default()
                };
                if !self
                    .deps
                    .store
                    .transition_ci_run(run_id, &[RunState::Queued], RunState::Cancelled, &update)
                    .await?
                {
                    return Ok(false);
                }
                self.ci_cancel_jobs(run_id, reason).await?;
                publications.push(run_id.to_owned());
                Ok(true)
            }
            RunState::Running => {
                let signalled = self
                    .ci
                    .executing
                    .lock()
                    .ok()
                    .and_then(|map| map.get(run_id).map(|tx| tx.send(Some(reason.to_owned()))))
                    .is_some();
                if signalled {
                    return Ok(true);
                }
                // No executor in this process (it was lost): record the end directly.
                self.ci_cancel_jobs(run_id, reason).await?;
                let update = CiRunUpdate {
                    reason: Some(reason.to_owned()),
                    ended_at: Some(self.now()),
                    ..CiRunUpdate::default()
                };
                let changed = self
                    .deps
                    .store
                    .transition_ci_run(run_id, &[RunState::Running], RunState::Cancelled, &update)
                    .await?;
                publications.push(run_id.to_owned());
                Ok(changed)
            }
            _ => Ok(false),
        }
    }

    async fn ci_cancel_jobs(&self, run_id: &str, reason: &str) -> AppResult<()> {
        for job in self.deps.store.ci_jobs(run_id, false).await? {
            let update = CiJobUpdate {
                reason: Some(reason.to_owned()),
                ended_at: Some(self.now()),
                ..CiJobUpdate::default()
            };
            self.deps
                .store
                .transition_ci_job(
                    &job.id,
                    &[RunState::Queued, RunState::Running],
                    RunState::Cancelled,
                    &update,
                )
                .await?;
        }
        Ok(())
    }

    /// Publishes the commit status of a run's current state (best effort; failures are
    /// recorded on the run and retried by the loop), after a change of the run: the sandbox's
    /// browsers read its pull requests again (SPEC-108 PRL-003).
    pub(crate) async fn ci_publish(&self, run_id: &str) {
        if let Err(err) = self.ci_try_publish(run_id, true).await {
            tracing::debug!(error = %err, run = %run_id, "publishing a CI status failed");
        }
    }

    /// `announce`: the run changed (not a retry of its status), so the browsers are told.
    async fn ci_try_publish(&self, run_id: &str, announce: bool) -> AppResult<()> {
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::CiStatuses,
            self.ci_try_publish_inner(run_id, announce),
        )
        .await
    }

    async fn ci_try_publish_inner(&self, run_id: &str, announce: bool) -> AppResult<()> {
        let store = &self.deps.store;
        let Some(run) = store.ci_run(run_id).await? else {
            return Ok(());
        };
        let Some(sandbox) = store.sandbox(&run.sandbox_id).await? else {
            return Ok(());
        };
        // A merge train's commit exists only in Yulius's clone (SPEC-084): nothing to publish on
        // (the note counts as an attempt, so the retry loop lets it be).
        if matches!(run.reference, CiRef::Train(_)) {
            return store
                .record_ci_status(run_id, None, Some(crate::merge_train::TRAIN_STATUS_NOTE))
                .await;
        }
        // The run's state is what a pull request's checks say (`ci_gate_result`), whether or not
        // GitHub takes the status below.
        if announce {
            self.publish_pulls(&run.sandbox_id);
        }
        // A playground has no GitHub repository (SPEC-163 PCI-005): its runs gate its merges here
        // and publish nothing.
        if sandbox.is_playground() {
            return store
                .record_ci_status(run_id, None, Some(PLAYGROUND_STATUS_NOTE))
                .await;
        }
        // A known App-wide wait must not spend this status's bounded recovery attempts.
        if self.deps.github.rate_limit_reset().is_some() {
            return Ok(());
        }
        let jobs = store.ci_jobs(run_id, false).await?;
        let failed = jobs.iter().find(|j| j.state == RunState::Failed);
        let summary = RunSummary {
            jobs: jobs.len(),
            passed: jobs.iter().filter(|j| j.state == RunState::Passed).count(),
            skipped: jobs.iter().filter(|j| j.state == RunState::Skipped).count(),
            failed_job: failed.map(|j| (j.name.as_str(), j.exit_code)),
            duration_ms: run
                .started_at
                .zip(run.ended_at)
                .map(|(start, end)| end.millis_since(start)),
            reason: run.reason.as_deref(),
        };
        let status = dom::commit_status(run.state, summary);
        let token = match self.dlv_installation_token(&sandbox).await {
            Ok(token) => token,
            Err(err) if self.deps.github.rate_limit_reset().is_some() => return Err(err),
            Err(err) => {
                let note = format!("not published: {}", err.public_message());
                store.record_ci_status(run_id, None, Some(&note)).await?;
                return Err(err);
            }
        };
        let request = NewCommitStatus {
            state: status.state.as_str().to_owned(),
            context: dom::STATUS_CONTEXT.to_owned(),
            description: status.description,
            target_url: Some(format!(
                "{}/s/{}/ci?run={}",
                self.config.public_url, sandbox.slug, run.id
            )),
        };
        match self
            .deps
            .github
            .create_commit_status(
                &token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                &run.sha,
                &request,
            )
            .await
        {
            Ok(()) => store.record_ci_status(run_id, Some(run.state), None).await,
            Err(err) if self.deps.github.rate_limit_reset().is_some() => Err(err),
            Err(err) => {
                let note = match &err {
                    AppError::Forbidden(message) => format!("not published: {message}"),
                    other => format!("not published: {}", other.public_message()),
                };
                store.record_ci_status(run_id, None, Some(&note)).await?;
                Err(err)
            }
        }
    }

    /// Starts the CI loops once (no-op when CI is disabled).
    pub fn start_ci(self: &Arc<Self>) {
        if !self.config.ci.enabled || self.ci.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.ci_recover().await;
            loop {
                app.ci_dispatch().await;
                tokio::select! {
                    () = app.ci.wake.notified() => {}
                    () = tokio::time::sleep(DISPATCH_TICK) => {}
                }
            }
        });
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(app.config.ci.poll);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                app.ci_sync_all().await;
                app.ci_retry_statuses().await;
                app.ci_prune().await;
            }
        });
        self.start_hygiene();
    }

    /// One sync round over every sandbox.
    pub async fn ci_sync_all(&self) {
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            if let Err(err) = self.ci_sync_sandbox(sandbox).await {
                tracing::debug!(sandbox = %sandbox.slug, error = %err, "CI sync failed");
            }
        }
    }

    /// Publishes again the commit statuses that did not reach GitHub (run by the CI loop).
    pub async fn ci_retry_statuses(&self) {
        let since = self.now().plus_millis(-STATUS_RETRY_WINDOW_MS);
        let Ok(runs) = self
            .deps
            .store
            .unpublished_ci_runs(since, STATUS_MAX_ATTEMPTS, 50)
            .await
        else {
            return;
        };
        for run in runs {
            // A retry changes nothing a pull request's checks say: no announcement.
            if let Err(err) = self.ci_try_publish(&run.id, false).await {
                tracing::debug!(error = %err, run = %run.id, "publishing a CI status failed");
            }
        }
    }

    async fn ci_prune(self: &Arc<Self>) {
        let now = self.now().0;
        {
            let Ok(mut last) = self.ci.last_prune.lock() else {
                return;
            };
            if now - *last < PRUNE_EVERY_MS {
                return;
            }
            *last = now;
        }
        match self
            .deps
            .store
            .prune_ci_runs(KEEP_RUNS, Timestamp(now - RETAIN_MS))
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!(runs = n, "old CI runs deleted"),
            Err(err) => tracing::warn!(error = %err, "pruning CI runs failed"),
        }
        self.cd_prune().await;
        self.prune_spans(Timestamp(now)).await;
        // Quarantined tests whose issue was closed go back into the runs (SPEC-497 OFC-002), off
        // the poll loop: it asks GitHub once per quarantine (review of the quarantine, round 3).
        let app = Arc::clone(self);
        tokio::spawn(async move { app.ci_release_quarantines().await });
    }

    /// Starts the oldest queued runs of every sandbox, as many as it has free lanes
    /// ([`CiSettings::lanes`]); the instance-wide job slots still bound the machine's load.
    pub async fn ci_dispatch(self: &Arc<Self>) {
        // A reading of what CI uses at each pass keeps the measuring bases recent, and once a
        // minute the host's load is kept (SPEC-497).
        let _ = self.ci_measured();
        self.ci_record_load().await;
        // A merge armed on a run that passed when it was queued happens now (SPEC-011).
        let ended: Vec<SandboxId> = self
            .ci
            .passed_when_queued
            .lock()
            .map(|mut ended| std::mem::take(&mut *ended))
            .unwrap_or_default();
        for sandbox in ended {
            let app = Arc::clone(self);
            tokio::spawn(async move { app.merge_armed(&sandbox).await });
        }
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        let lanes = self.ci_lanes();
        for run in active.into_iter().filter(|r| r.state == RunState::Queued) {
            // Over the disk quota a run waits in the queue instead of erroring (CI-301); asked
            // only when a lane is free.
            let lane_free = self
                .ci
                .busy
                .lock()
                .is_ok_and(|busy| free_lane(&busy, run.sandbox_id.as_str(), lanes).is_some());
            if !lane_free || !self.ci_disk_room(&run.sandbox_id).await {
                continue;
            }
            let sandbox = run.sandbox_id.as_str().to_owned();
            let (tx, rx) = watch::channel(None);
            // The lane whose checkouts last built what this run builds on, where one is free
            // (SPEC-497 SHR-002): a candidate starts from its members' builds, a pull request from
            // its previous head's.
            let last: Vec<(u32, CiRef)> = self
                .ci
                .lane_runs
                .lock()
                .map(|map| {
                    (0..lanes.max(1))
                        .filter_map(|lane| {
                            map.get(&format!("{sandbox}/{lane}"))
                                .map(|r| (lane, r.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let preferred = preferred_lanes(&run.reference, &last);
            let lane = {
                let (Ok(mut busy), Ok(mut executing)) =
                    (self.ci.busy.lock(), self.ci.executing.lock())
                else {
                    return;
                };
                let Some(lane) = reserve_run(
                    &mut busy,
                    &mut executing,
                    (&run.id, &sandbox, lanes),
                    &preferred,
                    tx,
                ) else {
                    continue;
                };
                lane
            };
            let before = last.iter().find(|(l, _)| *l == lane).map(|(_, r)| r.key());
            let note = lane_note(lane, before.as_deref(), &preferred);
            if let Ok(mut map) = self.ci.lane_runs.lock() {
                map.insert(format!("{sandbox}/{lane}"), run.reference.clone());
            }
            if let Ok(mut notes) = self.ci.lane_notes.lock() {
                notes.insert(run.id.clone(), note);
            }
            let app = Arc::clone(self);
            tokio::spawn(async move { app.ci_execute(run, rx, lane).await });
        }
    }

    /// After a restart: runs recorded as running have no executor; their hosts are ended and
    /// they are queued again.
    async fn ci_recover(&self) {
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        for run in active.into_iter().filter(|r| r.state == RunState::Running) {
            if self
                .ci
                .executing
                .lock()
                .is_ok_and(|map| map.contains_key(&run.id))
            {
                continue;
            }
            if let Ok(jobs) = self.deps.store.ci_jobs(&run.id, false).await {
                for job in jobs {
                    let dir = self.ci_run_dir(&job.id);
                    if dir.exists() {
                        let _ = self.deps.session_host.terminate(&dir);
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    // A restart ended it: what it left goes too (SPEC-504 CLB-004), in a task
                    // of its own: an engine that never answers must not hold CI's start (review
                    // round 1).
                    if let Some(isolation) = self.isolation.get() {
                        let (isolation, sandbox) = (Arc::clone(isolation), run.sandbox_id.clone());
                        tokio::spawn(
                            async move { isolation.ci_job_ended(&sandbox, &job.id).await },
                        );
                    }
                }
            }
            match self
                .deps
                .store
                .requeue_ci_run(
                    &run.id,
                    "[yulius] queued again after a restart of Yulius\n",
                    self.now(),
                )
                .await
            {
                Ok(true) => tracing::info!(run = %run.id, "CI run queued again after a restart"),
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(run = %run.id, error = %err, "requeueing a CI run failed")
                }
            }
        }
    }

    fn ci_run_dir(&self, job_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runs")
            .join(format!("ci-{job_id}"))
    }

    // ---------------------------------------------------------------- API

    async fn ci_sandbox(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        self.require_for(ctx, &sid, action).await
    }

    async fn ci_run_of(&self, sandbox: &SandboxRecord, run_id: &str) -> AppResult<CiRunRecord> {
        let store = &self.deps.store;
        if let Some(run) = store.ci_run(run_id).await? {
            return (run.sandbox_id == sandbox.id)
                .then_some(run)
                .ok_or(AppError::NotFound);
        }
        store
            .archived_gate_run(&sandbox.id, run_id)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// `GET /api/sandboxes/{sid}/ci`: the CI in force, recent runs (without outputs) and, for
    /// administrators, whether statuses can be published.
    ///
    /// # Errors
    /// Access errors, store failures.
    pub async fn ci_overview(&self, ctx: &AuthContext, sandbox_id: &str) -> AppResult<CiOverview> {
        let (sandbox, level) = self.ci_sandbox(ctx, sandbox_id, Action::View).await?;
        let view = self.ci_gate(&sandbox).await;
        let records = self.deps.store.ci_runs(&sandbox.id, RUN_LIST_LIMIT).await?;
        let mut runs = Vec::with_capacity(records.len());
        for run in &records {
            let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
            runs.push(self.ci_run_view(&sandbox, run, &jobs, false));
        }
        let admin = level >= AccessLevel::Admin || ctx.user.is_superadmin;
        let status_access = records
            .iter()
            .find(|run| run.status_for.is_some() || run.status_note.is_some())
            .and_then(|run| run.status_note.as_deref())
            .filter(|note| admin && note.contains(STATUS_PERMISSION_MISSING))
            .map(|_| {
                format!(
                    "Yulius can't publish its CI results on {}/{}: {STATUS_PERMISSION_MISSING}. The runs and the merge gate work without it.",
                    sandbox.repo_owner, sandbox.repo_name
                )
            });
        Ok(CiOverview {
            gate_notices: self.project_gate_notices(&sandbox).await?,
            gate: view.gate.as_ref().map(gate_view),
            gate_problem: view.problem,
            gate_fallback: view.fallback,
            runs,
            can_run: level >= AccessLevel::Write,
            status_access,
            actions_workflows: view.workflows,
        })
    }

    /// `GET /api/sandboxes/{sid}/ci/runs/{rid}`: one run with its jobs' output.
    ///
    /// # Errors
    /// Access errors, `NotFound`.
    pub async fn ci_run_detail(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::View).await?;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let jobs = self.deps.store.ci_jobs(&run.id, true).await?;
        Ok(self.ci_run_view(&sandbox, &run, &jobs, true))
    }

    /// `POST /api/sandboxes/{sid}/ci/runs/{rid}/cancel`.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Conflict` when it already ended.
    pub async fn ci_cancel_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        if !self
            .ci_cancel(&run.id, &format!("cancelled by {actor}"))
            .await?
        {
            return Err(AppError::Conflict("this run has already ended".into()));
        }
        self.dlv_audit(
            &actor,
            "ci.cancel",
            &sandbox.slug,
            &format!("run {} at {}", run.id, run.sha),
        )
        .await;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
        Ok(run_view(&run, &jobs, false))
    }

    /// The jobs that test `head`, a commit on its way into the integration branch (a pull
    /// request's head, a merge train's, a playground task's merge), given the integration
    /// branch's CI (`view`). One rule for whoever asks: the scheduler, a merge train, a
    /// playground's delivery, a run again and a change from the remote control.
    ///
    /// - The integration branch has CI (its own, or while its configuration is broken its last
    ///   valid gate, CFG-003): its jobs, always. A change never weakens the gate that judges it,
    ///   also when it changes its jobs (SPEC-504 CFG-001, ADR-0360; superseding CI-110); its own
    ///   `yulius.toml` is checked separately (`ci_config_problem`).
    /// - It has none (none yet, or never a valid one): the commit's own, when it brings some
    ///   (the change that adds CI, or repairs a file never valid, is tested with it); own jobs
    ///   keep the trusted security job (SPEC-217 VDP-001).
    /// - Otherwise none: nothing tests the commit.
    pub(crate) async fn ci_gate_for_head(
        &self,
        sandbox: &SandboxRecord,
        view: &GateView,
        head: &str,
    ) -> Option<Gate> {
        let repo = self.sandbox_repo(&sandbox.id);
        if let Some(gate) = &view.gate {
            // Its own changed and added jobs beside the gate's, never instead (CFG-001); an own
            // file that cannot be read fails the run by the configuration check.
            let Ok(own) = self.ci_jobs_at(&repo, head).await else {
                return Some(gate.clone());
            };
            // What the change itself changed: against its merge base with the integration
            // branch (review round 2; unknown: against the gate alone).
            let integration = format!("origin/{}", delivery_profile(sandbox).integration_branch);
            let at_base = match self.deps.git.merge_base(&repo, head, &integration).await {
                Ok(Some(base)) => self.ci_jobs_at(&repo, &base).await.ok(),
                _ => None,
            };
            return Some(dom::with_changed_jobs(
                gate.clone(),
                own,
                at_base.as_deref(),
            ));
        }
        // The last valid gate could not be looked for (Git failed): no verdict until it can,
        // never the commit's own jobs in its place (review round 1).
        if view.fallback_unknown {
            return None;
        }
        // With no gate in force the integration head has no merge-first target: a change that
        // brings CI runs its whole suite (SPEC-575 MFS-002, review round 1).
        let jobs = match self.ci_jobs_at(&repo, head).await {
            Ok(jobs) if !jobs.is_empty() => dom::after_merge_in_force(jobs, false),
            _ => return None,
        };
        let scans = self.ci_scans(sandbox).await;
        let own = Gate {
            jobs,
            source: GateSource::PullRequest,
        };
        Some(dom::keep_security_jobs(own, None, scans))
    }

    /// Queues the commit of `run_id` again with the CI in force now.
    ///
    /// # Errors
    /// `Conflict` while a run of that commit is active or without CI.
    pub(crate) async fn ci_rerun(
        &self,
        sandbox: &SandboxRecord,
        run_id: &str,
        by: &str,
    ) -> AppResult<CiRunRecord> {
        let run = self.ci_run_of(sandbox, run_id).await?;
        if matches!(run.reference, CiRef::Train(_)) {
            return Err(AppError::Conflict(
                "a merge train is not run again: Yulius builds a new one from the armed merges"
                    .into(),
            ));
        }
        let others = self
            .deps
            .store
            .ci_runs_for_sha(&sandbox.id, &run.sha)
            .await?;
        if others.iter().any(|r| r.state.is_active()) {
            return Err(AppError::Conflict(
                "a run of this commit is already queued or running".into(),
            ));
        }
        self.ci_forget_gate(&sandbox.id);
        let view = self.ci_gate(sandbox).await;
        // The jobs the scheduler would give this commit now: a branch's are the integration
        // branch's, a commit on its way there may have its own (`ci_gate_for_head`).
        // A branch's run takes the branch's gate; a commit on its way (a pull request's head,
        // a playground task's merge; a train's run never gets here) the scheduler's rule.
        let on_its_way = !matches!(run.reference, CiRef::Branch(_) | CiRef::Candidate(_));
        let gate = if on_its_way {
            self.ci_gate_for_head(sandbox, &view, &run.sha).await
        } else {
            view.gate.clone()
        };
        let Some(gate) = gate else {
            // Why nothing tests it: the integration branch's configuration, the commit's own
            // that cannot be read, or none anywhere.
            let own = if on_its_way {
                self.ci_jobs_at(&self.sandbox_repo(&sandbox.id), &run.sha)
                    .await
            } else {
                Ok(Vec::new())
            };
            return Err(AppError::Conflict(match (view.problem, own, on_its_way) {
                (Some(problem), ..) => problem,
                (None, Err(unreadable), _) => unreadable,
                (None, Ok(_), false) => "this sandbox has no CI configured".into(),
                (None, Ok(_), true) => "this commit has no CI configured".into(),
            }));
        };
        self.ci_queue(sandbox, run.reference.clone(), &run.sha, &gate, by)
            .await
    }

    /// `POST /api/sandboxes/{sid}/ci/runs/{rid}/rerun`.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Conflict`.
    pub async fn ci_rerun_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        let run = self.ci_rerun(&sandbox, run_id, &actor).await?;
        self.dlv_audit(
            &actor,
            "ci.rerun",
            &sandbox.slug,
            &format!("{} at {}", run.reference.key(), run.sha),
        )
        .await;
        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
        Ok(run_view(&run, &jobs, false))
    }

    /// `POST /api/sandboxes/{sid}/ci/sync`: looks for new heads now.
    ///
    /// # Errors
    /// Access errors, GitHub or Git failures.
    pub async fn ci_sync_now(&self, ctx: &AuthContext, sandbox_id: &str) -> AppResult<CiOverview> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        self.ci_sync_sandbox(&sandbox).await?;
        self.ci_overview(ctx, sandbox_id).await
    }
}

#[cfg(test)]
mod planning_tests {
    use super::*;

    fn run(
        sandbox: &SandboxId,
        id: &str,
        sha: &str,
        reference: CiRef,
        state: RunState,
        reason: Option<String>,
    ) -> CiRunRecord {
        CiRunRecord {
            id: id.into(),
            sandbox_id: sandbox.clone(),
            reference,
            sha: sha.into(),
            source: dom::GateSource::ProjectConfig,
            state,
            reason,
            requested_by: "yulius".into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(0),
            started_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn a_shared_config_repair_invalidates_an_older_owner_verdict()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = SandboxId::parse("s1")?;
        let replacement = run(
            &sandbox,
            "repair",
            "a",
            CiRef::PullRequest(3),
            RunState::Queued,
            None,
        );
        let refusal = run(
            &sandbox,
            "refused",
            "a",
            CiRef::PullRequest(2),
            RunState::Failed,
            Some(format!("{DEV_INVALID_PREFIX}dev")),
        );
        for (state, expected) in [
            (RunState::Passed, "repair"),
            (RunState::Failed, "repair"),
            (RunState::Running, "owner"),
            (RunState::Cancelled, "owner"),
        ] {
            let owner = run(&sandbox, "owner", "a", CiRef::PullRequest(1), state, None);
            let records = [replacement.clone(), refusal.clone(), owner];
            assert_eq!(
                run_for_pull_head(&records, 1).map(|r| r.id.as_str()),
                Some(expected),
                "{state:?}"
            );
        }
        let mut private_refusal = refusal;
        private_refusal.reference = CiRef::Train(vec![2]);
        let owner = run(
            &sandbox,
            "owner",
            "a",
            CiRef::PullRequest(1),
            RunState::Passed,
            None,
        );
        let records = [replacement, private_refusal, owner];
        assert_eq!(
            run_for_pull_head(&records, 1).map(|r| r.id.as_str()),
            Some("owner")
        );
        Ok(())
    }

    #[test]
    fn a_repaired_configuration_preserves_active_runs_and_human_cancellations()
    -> Result<(), Box<dyn std::error::Error>> {
        let sandbox = SandboxId::parse("s1")?;
        let pr = CiRef::PullRequest(1);
        let refusal = Some(format!("{DEV_INVALID_PREFIX}dev"));
        let records = [
            run(
                &sandbox,
                "replacement",
                "a",
                pr.clone(),
                RunState::Passed,
                None,
            ),
            run(
                &sandbox,
                "refusal",
                "a",
                pr.clone(),
                RunState::Failed,
                refusal.clone(),
            ),
            run(
                &sandbox,
                "obsolete",
                "a",
                pr.clone(),
                RunState::Failed,
                None,
            ),
            run(&sandbox, "queued", "a", pr.clone(), RunState::Queued, None),
            run(
                &sandbox,
                "cancel",
                "a",
                pr.clone(),
                RunState::Cancelled,
                None,
            ),
            run(
                &sandbox,
                "private",
                "b",
                CiRef::Train(vec![1]),
                RunState::Failed,
                refusal,
            ),
            run(&sandbox, "other", "b", pr, RunState::Passed, None),
        ];
        let ids = |valid| {
            ordinary_run_facts(&records, valid)
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(true), ["replacement", "queued", "cancel", "other"]);
        assert_eq!(
            ids(false),
            [
                "replacement",
                "refusal",
                "obsolete",
                "queued",
                "cancel",
                "other"
            ]
        );
        Ok(())
    }
}

#[cfg(test)]
mod ran_as_tests {
    use super::{CiJobRecord, Job, RunState, ran_as};
    use yulius_domain::resources::Need;

    /// SPEC-504 review round 5: a record is a job's run only where every field that changes how
    /// it runs is the same; a change to any one of them is a change.
    #[test]
    fn a_record_ran_a_job_only_with_all_of_its_definition() {
        let job = Job {
            name: "check".into(),
            key: "check".into(),
            command: "echo same".into(),
            matrix: Some("a".into()),
            need: Need {
                cpus: Some(2),
                memory_mb: Some(1024),
            },
            secrets: vec!["TOKEN".into()],
            timeout_minutes: Some(30),
            exclusive: true,
            paths: vec!["crates/**".into()],
            after_merge: false,
        };
        let record = |job: &Job| CiJobRecord {
            id: "j".into(),
            run_id: "r".into(),
            ordinal: 0,
            name: job.name.clone(),
            key: job.key.clone(),
            command: job.command.clone(),
            matrix: job.matrix.clone(),
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: String::new(),
            failures: None,
            started_at: None,
            ended_at: None,
            need: job.need,
            timeout_minutes: job.timeout_minutes,
            exclusive: job.exclusive,
            waited_ms: None,
        };
        assert!(ran_as(&record(&job), &job));
        type Change = fn(&mut Job);
        let changes: [(&str, Change); 7] = [
            ("key", |j| j.key = "lint".into()),
            ("command", |j| j.command = "echo new".into()),
            ("matrix", |j| j.matrix = Some("b".into())),
            ("cpus", |j| j.need.cpus = Some(4)),
            ("memory", |j| j.need.memory_mb = Some(2048)),
            ("time limit", |j| j.timeout_minutes = Some(20)),
            ("exclusive", |j| j.exclusive = false),
        ];
        for (what, change) in changes {
            let mut ran = job.clone();
            change(&mut ran);
            assert!(!ran_as(&record(&ran), &job), "another {what}");
        }
    }
}

#[cfg(test)]
mod lane_tests {
    use super::{
        busy_in, forget_idle_turn, lane_note, preferred_lanes, reserve_run, reuse_source,
        take_lane, take_lane_preferring,
    };

    /// SPEC-497 SHR-002 (review round 2): the note names a preferred lane as such, first choice
    /// or not; a lane no preference chose is only "taken".
    #[test]
    fn the_lane_note_says_whether_the_run_builds_on_it() {
        let built = "which this run builds on";
        assert!(
            lane_note(1, Some("pr/3"), &[3, 1]).ends_with(built),
            "second choice"
        );
        assert!(lane_note(3, Some("pr/5"), &[3, 1]).ends_with(built));
        assert_eq!(
            lane_note(0, Some("branch/dev"), &[3, 1]),
            "lane 0, last taken by branch/dev"
        );
        assert_eq!(lane_note(2, None, &[]), "lane 2");
    }

    /// Slice D review round 2: a passed run of the very commit a candidate is formed again on is a
    /// source; for any other target, a run of the same commit is not, nor a train's step verdict
    /// or a run that did not pass.
    #[test]
    fn a_candidate_on_the_same_commit_reuses_its_run() {
        use crate::ci::CiRunRecord;
        use yulius_domain::ci::{GateSource, RunState, Target};
        use yulius_domain::ids::SandboxId;
        use yulius_domain::time::Timestamp;
        let run = |sha: &str, state, by: &str| CiRunRecord {
            id: "r".into(),
            sandbox_id: SandboxId::parse("s1").unwrap_or_else(|_| unreachable!()),
            reference: CiRef::Train(vec![1]),
            sha: sha.into(),
            source: GateSource::ProjectConfig,
            state,
            reason: None,
            requested_by: by.into(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: Timestamp(0),
            started_at: None,
            ended_at: None,
        };
        let target = |reference, sha: &str| Target {
            reference,
            sha: sha.into(),
        };
        let candidate = target(CiRef::Train(vec![1]), "c1");
        assert!(reuse_source(
            &run("c1", RunState::Passed, "yulius"),
            &candidate
        ));
        assert!(reuse_source(
            &run("c0", RunState::Passed, "yulius"),
            &candidate
        ));
        assert!(!reuse_source(
            &run("c1", RunState::Failed, "yulius"),
            &candidate
        ));
        assert!(!reuse_source(
            &run("c0", RunState::Passed, crate::merge_train::TRAIN_VERDICT_BY),
            &candidate
        ));
        let branch = target(CiRef::Branch("dev".into()), "c1");
        assert!(!reuse_source(
            &run("c1", RunState::Passed, "yulius"),
            &branch
        ));
        assert!(reuse_source(
            &run("c0", RunState::Passed, "yulius"),
            &branch
        ));
    }
    use yulius_domain::ci::CiRef;

    /// SPEC-497 SHR-002: a run takes the lane whose checkouts last built what it builds on,
    /// when it is free: a pull request its own previous run's, a candidate its members' (the last
    /// member first), then an earlier candidate's sharing a member; otherwise the first free.
    #[test]
    fn a_run_takes_the_lane_that_last_built_what_it_builds_on() {
        let last = [
            (0, CiRef::PullRequest(7)),
            (1, CiRef::PullRequest(3)),
            (2, CiRef::Train(vec![3, 9])),
            (3, CiRef::PullRequest(5)),
        ];
        assert_eq!(preferred_lanes(&CiRef::PullRequest(3), &last), [1]);
        assert_eq!(
            preferred_lanes(&CiRef::Train(vec![3, 5]), &last),
            [3, 1, 2],
            "the last member first, then a candidate sharing #3"
        );
        assert!(preferred_lanes(&CiRef::Branch("dev".into()), &last).is_empty());
        assert!(preferred_lanes(&CiRef::PullRequest(4), &last).is_empty());
        let mut busy = std::collections::HashSet::new();
        assert_eq!(take_lane_preferring(&mut busy, "s1", 4, &[3, 1]), Some(3));
        assert_eq!(
            take_lane_preferring(&mut busy, "s1", 4, &[3, 1]),
            Some(1),
            "3 runs"
        );
        assert_eq!(
            take_lane_preferring(&mut busy, "s1", 4, &[3, 1]),
            Some(0),
            "the first free"
        );
        assert_eq!(
            take_lane_preferring(&mut busy, "s1", 4, &[9]),
            Some(2),
            "a lane past the count is none"
        );
        assert_eq!(take_lane_preferring(&mut busy, "s1", 4, &[]), None);
        let mut claimed = std::collections::HashSet::from(["s2".to_owned()]);
        assert_eq!(
            take_lane_preferring(&mut claimed, "s2", 4, &[1]),
            None,
            "claimed"
        );
    }

    /// SPEC-435: an exclusive job's turn stays while a job holds or waits for it, and goes once
    /// nobody does, so the map does not keep every job name ever seen.
    #[test]
    fn an_idle_turn_is_forgotten_and_a_held_one_kept() {
        let mut turns = std::collections::HashMap::new();
        let held = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        turns.insert("s1/site".to_owned(), std::sync::Arc::clone(&held));
        forget_idle_turn(&mut turns, "s1/site");
        assert!(turns.contains_key("s1/site"), "held: kept");
        drop(held);
        forget_idle_turn(&mut turns, "s1/site");
        assert!(turns.is_empty(), "idle: forgotten");
        forget_idle_turn(&mut turns, "s1/none");
    }

    use std::collections::{HashMap, HashSet};

    #[test]
    fn a_sandbox_runs_as_many_runs_as_it_has_lanes_and_never_while_claimed() {
        let mut busy = HashSet::new();
        assert_eq!(take_lane(&mut busy, "s1", 2), Some(0));
        assert_eq!(take_lane(&mut busy, "s1", 2), Some(1));
        assert_eq!(take_lane(&mut busy, "s1", 2), None, "both lanes run");
        assert_eq!(
            take_lane(&mut busy, "s2", 2),
            Some(0),
            "sandboxes are independent"
        );
        assert!(
            busy_in(&busy, "s1") && !busy_in(&busy, "s"),
            "no prefix confusion"
        );
        busy.remove("s1/0");
        assert_eq!(
            take_lane(&mut busy, "s1", 2),
            Some(0),
            "a freed lane is reused"
        );
        let mut claimed: HashSet<String> = ["s3".to_owned()].into();
        assert_eq!(
            take_lane(&mut claimed, "s3", 2),
            None,
            "a disk claim holds every lane"
        );
    }

    /// #738: a dispatch that reads the run still queued before its executor marked it running
    /// takes no second lane and keeps the first executor's cancellation.
    #[test]
    fn a_run_with_an_executor_is_not_reserved_again() {
        let (mut busy, mut executing) = (HashSet::new(), HashMap::new());
        assert_eq!(
            reserve_run(&mut busy, &mut executing, ("r1", "s1", 2), &[], "first"),
            Some(0)
        );
        assert_eq!(
            reserve_run(&mut busy, &mut executing, ("r1", "s1", 2), &[], "second"),
            None,
            "the run already executes"
        );
        assert_eq!(
            executing.get("r1"),
            Some(&"first"),
            "its cancellation stays"
        );
        assert!(!busy.contains("s1/1"), "no second lane: {busy:?}");
        assert_eq!(
            reserve_run(&mut busy, &mut executing, ("r2", "s1", 2), &[], "other"),
            Some(1),
            "another run takes the free lane"
        );
        busy.insert("s2".to_owned());
        assert_eq!(
            reserve_run(&mut busy, &mut executing, ("r3", "s2", 2), &[], "claimed"),
            None
        );
        assert!(
            !executing.contains_key("r3"),
            "a refused run registers nothing"
        );
    }
}
