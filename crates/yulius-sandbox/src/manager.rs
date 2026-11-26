//! The server's side of sandbox isolation (SPEC-002 ADR-22): holders on demand, their relays,
//! limits, quotas, idle stops, base builds and adoption after a restart.
//!
//! Ownership and bounds:
//! - **One holder per sandbox**, started by [`Manager::prepare`] under a per-sandbox lock.
//!   Starting runs `yulius sandbox-host start` as a child process within [`START_TIMEOUT`].
//!   Production-stage runs get a second one, the sandbox's production holder
//!   ([`Manager::prepare_production`], SPEC-279): its own namespaces, cgroup, system layer and
//!   Docker engine under the name `<sandbox>.prod`, with no forwarded ports and no relays.
//! - **Leases.** A run holds a lease from `prepare` until its process has joined
//!   ([`Manager::release`]); a sandbox with a lease or a process in its network namespace is
//!   never idle.
//! - **[`Manager::tick`]** (every [`TICK`]):
//!   - reaps holders that died;
//!   - restarts a dead `slirp4netns`;
//!   - keeps a Docker front beside every live holder, whatever its age, and moves the Docker
//!     directory into runs that started without it (ADR-0158);
//!   - stops holders idle for [`IdleConfig::stop_after`], and a retired sandbox's holder
//!     ([`Manager::retire`]) as soon as no lease is left, whatever still runs in it;
//!   - refreshes disk usage (at most every [`USAGE_EVERY`] per sandbox);
//!   - builds a new base when the host changed for [`REBUILD_DEBOUNCE`], one build at a time,
//!     as a child process.
//! - **Relays.** Each live holder has an `out.sock` server and a task that keeps the host's
//!   loopback listeners in step with the ports listening inside (polled every second).
//!   Both end with the holder.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use yulius_contracts::ptyhost::SandboxJoin;
use yulius_contracts::sandbox::{
    DockerFrontRecord, DockerFrontSpec, DockerServe, EngineSpec, HOLDER_RECORD_FILE,
    HOLDER_RECORD_VERSION, HolderRecord, HolderSpec, IdMap, production_holder, production_runs_dir,
    production_sandbox, valid_holder_name,
};

use crate::base::{BaseInfo, Bases};
use crate::cgroup::{Delegation, Limits, SandboxGroups};
use crate::holder::{StatePaths, alive, read_json, write_json};
use crate::usage::Usage;

mod build;
mod docker;

pub use crate::system_layer::{DOCKERD_PATHS, own_engine, system_file_text, system_has_dockerd};

/// Bound on starting a holder.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a CI job's end waits for its last process to leave the job's group (SPEC-504).
const JOB_EXIT_WAIT: Duration = Duration::from_secs(5);
/// How often [`Manager::tick`] should run.
pub const TICK: Duration = Duration::from_secs(30);
/// Disk usage is measured at most this often per sandbox.
pub const USAGE_EVERY: Duration = Duration::from_secs(10 * 60);
/// A refusal over the disk quota measures again when its reading is older than this, so freeing
/// space helps at once.
pub const QUOTA_RECHECK: Duration = Duration::from_secs(60);
/// The host system must stay unchanged this long before a rebuild.
pub const REBUILD_DEBOUNCE: Duration = Duration::from_secs(10 * 60);
/// The host's fingerprint is read at most this often.
pub const FINGERPRINT_EVERY: Duration = Duration::from_secs(30 * 60);
/// Bound on a base build.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How often the host side asks a holder which ports listen inside.
pub const PORTS_POLL: Duration = Duration::from_secs(1);
/// Bound on a Docker front reporting that it serves.
pub const FRONT_START_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on one `bind-docker` pass.
pub const BIND_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts to move the Docker directory into one run's mount namespace.
pub const BIND_TRIES: u8 = 3;
/// Marks a mount namespace that needs nothing more.
const BIND_DONE: u8 = u8::MAX;
/// The marker a removed sandbox's state directory gets ([`Manager::retire`], SPEC-206 RMV-004).
const RETIRED: &str = "retired";
/// How long a retired sandbox's holder outlives its retirement: its tasks' stop escalation
/// (about 20 s) and its shells' exit grace end their runs first, with egress and the engine
/// still there (SPEC-206 RMV-004).
pub const RETIRE_GRACE: Duration = Duration::from_secs(60);

/// Why sandboxes get no engine: `YULIUS_SANDBOX_DOCKER=off` (SPEC-196 DKL-005).
pub const DOCKER_OFF: &str = "Docker is turned off on this Yulius server \
    (YULIUS_SANDBOX_DOCKER=off); an operator removes that setting and restarts Yulius";
/// Why: the sandbox's system has no `dockerd` (SPEC-199 DSY-003). Its front switches to an engine
/// at the next tick after one is installed in the sandbox. Ubuntu's `docker.io` only suggests
/// buildx and compose, so both installs name them (SPEC-201 DBX-001/002).
pub const DOCKER_NO_DOCKERD: &str = "this sandbox's system has no `dockerd`: install Docker in the \
    sandbox (`sudo apt-get install -y docker.io docker-buildx docker-compose-v2`) and the engine \
    starts within a minute, or an operator runs `sudo yulius host setup`, which installs Docker on \
    the host (docker-ce with docker-buildx-plugin and docker-compose-plugin), and the sandbox gets \
    it when its holder next starts on a newer base";
/// Why: no cgroup delegation.
pub const DOCKER_NO_DELEGATION: &str = "the Yulius server has no cgroup delegation; an operator \
    sets Delegate=yes and DelegateSubgroup=server on yulius.service and restarts it \
    (docs/operations.md)";
/// Why: the sandbox's cgroup could not be prepared.
pub const DOCKER_NO_CGROUP: &str = "the sandbox's cgroup could not be prepared (the Yulius \
    server's log says why); an operator checks the delegation with `yulius doctor`";
/// Why: the executable could be rewritten by an engine (ADR-0151).
pub const DOCKER_WRITABLE_EXE: &str = "the `yulius` executable is owned or writable by the \
    service user, which an engine could rewrite; an operator installs it owned by root, mode 755";

/// Idle handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleConfig {
    /// A holder without runs or leases is stopped after this long.
    pub stop_after: Duration,
}

/// Everything the manager needs (see `docs/operations.md` for the settings).
#[derive(Debug, Clone)]
pub struct IsolationConfig {
    /// The data directory.
    pub data_dir: PathBuf,
    /// The `yulius` executable.
    pub exe: PathBuf,
    /// `bwrap`.
    pub bwrap: PathBuf,
    /// `newuidmap`.
    pub newuidmap: PathBuf,
    /// `newgidmap`.
    pub newgidmap: PathBuf,
    /// `slirp4netns`; `None`: no egress besides the relays.
    pub slirp4netns: Option<PathBuf>,
    /// User ids.
    pub uid: IdMap,
    /// Group ids.
    pub gid: IdMap,
    /// The delegated cgroup subtree, when the unit has one.
    pub delegation: Option<Delegation>,
    /// Limits per sandbox.
    pub limits: Limits,
    /// Soft disk quota per sandbox in bytes.
    pub disk_max: Option<u64>,
    /// Ports inside every sandbox relayed to targets on the host.
    pub forwards: BTreeMap<u16, SocketAddr>,
    /// The `sudo` shim written into bases.
    pub sudo_shim: String,
    /// Idle handling.
    pub idle: IdleConfig,
    /// Build bases automatically (off in tests that bring their own).
    pub auto_build: bool,
    /// Give each sandbox a Docker engine (ADR-0151), or why not ([`DOCKER_OFF`]); it also needs
    /// the cgroup delegation and `dockerd` in the sandbox's system ([`system_has_dockerd`]).
    pub docker: Result<(), String>,
    /// Where the sandboxes' `out.sock` relays report their troubles (SPEC-361).
    pub health: Arc<yulius_domain::connection_health::ConnectionHealth>,
}

/// A sandbox asking to be entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRequest {
    /// Identifier.
    pub id: String,
    /// Host name inside.
    pub hostname: String,
    /// The sandbox directory (for the disk quota).
    pub dir: PathBuf,
    /// First port of its range.
    pub port_first: u16,
    /// Last port of its range.
    pub port_last: u16,
}

/// What a run needs to join a sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    /// The ptyhost contract.
    pub join: SandboxJoin,
    /// The cgroup CI jobs join instead of `join.cgroup` (SPEC-053 CPU-203).
    pub ci_cgroup: Option<PathBuf>,
    /// The text of the sandbox system's `/etc/passwd` ([`system_file_text`]).
    pub system_passwd: Option<String>,
    /// The text of its `/etc/group`.
    pub system_group: Option<String>,
}

/// Why `prepare` refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    /// Over the soft disk quota.
    #[error(
        "this sandbox uses {used} of its {limit} disk quota; free space (old worktrees, caches, \
         installed packages) before starting new sessions"
    )]
    OverQuota {
        /// Usage, human readable.
        used: String,
        /// Quota, human readable.
        limit: String,
    },
    /// The holder could not start.
    #[error("the sandbox could not be prepared: {0}")]
    Holder(String),
    /// The sandbox was removed ([`Manager::retire`]): nothing runs in it any more.
    #[error("the sandbox was removed")]
    Retired,
}

struct Live {
    record: HolderRecord,
    relays: Vec<JoinHandle<()>>,
    /// Its Docker front, once it serves (ADR-0158).
    front: Option<DockerFrontRecord>,
    /// Mount namespaces of its runs: attempts to bind the Docker directory, or [`BIND_DONE`].
    bound: HashMap<u64, u8>,
    /// What the journal said of its front's state (SPEC-494 DFJ-005).
    front_watch: crate::front_gate::StateWatch,
    /// When its front is restarted by itself (SPEC-494 DFJ-008).
    repair: crate::front_repair::RepairWatch,
    /// A restart whose new front did not start: counted and journaled once one starts.
    restart_pending: Option<crate::front_repair::Why>,
    /// When the journal last said that its front runs an earlier build and stays busy.
    outdated_said: Option<Instant>,
}

impl Live {
    fn new(record: HolderRecord, relays: Vec<JoinHandle<()>>) -> Self {
        Self {
            record,
            relays,
            front: None,
            bound: HashMap::new(),
            front_watch: crate::front_gate::StateWatch::default(),
            repair: crate::front_repair::RepairWatch::default(),
            restart_pending: None,
            outdated_said: None,
        }
    }

    /// The front's pid, when it has one.
    fn front_pid(&self) -> u32 {
        self.front.as_ref().map_or(0, |f| f.pid)
    }

    /// Whether its front may run an engine.
    fn serves_engine(&self) -> bool {
        self.front.as_ref().is_some_and(|f| f.mode != "refuse")
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        for task in &self.relays {
            task.abort();
        }
    }
}

#[derive(Default)]
struct SandboxState {
    live: Option<Live>,
    /// A holder that ended while runs may still use its namespaces (and its overlay mount): no
    /// new holder may mount the same upper layer until they are gone.
    stale: Option<HolderRecord>,
    leases: usize,
    last_used: Option<Instant>,
    usage: Option<(Usage, Instant)>,
    measuring: bool,
    request: Option<SandboxRequest>,
}

#[derive(Default)]
struct BuildState {
    pending: Option<(String, Instant)>,
    /// When the pending change was first seen (ms since the epoch).
    pending_at_ms: Option<i64>,
    checked: Option<Instant>,
    last_error: Option<String>,
    /// When the running build started (ms since the epoch).
    building_since_ms: Option<i64>,
    /// How long the last successful build took.
    last_build: Option<Duration>,
}

/// Where the base's rebuild stands, for Settings → Instance (SPEC-203 HST-014). Times are ms
/// since the epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildStatus {
    /// A build runs, since then.
    pub building_since_ms: Option<i64>,
    /// The host changed then; a build starts once it stays unchanged for [`REBUILD_DEBOUNCE`].
    pub pending_since_ms: Option<i64>,
    /// When that build starts.
    pub starts_at_ms: Option<i64>,
    /// How long the last build took (an estimate for the next).
    pub last_build_secs: Option<u64>,
    /// The last build's error.
    pub last_error: Option<String>,
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Whether the host's fingerprint is due: every [`FINGERPRINT_EVERY`], at once without a base,
/// and at every tick while the host has `dockerd` and the current base has none, so that a
/// Docker installed on the host reaches new bases after the debounce, not up to half an hour
/// later (SPEC-203 HST-016).
#[must_use]
pub fn fingerprint_due(
    checked: Option<Duration>,
    has_base: bool,
    base_lacks_dockerd: bool,
) -> bool {
    !has_base || base_lacks_dockerd || checked.is_none_or(|ago| ago >= FINGERPRINT_EVERY)
}

/// A summary for logs, the status panel and `yulius doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationStatus {
    /// The current base.
    pub base: Option<BaseInfo>,
    /// A build is running.
    pub building: bool,
    /// The last build error.
    pub build_error: Option<String>,
    /// Sandboxes with a live holder.
    pub holders: usize,
    /// Limits apply.
    pub limits: bool,
    /// Egress is direct.
    pub egress: bool,
}

/// See the module docs.
pub struct Manager {
    config: IsolationConfig,
    /// Restart controls of the active host-side outbound relays.
    #[cfg(target_os = "linux")]
    pub outbound_relays: Arc<crate::relay::handoff::OutboundRelays>,
    bases: Bases,
    /// A purge may remove the durable retirement marker while old requests still exist.
    retirements: Mutex<HashMap<String, Instant>>,
    sandboxes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<SandboxState>>>>,
    build: Mutex<BuildState>,
    building: AtomicBool,
    /// The disk quota in force (bytes; 0: none): the configuration's until
    /// [`Manager::set_disk_quota`] changes it.
    disk_max: AtomicU64,
    /// Fronts restarted since the server started, by repair or on request (SPEC-494).
    docker_restarts: AtomicU64,
    /// Events of the sandboxes' fronts for the instance's measures, until the app takes them
    /// (SPEC-494 DFJ-013); bounded by [`DOCKER_EVENTS_MAX`].
    docker_events: Mutex<std::collections::VecDeque<DockerEvent>>,
    /// A sweep of CI jobs' leftovers is under way (SPEC-504 CLB-005): the next tick starts
    /// none beside it.
    sweeping: Arc<std::sync::atomic::AtomicBool>,
}

/// Most front events kept until the app takes them (it does every few seconds).
pub const DOCKER_EVENTS_MAX: usize = 4096;

/// An event of a sandbox's front the instance counts (SPEC-494 DFJ-013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerEvent {
    /// The sandbox (a production holder's name for its production engine).
    pub sandbox: String,
    /// `restarted` (the front and engine, by a repair, a request or an update), `gave_way` or
    /// `ci_leftovers` (SPEC-504 CLB-006: `detail` is `<containers> <networks> <job-end|sweep>`).
    pub kind: &'static str,
    /// How many (give-ways since the last tick; 1 for a restart).
    pub count: u64,
    /// Why, for a restart.
    pub detail: String,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether `path` lies below `sandboxes/` of `data`, which a sandbox's runs can rearrange (any
/// folder may become a link), or is a production run's files (SPEC-279 PDE-004) or a CD cache
/// (SPEC-300): removed only by the walk from the data directory (ADR-0176).
fn reachable_by_runs(data: &Path, path: &Path) -> bool {
    path.starts_with(data.join("sandboxes")) || production_run_files(data, path)
}

/// Whether `path` is (or lies below) one production run's files, `cd/<sandbox>/runs/<run>`, or
/// one CD environment's cache, `cd/<sandbox>/cache/<name>`, which production runs write too.
fn production_run_files(data: &Path, path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(data.join("cd")) else {
        return false;
    };
    let parts: Vec<_> = rest.components().collect();
    parts.len() >= 3
        && parts
            .iter()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        && (parts[1].as_os_str() == "runs" || parts[1].as_os_str() == "cache")
        && parts[0].as_os_str().to_str().is_some_and(valid_name)
}

/// Whether `name` is a holder name as the server makes them: a sandbox id (ASCII letters,
/// digits, `-` and `_`, at most 64) or its production holder's (`<id>.prod`, SPEC-279): one path
/// component that cannot leave its directory.
fn valid_name(name: &str) -> bool {
    valid_holder_name(name)
}

/// The production holder's host name for a sandbox named `hostname`: `<hostname>-prod`, the
/// sandbox's part cut so the whole stays a host name (63).
fn production_hostname(hostname: &str) -> String {
    let cut: String = hostname.chars().take(58).collect();
    format!("{cut}-prod")
}

/// The path component of the sandbox `name`: the name itself, or for anything else a fixed
/// component no sandbox has (`.invalid`), so a path built from it never leaves its tree.
pub(crate) fn path_name(name: &str) -> &str {
    if valid_name(name) { name } else { ".invalid" }
}

/// Bytes as "12.3 GB".
#[must_use]
pub fn human(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64 / GB;
    format!("{value:.1} GB")
}

impl Manager {
    /// A manager for `config`.
    #[must_use]
    pub fn new(config: IsolationConfig) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(target_os = "linux")]
            outbound_relays: Arc::default(),
            bases: Bases::new(&config.data_dir),
            disk_max: AtomicU64::new(config.disk_max.unwrap_or(0)),
            docker_restarts: AtomicU64::new(0),
            docker_events: Mutex::new(std::collections::VecDeque::new()),
            sweeping: Arc::default(),
            config,
            retirements: Mutex::default(),
            sandboxes: Mutex::new(HashMap::new()),
            build: Mutex::new(BuildState::default()),
            building: AtomicBool::new(false),
        })
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &IsolationConfig {
        &self.config
    }

    /// The soft disk quota per sandbox in force (bytes); `None`: none.
    fn disk_max(&self) -> Option<u64> {
        Some(self.disk_max.load(Ordering::Relaxed)).filter(|bytes| *bytes > 0)
    }

    /// Changes the soft disk quota per sandbox (bytes; `None` or 0: none) while the manager
    /// runs (SPEC-459 HSL-006): the next check of a sandbox compares against it.
    pub fn set_disk_quota(&self, quota: Option<u64>) {
        self.disk_max.store(quota.unwrap_or(0), Ordering::Relaxed);
    }

    fn state_dir(&self, sandbox: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runtime")
            .join("sandboxes")
            .join(path_name(sandbox))
    }

    fn sysroot(&self, sandbox: &str) -> PathBuf {
        self.config
            .data_dir
            .join("sysroots")
            .join(path_name(sandbox))
    }

    /// The request of a sandbox known only from its holder's files (an adopted holder before
    /// any run asked for it): its host name and ports from `holder.json`.
    fn adopted_request(&self, sandbox: &str) -> SandboxRequest {
        let spec = std::fs::read(StatePaths::new(self.state_dir(sandbox)).spec())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        let field = |name: &str| spec.as_ref().and_then(|s| s[name].as_u64());
        let port = |name: &str| field(name).and_then(|p| u16::try_from(p).ok()).unwrap_or(0);
        SandboxRequest {
            id: sandbox.to_owned(),
            hostname: spec
                .as_ref()
                .and_then(|s| s["hostname"].as_str())
                .map_or_else(|| sandbox.to_owned(), str::to_owned),
            dir: match production_sandbox(sandbox) {
                Some(of) => production_runs_dir(&self.config.data_dir, of),
                None => self
                    .config
                    .data_dir
                    .join("sandboxes")
                    .join(path_name(sandbox)),
            },
            port_first: port("port_first"),
            port_last: port("port_last"),
        }
    }

    /// The request of `sandbox`'s production holder (SPEC-279 PDE-001): its name, a host name
    /// of its own, the production runs directory (what its engine binds) and no dev ports.
    #[must_use]
    pub fn production_request(&self, sandbox: &SandboxRequest) -> SandboxRequest {
        SandboxRequest {
            id: production_holder(&sandbox.id),
            hostname: production_hostname(&sandbox.hostname),
            dir: production_runs_dir(&self.config.data_dir, &sandbox.id),
            port_first: 0,
            port_last: 0,
        }
    }

    /// Ports forwarded out of the holder `name` to the host: none for a production holder,
    /// whose runs reach only the Internet (SPEC-279 PDE-001).
    fn forwards_of(&self, name: &str) -> Vec<u16> {
        if production_sandbox(name).is_some() {
            Vec::new()
        } else {
            self.config.forwards.keys().copied().collect()
        }
    }

    fn entry(&self, sandbox: &str) -> Arc<tokio::sync::Mutex<SandboxState>> {
        Arc::clone(lock(&self.sandboxes).entry(sandbox.to_owned()).or_default())
    }

    /// The current base (`None` until one is built).
    #[must_use]
    pub fn base(&self) -> Option<BaseInfo> {
        self.bases.current()
    }

    /// Where the base's rebuild stands (SPEC-203 HST-014).
    #[must_use]
    pub fn rebuild(&self) -> RebuildStatus {
        let build = lock(&self.build);
        let debounce = i64::try_from(REBUILD_DEBOUNCE.as_millis()).unwrap_or(i64::MAX);
        RebuildStatus {
            building_since_ms: build
                .building_since_ms
                .filter(|_| self.building.load(Ordering::SeqCst)),
            pending_since_ms: build.pending_at_ms,
            starts_at_ms: build.pending_at_ms.map(|at| at.saturating_add(debounce)),
            last_build_secs: build.last_build.map(|d| d.as_secs()),
            last_error: build.last_error.clone(),
        }
    }

    /// Status (see [`IsolationStatus`]).
    pub async fn status(&self) -> IsolationStatus {
        let entries: Vec<_> = lock(&self.sandboxes).values().cloned().collect();
        let mut holders = 0;
        for entry in entries {
            if entry.lock().await.live.is_some() {
                holders += 1;
            }
        }
        IsolationStatus {
            base: self.bases.current(),
            building: self.building.load(Ordering::SeqCst),
            build_error: lock(&self.build).last_error.clone(),
            holders,
            limits: self.config.delegation.is_some(),
            egress: self.config.slirp4netns.is_some(),
        }
    }

    /// Enters `request`'s sandbox: its holder (started when needed) and a lease the caller ends
    /// with [`Manager::release`] once its run has joined. `Ok(None)` while no base exists yet (the
    /// run uses the previous containment).
    ///
    /// # Errors
    /// Over the disk quota, or the holder cannot start.
    pub async fn prepare(&self, request: &SandboxRequest) -> Result<Option<Joined>, PrepareError> {
        self.prepare_holder(request, true).await
    }

    /// Enters `sandbox`'s production holder (SPEC-279 PDE-003), started when needed, with a
    /// lease the caller ends with [`Manager::release`] of [`production_holder`]. Refused while
    /// the sandbox is retired or over its quota without the production holder's own files,
    /// which count toward it: those never refuse a deploy, whose engine the upkeep then prunes
    /// (review finding 1). `Ok(None)` while no base exists yet.
    ///
    /// # Errors
    /// Retired, over the disk quota, or the holder cannot start.
    pub async fn prepare_production(
        &self,
        sandbox: &SandboxRequest,
    ) -> Result<Option<Joined>, PrepareError> {
        if self.retired(&sandbox.id) {
            return Err(PrepareError::Retired);
        }
        {
            let entry = self.entry(&sandbox.id);
            let mut state = entry.lock().await;
            // Measured by the upkeep even while the sandbox's own holder never ran.
            if state.request.is_none() {
                state.request = Some(sandbox.clone());
            }
            if let Some(refusal) = self.quota_refusal(sandbox, &mut state).await {
                let limit = self.disk_max().unwrap_or(u64::MAX);
                let without_production = state.usage.as_ref().map_or(u64::MAX, |(usage, _)| {
                    let production = usage
                        .largest
                        .iter()
                        .find(|(consumer, _)| consumer == "production")
                        .map_or(0, |(_, bytes)| *bytes);
                    usage.bytes.saturating_sub(production)
                });
                if without_production > limit {
                    return Err(refusal);
                }
            }
        }
        let production = self.production_request(sandbox);
        std::fs::create_dir_all(&production.dir)
            .map_err(|e| PrepareError::Holder(format!("{}: {e}", production.dir.display())))?;
        self.prepare_holder(&production, false).await
    }

    /// [`Manager::prepare`] for any holder name; `quota`: refuse over the holder's own quota
    /// reading (a production holder's is its sandbox's, checked by the caller).
    async fn prepare_holder(
        &self,
        request: &SandboxRequest,
        quota: bool,
    ) -> Result<Option<Joined>, PrepareError> {
        if self.retired(&request.id) {
            return Err(PrepareError::Retired);
        }
        let Some(base) = self.bases.current() else {
            return Ok(None);
        };
        let entry = self.entry(&request.id);
        let mut state = entry.lock().await;
        // Retirement may have been marked while this prepare waited for the holder.
        if self.retired(&request.id) {
            return Err(PrepareError::Retired);
        }
        state.request = Some(request.clone());
        if quota && let Some(refusal) = self.quota_refusal(request, &mut state).await {
            return Err(refusal);
        }
        let reusable = state.live.as_ref().is_some_and(|live| alive(&live.record));
        if !reusable {
            if let Some(live) = state.live.take() {
                let record = live.record.clone();
                drop(live);
                self.stop_holder(&request.id, &record).await;
                state.stale = Some(record);
            }
            if let Some(stale) = state.stale.clone() {
                let runs = tokio::task::spawn_blocking(move || Self::runs_in(stale.net_ns, &[]))
                    .await
                    .unwrap_or(1);
                if runs > 0 {
                    return Err(PrepareError::Holder(format!(
                        "the sandbox's previous holder ended while {runs} process(es) still use it; \
                         new sessions start when they end"
                    )));
                }
                state.stale = None;
            }
            let record = self
                .start_holder(request, &base)
                .await
                .map_err(PrepareError::Holder)?;
            let relays = self.start_relays(&request.id, &record);
            state.live = Some(Live::new(record, relays));
            self.keep_front(&request.id, &mut state).await;
        }
        let Some(record) = state.live.as_ref().map(|live| live.record.clone()) else {
            return Err(PrepareError::Holder("no holder".into()));
        };
        state.leases += 1;
        state.last_used = Some(Instant::now());
        let groups = self.cgroup_of(&request.id);
        let system_file = |name: &str| {
            system_file_text(
                &self.bases.path(&record.base_id),
                &self.sysroot(&request.id).join("upper"),
                name,
            )
        };
        Ok(Some(Joined {
            ci_cgroup: groups.as_ref().map(|g| g.ci.clone()),
            system_passwd: system_file("passwd"),
            system_group: system_file("group"),
            join: SandboxJoin {
                holder_pid: record.inner_pid,
                user_ns: record.user_ns,
                net_ns: record.net_ns,
                mnt_ns: record.mnt_ns,
                cgroup: groups.map(|g| g.agents.to_string_lossy().into_owned()),
                system_root: record.system_root.clone(),
                hostname: request.hostname.clone(),
                exec_shim: Some(self.config.exe.to_string_lossy().into_owned()),
                private_dirs: vec![self.config.data_dir.to_string_lossy().into_owned()],
                host_system: None,
                docker_dir: Some(
                    StatePaths::new(self.state_dir(&request.id))
                        .docker_dir()
                        .to_string_lossy()
                        .into_owned(),
                ),
            },
        }))
    }

    /// Processes running in `sandbox`'s namespaces besides its holder, as `/proc` shows them (0
    /// without a live holder). Yulius's own records of runs, shells and the browser are checked
    /// beside it.
    pub async fn processes(&self, sandbox: &str) -> usize {
        let (record, front) = {
            let entry = self.entry(sandbox);
            let state = entry.lock().await;
            match &state.live {
                Some(live) => (live.record.clone(), live.front_pid()),
                None => return 0,
            }
        };
        tokio::task::spawn_blocking(move || {
            Self::runs_in(record.net_ns, &[record.inner_pid, front])
        })
        .await
        .unwrap_or(1)
    }

    /// Whether a call from inside `sandbox` reaches Yulius's listener on `port` through its
    /// holder's forward (SPEC-355 SLR-005, [`crate::relay::probe_link`]): `None` without a live
    /// holder, for a production holder (it forwards nothing) and for a holder that does not offer
    /// the test.
    pub async fn probe_link(&self, sandbox: &str, port: u16) -> Option<bool> {
        if !valid_name(sandbox) || production_sandbox(sandbox).is_some() {
            return None;
        }
        let entry = lock(&self.sandboxes).get(sandbox).cloned()?;
        entry.lock().await.live.as_ref()?;
        let state = StatePaths::new(self.state_dir(sandbox));
        crate::relay::probe_link(&state.inbound(), port).await
    }

    /// Whether `sandbox`'s holder runs, ended while processes still use its namespaces (and its
    /// overlay mount), holds a lease, or is recorded alive in its state directory: its system
    /// layer and state are never deleted then (SPEC-215 LFT-002). A record that cannot be read
    /// counts as alive; only a missing one counts as none.
    pub async fn holder_live(&self, sandbox: &str) -> bool {
        if !valid_name(sandbox) {
            return false;
        }
        let entry = lock(&self.sandboxes).get(sandbox).cloned();
        if let Some(entry) = entry {
            let state = entry.lock().await;
            if state.live.is_some() || state.leases > 0 {
                return true;
            }
            if let Some(stale) = state.stale.clone() {
                let runs = tokio::task::spawn_blocking(move || Self::runs_in(stale.net_ns, &[]))
                    .await
                    .unwrap_or(1);
                if runs > 0 {
                    return true;
                }
            }
        }
        let record = self.state_dir(sandbox).join(HOLDER_RECORD_FILE);
        tokio::task::spawn_blocking(move || {
            // A record that is there but cannot be read (a partial write, a newer format) may
            // describe a live holder: kept, never guessed dead.
            match read_json::<HolderRecord>(&record) {
                Ok(record) => alive(&record),
                Err(err) => err.kind() != std::io::ErrorKind::NotFound,
            }
        })
        .await
        .unwrap_or(true)
    }

    /// Removes `path`, under `sysroots/`, `runtime/sandboxes/`, `system/` or `sandboxes/` of the
    /// data directory, or a production run's files (`cd/<sandbox>/runs/<run>`, SPEC-279), as root of a mapped namespace where the service user cannot (its files belong
    /// to a sandbox's mapped ids; SPEC-215). Below `sandboxes/`, which a sandbox's runs can
    /// rearrange, the removal walks down from the data directory without following a link
    /// (ADR-0176).
    ///
    /// # Errors
    /// `path` lies elsewhere, or the removal failed.
    pub async fn remove_owned_tree(&self, path: &Path) -> Result<(), String> {
        let data = &self.config.data_dir;
        let allowed = ([
            data.join("sysroots"),
            data.join("runtime").join("sandboxes"),
            data.join("system"),
            data.join("sandboxes"),
        ]
        .iter()
        .any(|root| path.starts_with(root) && path != root.as_path())
            || production_run_files(data, path))
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if !allowed {
            return Err(format!("{} is not a sandbox's system tree", path.display()));
        }
        let mapper = crate::userns::Mapper {
            bwrap: self.config.bwrap.clone(),
            newuidmap: self.config.newuidmap.clone(),
            newgidmap: self.config.newgidmap.clone(),
            uid: self.config.uid,
            gid: self.config.gid,
        };
        let exe = self.config.exe.clone();
        let path = path.to_path_buf();
        let root = data.clone();
        let reachable = reachable_by_runs(data, &path);
        tokio::task::spawn_blocking(move || {
            if reachable {
                crate::base::remove_tree_as_mapped_root(&mapper, &exe, &root, &path)
            } else {
                crate::base::remove_tree(&mapper, &exe, &root, &path)
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Bases that are not current and that no holder of this manager or recorded alive mounts
    /// (ADR-0161); none while a build runs.
    pub async fn unused_bases(&self) -> Vec<PathBuf> {
        if self.building.load(Ordering::SeqCst) {
            return Vec::new();
        }
        let entries: Vec<_> = lock(&self.sandboxes).values().cloned().collect();
        let mut in_use = Vec::new();
        for entry in entries {
            let state = entry.lock().await;
            if let Some(live) = &state.live {
                in_use.push(live.record.base_id.clone());
            }
            if let Some(stale) = &state.stale {
                in_use.push(stale.base_id.clone());
            }
        }
        let bases = self.bases.clone();
        tokio::task::spawn_blocking(move || crate::base::unused(&bases, &in_use))
            .await
            .unwrap_or_default()
    }

    /// Ends a lease taken by [`Manager::prepare`].
    pub async fn release(&self, sandbox: &str) {
        let entry = self.entry(sandbox);
        let mut state = entry.lock().await;
        state.leases = state.leases.saturating_sub(1);
        state.last_used = Some(Instant::now());
    }

    /// Retires `sandbox` (it was removed, SPEC-206 RMV-004): [`Manager::prepare`] refuses it,
    /// [`Manager::tick`] stops its holder (with its Docker engine and containers, and egress)
    /// [`RETIRE_GRACE`] after this once no run holds a lease, whatever still runs in it, and
    /// [`Manager::adopt_one`] stops it instead of adopting it. A marker in its state directory
    /// keeps it across restarts; its time is the retirement's.
    pub async fn retire(&self, sandbox: &str) {
        if self.mark_retired(sandbox, true).await {
            tracing::info!(
                sandbox,
                "sandbox retired: its holder stops once its runs end"
            );
        }
    }

    /// Whether retirement bars new holders and the last holder has ended. Repairs a
    /// missed retirement in existing holder directories without restoring purged files.
    pub async fn retired_ports_idle(&self, sandbox: &str) -> bool {
        self.mark_retired(sandbox, false).await && !self.holder_live(sandbox).await
    }

    async fn mark_retired(&self, sandbox: &str, create_dirs: bool) -> bool {
        if !valid_name(sandbox) {
            tracing::warn!(sandbox, "not a sandbox name: nothing retired");
            return false;
        }
        // Its production holder goes with it (SPEC-279 PDE-005).
        let mut names = vec![sandbox.to_owned()];
        if production_sandbox(sandbox).is_none() {
            names.push(production_holder(sandbox));
        }
        let dirs: Vec<_> = names.iter().map(|name| self.state_dir(name)).collect();
        {
            let mut retirements = lock(&self.retirements);
            for name in names {
                retirements.entry(name).or_insert_with(Instant::now);
            }
        }
        let marked = tokio::task::spawn_blocking(move || {
            for dir in dirs {
                if create_dirs {
                    std::fs::create_dir_all(&dir)?;
                }
                match std::fs::File::create_new(dir.join(RETIRED)) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) if !create_dirs && e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)));
        if let Err(error) = marked {
            tracing::warn!(sandbox, %error, "the sandbox could not be retired");
            return false;
        }
        true
    }

    /// Whether `sandbox` was retired ([`Manager::retire`]).
    fn retired(&self, sandbox: &str) -> bool {
        lock(&self.retirements).contains_key(sandbox)
            || self.state_dir(sandbox).join(RETIRED).exists()
    }

    /// How long ago `sandbox` was retired; `None` when it was not.
    fn retired_for(&self, sandbox: &str) -> Option<Duration> {
        std::fs::metadata(self.state_dir(sandbox).join(RETIRED))
            .and_then(|meta| meta.modified())
            .ok()
            .map(|modified| modified.elapsed().unwrap_or_default())
            .or_else(|| lock(&self.retirements).get(sandbox).map(Instant::elapsed))
    }

    fn cgroup_of(&self, sandbox: &str) -> Option<SandboxGroups> {
        let delegation = self.config.delegation.as_ref()?;
        match delegation.sandbox_group(sandbox, &self.config.limits) {
            Ok(groups) => Some(groups),
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot apply the sandbox's limits");
                None
            }
        }
    }

    async fn start_holder(
        &self,
        request: &SandboxRequest,
        base: &BaseInfo,
    ) -> Result<HolderRecord, String> {
        let state = StatePaths::new(self.state_dir(&request.id));
        let sysroot = self.sysroot(&request.id);
        let groups = self.cgroup_of(&request.id);
        for dir in [
            state.dir.clone(),
            state.root(),
            state.docker_dir(),
            sysroot.join("upper"),
            sysroot.join("work"),
            sysroot.join("docker"),
        ] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        for dir in [&state.dir, &sysroot] {
            let _ =
                std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        }
        let spec = HolderSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: request.id.clone(),
            hostname: request.hostname.clone(),
            state_dir: state.dir.to_string_lossy().into_owned(),
            base: self.bases.path(&base.id).to_string_lossy().into_owned(),
            base_id: base.id.clone(),
            upper: sysroot.join("upper").to_string_lossy().into_owned(),
            work: sysroot.join("work").to_string_lossy().into_owned(),
            exe: self.config.exe.to_string_lossy().into_owned(),
            bwrap: self.config.bwrap.to_string_lossy().into_owned(),
            newuidmap: self.config.newuidmap.to_string_lossy().into_owned(),
            newgidmap: self.config.newgidmap.to_string_lossy().into_owned(),
            slirp4netns: self
                .config
                .slirp4netns
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            uid: self.config.uid,
            gid: self.config.gid,
            cgroup: groups.map(|g| g.system.to_string_lossy().into_owned()),
            port_first: request.port_first,
            port_last: request.port_last,
            forwards: self.forwards_of(&request.id),
            containers: self.docker(),
        };
        spec.validate().map_err(|e| e.to_string())?;
        write_json(&state.spec(), &spec).map_err(|e| e.to_string())?;
        // The relay out of the sandbox must exist before the holder accepts connections.
        let _ = std::fs::remove_file(state.outbound());
        let mut command = tokio::process::Command::new(&self.config.exe);
        command
            .args(["sandbox-host", "start", "--spec"])
            .arg(state.spec())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let child = command.spawn().map_err(|e| format!("sandbox-host: {e}"))?;
        let output = tokio::time::timeout(START_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| "the sandbox holder did not start in time".to_owned())?
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            let text = String::from_utf8_lossy(&output.stderr);
            return Err(text.trim().chars().take(2000).collect());
        }
        let record: HolderRecord = read_json(&state.record()).map_err(|e| e.to_string())?;
        tracing::info!(
            sandbox = %request.id,
            base = %record.base_id,
            holder = record.inner_pid,
            egress = record.slirp_pid.is_some(),
            "sandbox holder started"
        );
        Ok(record)
    }

    fn start_relays(&self, sandbox: &str, record: &HolderRecord) -> Vec<JoinHandle<()>> {
        // A production holder forwards nothing and has no dev ports (SPEC-279 PDE-001).
        if production_sandbox(sandbox).is_some() {
            return Vec::new();
        }
        let state = StatePaths::new(self.state_dir(sandbox));
        let mut tasks = Vec::new();
        let _ = std::fs::remove_file(state.outbound());
        match tokio::net::UnixListener::bind(state.outbound()) {
            Ok(listener) => {
                let targets = Arc::new(self.config.forwards.clone());
                #[cfg(target_os = "linux")]
                let serving = self.outbound_relays.serve(
                    listener,
                    targets,
                    sandbox.to_owned(),
                    record.clone(),
                    Arc::clone(&self.config.health),
                );
                #[cfg(not(target_os = "linux"))]
                let serving = crate::relay::serve_outbound(
                    listener,
                    targets,
                    sandbox.to_owned(),
                    Arc::clone(&self.config.health),
                );
                tasks.push(tokio::spawn(serving));
            }
            Err(error) => tracing::warn!(sandbox, %error, "cannot serve the sandbox's out.sock"),
        }
        let in_sock = state.inbound();
        let holder = record.inner_pid;
        tasks.push(tokio::spawn(async move {
            let mut ports = crate::relay::InboundPorts::new(in_sock.clone());
            loop {
                match crate::relay::query_ports(&in_sock).await {
                    Ok(listening) => ports.sync(&listening).await,
                    Err(error) => {
                        tracing::debug!(holder, %error, "holder did not list its ports");
                        ports.sync(&[]).await;
                    }
                }
                tokio::time::sleep(PORTS_POLL).await;
            }
        }));
        tasks
    }

    async fn stop_holder(&self, sandbox: &str, record: &HolderRecord) {
        // Its front goes with it: no Docker trouble of it is left open (SPEC-494), on every path
        // that drops a holder.
        self.config.health.held(
            yulius_domain::connection_health::Limit::Docker,
            Some(sandbox),
            0,
        );
        let state = StatePaths::new(self.state_dir(sandbox));
        let record = record.clone();
        let _ = tokio::task::spawn_blocking(move || crate::holder::stop(&state, &record)).await;
        tracing::info!(sandbox, "sandbox holder stopped");
    }

    /// Adopts holders a previous server process started; removes the records of dead ones.
    /// The holders of a Docker smoke test that still runs are left to it (`smoke_runs`,
    /// ADR-0308).
    pub async fn adopt(&self) -> usize {
        let root = self.config.data_dir.join("runtime").join("sandboxes");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        let mut adopted = 0;
        for entry in entries.flatten().take(10_000) {
            let sandbox = entry.file_name().to_string_lossy().into_owned();
            if smoke_runs(&sandbox) {
                tracing::info!(sandbox, "left to the Docker smoke test that runs");
                continue;
            }
            if self.adopt_one(&sandbox).await {
                adopted += 1;
            }
        }
        if adopted > 0 {
            tracing::info!(
                adopted,
                "adopted sandbox holders of the previous server process"
            );
        }
        adopted
    }

    /// Adopts `sandbox`'s holder when it still runs, with its Docker front (kept when it still
    /// serves this holder in the right mode, else started; SPEC-196 DKL-003); removes the record
    /// of a dead one, and stops a retired sandbox's (SPEC-206 RMV-004). Returns whether a holder
    /// was adopted.
    pub async fn adopt_one(&self, sandbox: &str) -> bool {
        let state = StatePaths::new(self.state_dir(sandbox));
        let Ok(record) = read_json::<HolderRecord>(&state.record()) else {
            return false;
        };
        let slot = self.entry(sandbox);
        let mut guard = slot.lock().await;
        if alive(&record) && !self.retired(sandbox) {
            let relays = self.start_relays(sandbox, &record);
            let mut live = Live::new(record, relays);
            live.front = read_json::<DockerFrontRecord>(&state.docker_record()).ok();
            guard.live = Some(live);
            guard.last_used = Some(Instant::now());
            self.keep_front(sandbox, &mut guard).await;
            true
        } else {
            self.stop_holder(sandbox, &record).await;
            guard.stale = Some(record);
            false
        }
    }

    /// Processes in `net_ns` other than `exclude` (the holder and its Docker front): the
    /// sandbox's runs, and its engine while one runs.
    fn runs_in(net_ns: u64, exclude: &[u32]) -> usize {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        entries
            .flatten()
            .take(1 << 20)
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
            .filter(|pid| !exclude.contains(pid))
            .filter(|pid| crate::holder::ns_inode(*pid, "net") == Some(net_ns))
            .count()
    }

    /// One pass of upkeep (see the module docs).
    pub async fn tick(self: &Arc<Self>) {
        let entries: Vec<(String, Arc<tokio::sync::Mutex<SandboxState>>)> = lock(&self.sandboxes)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        let current = self.bases.current();
        let mut in_use = Vec::new();
        let mut sandboxes = Vec::new();
        for (sandbox, entry) in entries {
            let mut state = entry.lock().await;
            if let Some(live) = &state.live {
                let record = live.record.clone();
                if !alive(&record) {
                    tracing::warn!(
                        sandbox,
                        "sandbox holder ended; the next run starts a new one"
                    );
                    state.live = None;
                    self.stop_holder(&sandbox, &record).await;
                    state.stale = Some(record);
                } else {
                    if let Some(program) = &self.config.slirp4netns
                        && !crate::holder::egress_alive(&record)
                    {
                        let program = program.to_string_lossy().into_owned();
                        let paths = StatePaths::new(self.state_dir(&sandbox));
                        let mut updated = record.clone();
                        let system = self.cgroup_of(&sandbox).map(|groups| groups.system);
                        let restarted = tokio::task::spawn_blocking(move || {
                            crate::holder::restart_egress(
                                &program,
                                &paths,
                                &mut updated,
                                system.as_deref(),
                            )
                            .map(|()| updated)
                        })
                        .await;
                        match restarted {
                            Ok(Ok(updated)) => {
                                tracing::warn!(sandbox, "restarted the sandbox's egress");
                                if let Some(live) = state.live.as_mut() {
                                    live.record = updated;
                                }
                            }
                            Ok(Err(error)) => {
                                tracing::warn!(sandbox, %error, "cannot restart egress");
                            }
                            Err(_) => {}
                        }
                    }
                    self.keep_front(&sandbox, &mut state).await;
                    let reading = self.watch_front(&sandbox, &mut state);
                    self.repair_front(&sandbox, &mut state, reading).await;
                    self.replace_outdated_front(&sandbox, &mut state).await;
                    self.bind_docker_dirs(&sandbox, &mut state).await;
                    let front = state.live.as_ref().map_or(0, Live::front_pid);
                    let runs = tokio::task::spawn_blocking(move || {
                        Self::runs_in(record.net_ns, &[record.inner_pid, front])
                    })
                    .await
                    .unwrap_or(1);
                    if runs > 0 || state.leases > 0 {
                        state.last_used = Some(Instant::now());
                    }
                    let idle_for = state.last_used.map_or(Duration::MAX, |t| t.elapsed());
                    let superseded = current
                        .as_ref()
                        .is_some_and(|base| base.id != record.base_id);
                    let stop = holder_stops(
                        self.retired_for(&sandbox),
                        state.leases,
                        runs,
                        idle_for,
                        self.config.idle.stop_after,
                        superseded,
                    );
                    if stop {
                        let record = record.clone();
                        state.live = None;
                        self.stop_holder(&sandbox, &record).await;
                    } else if let Some(live) = &state.live {
                        in_use.push(live.record.base_id.clone());
                    }
                }
            }
            self.refresh_usage(&sandbox, &mut state);
            // Every sandbox's emptied job groups go; only a serving engine is asked for
            // leftovers (`remove_ci_leftovers` finds no socket otherwise).
            sandboxes.push(sandbox);
        }
        self.maybe_build(in_use);
        self.sweep_in_background(sandboxes);
    }

    /// Sweeps the CI jobs' leftovers and emptied job groups of `sandboxes` one after another in a
    /// task of its own, so an engine slow to answer holds no tick; none while the last one still
    /// runs.
    fn sweep_in_background(self: &Arc<Self>, sandboxes: Vec<String>) {
        use std::sync::atomic::Ordering;
        if sandboxes.is_empty()
            || self.config.delegation.is_none()
            || self.sweeping.swap(true, Ordering::SeqCst)
        {
            return;
        }
        /// Ends the sweep however its task ends.
        struct Swept(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Swept {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let swept = Swept(Arc::clone(&self.sweeping));
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let _swept = swept;
            // CI jobs never run in a production holder.
            for sandbox in sandboxes
                .iter()
                .filter(|sandbox| production_sandbox(sandbox).is_none())
            {
                manager.sweep_ci_leftovers(sandbox).await;
            }
        });
    }

    /// The refusal when the sandbox is over its quota. A reading older than [`QUOTA_RECHECK`]
    /// that says "over" is replaced by a fresh measurement first (space may have been freed).
    async fn quota_refusal(
        &self,
        request: &SandboxRequest,
        state: &mut SandboxState,
    ) -> Option<PrepareError> {
        let limit = self.disk_max()?;
        let (usage, at) = state.usage.as_ref()?;
        if usage.bytes <= limit {
            return None;
        }
        if at.elapsed() >= QUOTA_RECHECK {
            let (dir, sysroot) = (request.dir.clone(), self.sysroot(&request.id));
            let production = self.sysroot(&production_holder(&request.id));
            let fresh =
                tokio::task::spawn_blocking(move || measure_sandbox(&dir, &sysroot, &production))
                    .await
                    .ok()?;
            state.usage = Some((fresh, Instant::now()));
        }
        let (usage, _) = state.usage.as_ref()?;
        (usage.bytes > limit).then(|| PrepareError::OverQuota {
            used: human(usage.bytes),
            limit: human(limit),
        })
    }

    fn refresh_usage(self: &Arc<Self>, sandbox: &str, state: &mut SandboxState) {
        // A production holder's files are measured with its sandbox's (SPEC-279 PDE-006).
        if self.disk_max().is_none() || state.measuring || production_sandbox(sandbox).is_some() {
            return;
        }
        let Some(request) = state.request.clone() else {
            return;
        };
        let due = state
            .usage
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() >= USAGE_EVERY);
        if !due {
            return;
        }
        state.measuring = true;
        let manager = Arc::clone(self);
        let sandbox = sandbox.to_owned();
        let sysroot = self.sysroot(&sandbox);
        let production = self.sysroot(&production_holder(&sandbox));
        tokio::spawn(async move {
            let usage = tokio::task::spawn_blocking(move || {
                measure_sandbox(&request.dir, &sysroot, &production)
            })
            .await
            .unwrap_or_default();
            let entry = manager.entry(&sandbox);
            let mut state = entry.lock().await;
            state.measuring = false;
            state.usage = Some((usage, Instant::now()));
        });
    }

    /// Undoes the sandboxes' CPU split when the server stops ([`Delegation::unsplit_all`]).
    pub fn unsplit_cpu(&self) {
        if let Some(delegation) = &self.config.delegation {
            delegation.unsplit_all();
        }
    }

    /// Stops every holder (tests and shutdown of a test server).
    pub async fn stop_all(&self) {
        let entries: Vec<(String, Arc<tokio::sync::Mutex<SandboxState>>)> = lock(&self.sandboxes)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        for (sandbox, entry) in entries {
            let mut state = entry.lock().await;
            if let Some(live) = state.live.take() {
                let record = live.record.clone();
                drop(live);
                self.stop_holder(&sandbox, &record).await;
            }
        }
    }

    /// The last measured usage and the quota (bytes); `None` before the first measurement or
    /// without a quota.
    pub async fn disk(&self, sandbox: &str) -> Option<(u64, u64)> {
        let limit = self.disk_max()?;
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        state.usage.as_ref().map(|(usage, _)| (usage.bytes, limit))
    }

    /// The largest consumers of the last measurement, largest first (SPEC-059 DSK-202).
    pub async fn largest(&self, sandbox: &str) -> Vec<(String, u64)> {
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        state
            .usage
            .as_ref()
            .map(|(usage, _)| usage.largest.clone())
            .unwrap_or_default()
    }

    /// Measures a sandbox now and keeps the reading (after space was freed).
    pub async fn measure_disk(&self, request: &SandboxRequest) -> Option<(u64, u64)> {
        let limit = self.disk_max()?;
        let (dir, sysroot) = (request.dir.clone(), self.sysroot(&request.id));
        let production = self.sysroot(&production_holder(&request.id));
        let usage =
            tokio::task::spawn_blocking(move || measure_sandbox(&dir, &sysroot, &production))
                .await
                .ok()?;
        let bytes = usage.bytes;
        let entry = self.entry(&request.id);
        entry.lock().await.usage = Some((usage, Instant::now()));
        Some((bytes, limit))
    }

    /// Seeds a sandbox's measured usage (tests).
    pub async fn set_usage(&self, sandbox: &str, bytes: u64) {
        self.set_usage_aged(sandbox, bytes, Duration::ZERO).await;
    }

    /// Seeds a measurement taken `age` ago (tests).
    pub async fn set_usage_aged(&self, sandbox: &str, bytes: u64, age: Duration) {
        let entry = self.entry(sandbox);
        entry.lock().await.usage = Some((
            Usage {
                bytes,
                ..Usage::default()
            },
            Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
        ));
    }
}

/// Whether `sandbox` is the throwaway sandbox of a Docker smoke test that still runs, or its
/// production holder: `docker-smoke-<pid>[.prod]` (the leftovers scan's name rule) while process
/// `<pid>` lives. A server that starts beside the smoke must not take them (SPEC-477 SMK-001,
/// ADR-0308, #1160): the smoke starts its holder with Docker off and reads the refusal, and an
/// adopting server replaced that refusing front with its own engine front. Once the process is
/// gone they are holders like any other.
fn smoke_runs(sandbox: &str) -> bool {
    yulius_domain::leftovers::smoke_pid(sandbox).is_some_and(crate::holder::exists)
}

/// Whether [`Manager::tick`] stops a live holder: never while a run holds a lease; a retired
/// sandbox's once it was retired for [`RETIRE_GRACE`], whatever else still runs in it (SPEC-206
/// RMV-004); any other once nothing runs in it and it was idle for `stop_after` (a minute when
/// its base was superseded).
fn holder_stops(
    retired_for: Option<Duration>,
    leases: usize,
    runs: usize,
    idle_for: Duration,
    stop_after: Duration,
    superseded: bool,
) -> bool {
    leases == 0
        && (retired_for.is_some_and(|retired| retired >= RETIRE_GRACE)
            || (runs == 0
                && (idle_for >= stop_after || (superseded && idle_for >= Duration::from_secs(60)))))
}

/// A sandbox's usage: its directory, split by consumer, its system layer (`sysroot/upper`), its
/// Docker engine's storage (`sysroot/docker`, SPEC-187 DKR-006) and its production holder's
/// system layer and engine storage (`production`, SPEC-279 PDE-006) as one each.
fn measure_sandbox(dir: &Path, sysroot: &Path, production: &Path) -> Usage {
    crate::usage::measure(&[
        (dir, None),
        (&sysroot.join("upper"), Some("system")),
        (&sysroot.join("docker"), Some("containers")),
        (production, Some("production")),
    ])
}

#[cfg(test)]
mod tests {
    use super::docker::*;
    use super::*;

    /// SPEC-203 HST-016: a base without the host's `dockerd` is checked at every tick, not every
    /// half hour.
    #[test]
    fn a_base_without_the_hosts_dockerd_checks_the_host_at_every_tick() {
        let soon = Some(Duration::from_secs(30));
        assert!(!fingerprint_due(soon, true, false));
        assert!(fingerprint_due(soon, true, true));
        assert!(fingerprint_due(soon, false, false));
        assert!(fingerprint_due(None, true, false));
        assert!(fingerprint_due(Some(FINGERPRINT_EVERY), true, false));
    }

    #[test]
    fn sizes_read_like_a_person_writes_them() {
        assert_eq!(human(100_000_000_000), "100.0 GB");
        assert_eq!(human(1_234_567_890), "1.2 GB");
    }

    #[tokio::test]
    async fn without_a_base_runs_keep_the_previous_containment()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let map = IdMap {
            host: 1000,
            sub_start: 100_000,
            count: 65_535,
        };
        let manager = Manager::new(IsolationConfig {
            data_dir: data.path().to_path_buf(),
            exe: PathBuf::from("/usr/local/bin/yulius"),
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            newuidmap: PathBuf::from("/usr/bin/newuidmap"),
            newgidmap: PathBuf::from("/usr/bin/newgidmap"),
            slirp4netns: None,
            uid: map,
            gid: map,
            delegation: None,
            limits: Limits::default(),
            disk_max: Some(1),
            forwards: BTreeMap::new(),
            sudo_shim: String::new(),
            idle: IdleConfig {
                stop_after: Duration::from_secs(300),
            },
            auto_build: false,
            docker: Err(DOCKER_OFF.into()),
            health: Arc::default(),
        });
        let request = SandboxRequest {
            id: "s1".into(),
            hostname: "s1".into(),
            dir: data.path().join("sandboxes/s1"),
            port_first: 20_000,
            port_last: 20_019,
        };
        assert_eq!(manager.prepare(&request).await, Ok(None));
        let status = manager.status().await;
        assert!(status.base.is_none() && !status.limits && !status.egress);
        Ok(())
    }

    fn manager(data: &Path, disk_max: u64) -> Arc<Manager> {
        Manager::new(config(data, disk_max))
    }

    fn config(data: &Path, disk_max: u64) -> IsolationConfig {
        let map = IdMap {
            host: 1000,
            sub_start: 100_000,
            count: 65_535,
        };
        IsolationConfig {
            data_dir: data.to_path_buf(),
            exe: PathBuf::from("/usr/local/bin/yulius"),
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            newuidmap: PathBuf::from("/usr/bin/newuidmap"),
            newgidmap: PathBuf::from("/usr/bin/newgidmap"),
            slirp4netns: None,
            uid: map,
            gid: map,
            delegation: None,
            limits: Limits::default(),
            disk_max: Some(disk_max),
            forwards: BTreeMap::new(),
            sudo_shim: String::new(),
            idle: IdleConfig {
                stop_after: Duration::from_secs(300),
            },
            auto_build: false,
            docker: Err(DOCKER_OFF.into()),
            health: Arc::default(),
        }
    }

    /// A delegated unit in `dir` (fake cgroup files).
    fn delegation(dir: &Path) -> Result<Delegation, Box<dyn std::error::Error>> {
        let unit = dir.join("unit");
        std::fs::create_dir_all(&unit)?;
        std::fs::write(unit.join("cgroup.procs"), "")?;
        std::fs::write(unit.join("cgroup.controllers"), "memory pids\n")?;
        Ok(crate::cgroup::prepare(&unit)?)
    }

    fn live_record() -> HolderRecord {
        HolderRecord {
            version: HOLDER_RECORD_VERSION,
            sandbox: "s1".into(),
            base_id: "base-1".into(),
            inner_pid: 4242,
            bwrap_pid: 4241,
            slirp_pid: None,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            boot_id: "b".into(),
            started_at_ms: 0,
            system_root: "/r".into(),
            engine: false,
        }
    }

    /// A live holder whose front serves an engine.
    fn engine_live(record: HolderRecord) -> Live {
        let mut live = Live::new(record, Vec::new());
        live.front = Some(DockerFrontRecord {
            pid: 0,
            mode: "engine".into(),
            reason: None,
            started_at_ms: 0,
        });
        live
    }

    /// SPEC-494 DFJ-005/006 (review round 2): the upkeep reads the state file of the front it
    /// knows and reports it full as the sandbox's Docker trouble; another front's file is not
    /// read.
    #[tokio::test]
    async fn the_upkeep_reports_its_fronts_state_as_the_sandboxs_docker_trouble()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        use yulius_domain::connection_health::Limit;
        let data = tempfile::tempdir()?;
        let config = config(data.path(), 1);
        let health = Arc::clone(&config.health);
        let manager = Manager::new(config);
        let paths = StatePaths::new(manager.state_dir("s1"));
        std::fs::create_dir_all(&paths.dir)?;
        let full = |pid| DockerFrontState {
            pid,
            held: 256,
            max: 256,
            gave_way: 0,
            refused: 2,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: Some(1),
            engine_running: true,
            engine_busy: false,
        };
        let mut live = Live::new(live_record(), Vec::new());
        // This server started it: its refusals are all new.
        live.front_watch.started(77);
        live.front = Some(DockerFrontRecord {
            pid: 77,
            mode: "engine".into(),
            reason: None,
            started_at_ms: 0,
        });
        let mut state = SandboxState {
            live: Some(live),
            ..SandboxState::default()
        };
        crate::holder::write_json(&paths.docker_state(), &full(78))?;
        manager.watch_front("s1", &mut state);
        assert!(
            health.troubles().is_empty(),
            "another front's state was read"
        );
        crate::holder::write_json(&paths.docker_state(), &full(77))?;
        manager.watch_front("s1", &mut state);
        let open = health.troubles();
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!(
            (open[0].limit, open[0].sandbox.as_deref(), open[0].refused),
            (Limit::Docker, Some("s1"), 2)
        );
        // A holder dropped, by any path, leaves no Docker trouble of its sandbox (its boot id
        // is not this host's: nothing is signalled).
        manager.stop_holder("s1", &live_record()).await;
        assert!(health.troubles().is_empty(), "{:?}", health.troubles());
        Ok(())
    }

    /// Stands in for `yulius sandbox-host docker`: serves the spec's socket, answering every
    /// connection with its mode (and reason), until killed. `bind-docker` does nothing.
    const FAKE_FRONT: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys
if sys.argv[2] != "docker":
    sys.exit(0)
spec = json.load(open(sys.argv[4]))
path = os.path.join(spec["state_dir"], "docker", "docker.sock")
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(16)
print("ready", flush=True)
serve = spec["serve"]
answer = ("mode:" + serve["mode"] + ":" + serve.get("reason", "")).encode()
while True:
    c, _ = s.accept()
    c.sendall(answer)
    c.close()
"#;

    /// Writes the executable `path` (mode 755) with `content`, as a server's update does: a
    /// staged file moved into place. The file is written by a child process, never by this one:
    /// other tests fork all the time, a fork made while this process holds the file open for
    /// writing keeps it open until its exec, and starting the file then fails with "Text file
    /// busy" (#1339: 5 in 120 loaded runs of these tests).
    fn put_executable(path: &Path, content: &str) -> std::io::Result<()> {
        use std::io::Write as _;
        let staged = path.with_extension("staged");
        let mut writer = std::process::Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&staged)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        // The writer is reaped whatever the write did; its error is told after.
        let written = writer
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(content.as_bytes()));
        let status = writer.wait()?;
        written?;
        if !status.success() {
            return Err(std::io::Error::other(
                "the staged executable was not written",
            ));
        }
        std::fs::rename(&staged, path)
    }

    /// A manager with Docker possible (or not, `docker`) whose executable is [`FAKE_FRONT`],
    /// and an adopted "holder" that is this test process (its namespaces are alive). Never
    /// stop that holder: it is this process.
    fn adopting(
        data: &Path,
        docker: Result<(), String>,
    ) -> Result<Arc<Manager>, Box<dyn std::error::Error>> {
        adopting_as(data, "s1", docker)
    }

    /// [`adopting`] for the holder of `sandbox`.
    fn adopting_as(
        data: &Path,
        sandbox: &str,
        docker: Result<(), String>,
    ) -> Result<Arc<Manager>, Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt as _;
        let exe = data.join("fake-yulius");
        if !exe.exists() {
            put_executable(&exe, FAKE_FRONT)?;
        }
        let mut config = config(data, 1 << 40);
        config.exe = exe.clone();
        config.uid.host = std::fs::metadata(&exe)?.uid().wrapping_add(1);
        config.docker = docker;
        config.delegation = Some(delegation(data)?);
        let manager = Manager::new(config);
        let state = StatePaths::new(manager.state_dir(sandbox));
        std::fs::create_dir_all(&state.dir)?;
        let dockerd = manager.bases.path("base-1").join("usr/bin");
        std::fs::create_dir_all(&dockerd)?;
        std::fs::write(dockerd.join("dockerd"), "")?;
        let me = std::process::id();
        let ns = |kind| crate::holder::ns_inode(me, kind).ok_or("no namespace");
        let record = HolderRecord {
            sandbox: sandbox.to_owned(),
            inner_pid: me,
            bwrap_pid: 0,
            user_ns: ns("user")?,
            net_ns: ns("net")?,
            mnt_ns: ns("mnt")?,
            boot_id: crate::holder::boot_id(),
            ..live_record()
        };
        write_json(&state.record(), &record)?;
        Ok(manager)
    }

    async fn ask_front(manager: &Manager) -> std::io::Result<String> {
        ask_front_of(manager, "s1").await
    }

    /// What the front of `sandbox` answers at its socket.
    async fn ask_front_of(manager: &Manager, sandbox: &str) -> std::io::Result<String> {
        use tokio::io::AsyncReadExt as _;
        let socket = StatePaths::new(manager.state_dir(sandbox)).docker_socket();
        let mut stream = tokio::net::UnixStream::connect(socket).await?;
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut text))
            .await
            .map_err(|_| std::io::Error::other("no answer"))??;
        Ok(text)
    }

    async fn front_of(manager: &Manager) -> Option<DockerFrontRecord> {
        let entry = manager.entry("s1");
        let state = entry.lock().await;
        state.live.as_ref().and_then(|live| live.front.clone())
    }

    fn kill(pid: u32) {
        crate::holder::signal(pid, nix::sys::signal::Signal::SIGKILL);
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::holder::exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// SPEC-201 DBX-006 (#614): a restart of `yulius.service` prunes an idle engine's empty
    /// leaf while the holder and its front keep running. The next upkeep tick brings it back,
    /// even for a front from an older binary, which no update replaces.
    #[tokio::test]
    async fn a_kept_engine_front_gets_its_pruned_leaf_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let front = front_of(&manager).await.ok_or("no front")?;
        assert_eq!(front.mode, "engine");
        let groups = manager.group_paths("s1").ok_or("no delegation")?;
        let leaf = groups.engine_root;
        assert!(leaf.is_dir());
        // SPEC-483 C4: the front starts the engine in the root below the engine's group.
        let served = served_engine_group(&StatePaths::new(manager.state_dir("s1")));
        assert_eq!(served.as_deref(), Some(leaf.to_string_lossy().as_ref()));
        // A restart of the service prunes both empty groups.
        std::fs::remove_dir(&leaf)?;
        std::fs::remove_dir(&groups.engine)?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.keep_front("s1", &mut guard).await;
        }
        assert_eq!(
            front_of(&manager).await.map(|f| f.pid),
            Some(front.pid),
            "the running front is kept"
        );
        assert!(leaf.is_dir(), "the engine's root and group are back");
        kill(front.pid);
        Ok(())
    }

    /// SPEC-494 AC-6 (DFJ-008): a front that stayed full is restarted by the upkeep once no CI
    /// job of the sandbox runs (one running holds it), and the restart is counted.
    #[tokio::test]
    async fn a_front_full_for_long_is_restarted_when_no_ci_job_runs()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        let long_ago = Instant::now()
            .checked_sub(crate::front_repair::REPAIR_AFTER * 2)
            .ok_or("the clock is too young")?;
        let delegation = manager.config.delegation.clone().ok_or("no delegation")?;
        let _ = delegation.set_ci_jobs("s1", 1, 1);
        let jammed = crate::front_repair::Reading {
            jammed: true,
            ..crate::front_repair::Reading::default()
        };
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            if let Some(live) = guard.live.as_mut() {
                live.repair.observe(true, None, long_ago);
            }
            manager.repair_front("s1", &mut guard, Some(jammed)).await;
        }
        assert_eq!(
            front_of(&manager).await.map(|f| f.pid),
            Some(first.pid),
            "a CI job runs"
        );
        assert_eq!(manager.docker_restarts(), 0);
        let _ = delegation.set_ci_jobs("s1", 0, 0);
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.repair_front("s1", &mut guard, Some(jammed)).await;
        }
        let second = front_of(&manager)
            .await
            .ok_or("no front after the repair")?;
        // The next tick starts over: the new front is not restarted for the old one's jam.
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.repair_front("s1", &mut guard, Some(jammed)).await;
        }
        let kept = front_of(&manager).await.map(|f| f.pid);
        kill(second.pid);
        assert_ne!(second.pid, first.pid, "restarted");
        assert_eq!(kept, Some(second.pid), "the repair starts over");
        assert_eq!(manager.docker_restarts(), 1);
        Ok(())
    }

    /// Slice 3 review round 2: the upkeep never pings a front with no client, an engine starting
    /// or stopping, or no engine (a ping would keep it from idling or start it); beside a client
    /// of a settled engine it does, and three unanswered pings in a row restart the front.
    #[tokio::test]
    async fn the_upkeep_pings_only_beside_a_client_and_restarts_an_unanswered_engine()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::front_repair::Reading;
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        // The front's socket, answered by the test: every ping is seen, and refused (503).
        let socket = StatePaths::new(manager.state_dir("s1")).docker_socket();
        std::fs::remove_file(&socket)?;
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let pings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&pings);
        let server = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            while let Ok((mut stream, _)) = listener.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = stream
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n")
                    .await;
            }
        });
        let tick = |reading: Reading| {
            let manager = Arc::clone(&manager);
            async move {
                let entry = manager.entry("s1");
                let mut guard = entry.lock().await;
                manager.repair_front("s1", &mut guard, Some(reading)).await;
            }
        };
        let settled = Reading {
            jammed: false,
            engine_running: true,
            engine_busy: false,
            held: 1,
        };
        for quiet in [
            Reading { held: 0, ..settled },
            Reading {
                engine_busy: true,
                ..settled
            },
            Reading {
                engine_running: false,
                ..settled
            },
        ] {
            for _ in 0..3 {
                tick(quiet).await;
            }
        }
        assert_eq!(pings.load(Ordering::SeqCst), 0, "pinged without a client");
        assert_eq!(front_of(&manager).await.map(|f| f.pid), Some(first.pid));
        for _ in 0..3 {
            tick(settled).await;
        }
        let second = front_of(&manager)
            .await
            .ok_or("no front after the restart")?;
        server.abort();
        kill(second.pid);
        assert_eq!(pings.load(Ordering::SeqCst), 3);
        assert_ne!(second.pid, first.pid, "an unanswered engine is restarted");
        assert_eq!(manager.docker_restarts(), 1);
        Ok(())
    }

    /// Slice 3 review round 2: a tick without a reading of the front (a successor that wrote
    /// none yet) starts the count over, so the successor is not restarted for an old jam.
    #[tokio::test]
    async fn a_successor_front_is_not_restarted_for_its_predecessors_jam()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        let long_ago = Instant::now()
            .checked_sub(crate::front_repair::REPAIR_AFTER * 2)
            .ok_or("the clock is too young")?;
        let jammed = crate::front_repair::Reading {
            jammed: true,
            ..crate::front_repair::Reading::default()
        };
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            if let Some(live) = guard.live.as_mut() {
                live.repair.observe(true, None, long_ago);
            }
            manager.repair_front("s1", &mut guard, None).await;
            manager.repair_front("s1", &mut guard, Some(jammed)).await;
        }
        let kept = front_of(&manager).await.map(|f| f.pid);
        kill(first.pid);
        assert_eq!(kept, Some(first.pid));
        assert_eq!(manager.docker_restarts(), 0);
        Ok(())
    }

    /// Slice 3 review round 1: a front that does not start again leaves the trouble open (no
    /// "no longer full" while Docker is down), counts no restart, and the asked restart says why.
    #[tokio::test]
    async fn a_front_that_does_not_start_again_keeps_the_trouble_open()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        let paths = StatePaths::new(manager.state_dir("s1"));
        let full = DockerFrontState {
            pid: first.pid,
            held: 256,
            max: 256,
            gave_way: 0,
            refused: 0,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: None,
            engine_running: true,
            engine_busy: false,
        };
        write_json(&paths.docker_state(), &full)?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.watch_front("s1", &mut guard);
        }
        // The next front cannot start: its executable no longer says ready.
        put_executable(&data.path().join("fake-yulius"), "#!/bin/sh\necho broken\n")?;
        let asked = manager.restart_docker("s1").await;
        assert!(asked.is_err_and(|why| why.contains("did not start again")));
        assert_eq!(manager.config.health.troubles().len(), 1, "still open");
        assert_eq!(manager.docker_restarts(), 0);
        // Review round 3: the executable works again, and a person asks before the next tick:
        // one stop and one new front are one restart, counted once.
        put_executable(&data.path().join("fake-yulius"), FAKE_FRONT)?;
        manager.restart_docker("s1").await?;
        assert_eq!(manager.docker_restarts(), 1, "counted once");
        assert_eq!(manager.take_docker_events().len(), 1);
        // Or the next tick starts it: the pending restart is counted then.
        put_executable(&data.path().join("fake-yulius"), "#!/bin/sh\necho broken\n")?;
        assert!(manager.restart_docker("s1").await.is_err());
        put_executable(&data.path().join("fake-yulius"), FAKE_FRONT)?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.keep_front("s1", &mut guard).await;
        }
        let last = front_of(&manager).await.ok_or("no front after the tick")?;
        kill(last.pid);
        assert_eq!(manager.docker_restarts(), 2, "the late start is counted");
        // And the instance's measure counts it too (review round 1 of slices 5 to 7).
        let events = manager.take_docker_events();
        assert_eq!(
            events
                .iter()
                .map(|e| (e.kind, e.count, e.detail.as_str()))
                .collect::<Vec<_>>(),
            vec![("restarted", 1, crate::front_repair::Why::Asked.says())],
            "{events:?}"
        );
        Ok(())
    }

    /// Slice 3 review round 4: a restart asked while no front runs stopped nothing: a front
    /// started at a later tick is no restart and is not counted.
    #[tokio::test]
    async fn an_asked_restart_that_stopped_nothing_is_not_counted_later()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        // No front can start from the beginning.
        put_executable(&data.path().join("fake-yulius"), "#!/bin/sh\necho broken\n")?;
        assert_eq!(manager.adopt().await, 1);
        assert!(front_of(&manager).await.is_none());
        assert!(manager.restart_docker("s1").await.is_err());
        put_executable(&data.path().join("fake-yulius"), FAKE_FRONT)?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.keep_front("s1", &mut guard).await;
        }
        let front = front_of(&manager).await.ok_or("no front after the tick")?;
        assert_eq!(
            manager.docker_restarts(),
            0,
            "a late start of nothing stopped"
        );
        // Asked with no front to stop, a start that works is no restart either.
        kill(front.pid);
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            if let Some(live) = guard.live.as_mut() {
                live.front = None;
            }
        }
        manager.restart_docker("s1").await?;
        let started = front_of(&manager).await.ok_or("no front after the ask")?;
        kill(started.pid);
        assert_eq!(manager.docker_restarts(), 0);
        Ok(())
    }

    /// Slice 3 review round 1: a production holder's front is journaled, never restarted by the
    /// upkeep (a production CD run is its one user).
    #[tokio::test]
    async fn a_production_front_is_never_restarted_by_the_upkeep()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let name = production_holder("s1");
        let manager = adopting_as(data.path(), &name, Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let long_ago = Instant::now()
            .checked_sub(crate::front_repair::REPAIR_AFTER * 2)
            .ok_or("the clock is too young")?;
        let jammed = crate::front_repair::Reading {
            jammed: true,
            ..crate::front_repair::Reading::default()
        };
        let entry = manager.entry(&name);
        let mut guard = entry.lock().await;
        let before = guard.live.as_ref().and_then(|live| live.front.clone());
        if let Some(live) = guard.live.as_mut() {
            live.repair.observe(true, None, long_ago);
        }
        manager.repair_front(&name, &mut guard, Some(jammed)).await;
        let after = guard.live.as_ref().and_then(|live| live.front.clone());
        if let Some(front) = &after {
            kill(front.pid);
        }
        assert_eq!(before.map(|f| f.pid), after.map(|f| f.pid));
        assert_eq!(manager.docker_restarts(), 0);
        Ok(())
    }

    /// SPEC-494 DFJ-009: a restart asked for (the Superagent's tool) runs at once and ends the
    /// sandbox's Docker trouble once the new front's state is read.
    #[tokio::test]
    async fn an_asked_restart_starts_a_new_front_and_ends_the_trouble()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        let paths = StatePaths::new(manager.state_dir("s1"));
        let state = |pid, held| DockerFrontState {
            pid,
            held,
            max: 256,
            gave_way: 0,
            refused: 0,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: None,
            engine_running: true,
            engine_busy: false,
        };
        write_json(&paths.docker_state(), &state(first.pid, 256))?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.watch_front("s1", &mut guard);
        }
        assert_eq!(manager.config.health.troubles().len(), 1);
        manager.restart_docker("s1").await?;
        let second = front_of(&manager)
            .await
            .ok_or("no front after the restart")?;
        // The instance counts the restart (SPEC-494 DFJ-013), once.
        let events = manager.take_docker_events();
        assert_eq!(
            events.iter().map(|e| (e.kind, e.count)).collect::<Vec<_>>(),
            vec![("restarted", 1)],
            "{events:?}"
        );
        assert!(manager.take_docker_events().is_empty(), "taken once");
        write_json(&paths.docker_state(), &state(second.pid, 0))?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.watch_front("s1", &mut guard);
        }
        kill(second.pid);
        assert_ne!(second.pid, first.pid);
        assert!(
            manager.config.health.troubles().is_empty(),
            "{:?}",
            manager.config.health.troubles()
        );
        assert_eq!(manager.docker_restarts(), 1);
        Ok(())
    }

    /// SPEC-494 AC-8 (DFJ-010): a front kept from another build (the fake front's file is not
    /// the server's executable) is replaced at a tick once it is quiet, and kept while it holds
    /// a connection.
    #[tokio::test]
    async fn an_outdated_front_is_replaced_once_it_is_quiet()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let first = front_of(&manager).await.ok_or("no front")?;
        let busy = DockerFrontState {
            pid: first.pid,
            held: 2,
            max: 256,
            gave_way: 0,
            refused: 0,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: None,
            engine_running: false,
            engine_busy: false,
        };
        let paths = StatePaths::new(manager.state_dir("s1"));
        write_json(&paths.docker_state(), &busy)?;
        let replace = || async {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.replace_outdated_front("s1", &mut guard).await;
        };
        replace().await;
        assert_eq!(
            front_of(&manager).await.map(|f| f.pid),
            Some(first.pid),
            "busy: kept"
        );
        // Review round 1 of slices 5 to 7: the state says held 0, but a client connected since
        // it was written (the front, stopped, has not accepted it): kept.
        write_json(&paths.docker_state(), &DockerFrontState { held: 0, ..busy })?;
        crate::holder::signal(first.pid, nix::sys::signal::Signal::SIGSTOP);
        let waiting = std::os::unix::net::UnixStream::connect(paths.docker_socket());
        replace().await;
        let kept = front_of(&manager).await.map(|f| f.pid);
        crate::holder::signal(first.pid, nix::sys::signal::Signal::SIGCONT);
        let mut waiting = waiting?;
        {
            use std::io::Read as _;
            let mut answer = String::new();
            waiting.read_to_string(&mut answer)?;
            assert!(answer.starts_with("mode:engine"), "{answer}");
        }
        assert_eq!(kept, Some(first.pid), "a waiting client: kept");
        assert!(manager.take_docker_events().is_empty());
        // A front from before ADR-0344 writes no state: quiet when it runs no engine and holds
        // no connection, however many sockets it holds besides.
        std::fs::remove_file(paths.docker_state())?;
        replace().await;
        let second = front_of(&manager)
            .await
            .ok_or("no front after the replacement")?;
        kill(second.pid);
        assert_ne!(second.pid, first.pid, "quiet: replaced");
        assert_eq!(manager.docker_restarts(), 1);
        let events = manager.take_docker_events();
        assert_eq!(
            events
                .iter()
                .map(|e| (e.kind, e.count, e.detail.as_str()))
                .collect::<Vec<_>>(),
            vec![("restarted", 1, crate::front_repair::Why::Outdated.says())],
            "{events:?}"
        );
        Ok(())
    }

    /// SPEC-494 slice 2 review round 2: a front this server starts counts its refusals as new
    /// (one adopted after a restart would take them as its baseline).
    #[tokio::test]
    async fn a_front_this_server_starts_reports_its_refusals_as_new()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let front = front_of(&manager).await.ok_or("no front")?;
        let state = DockerFrontState {
            pid: front.pid,
            held: 10,
            max: 256,
            gave_way: 0,
            refused: 2,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: None,
            engine_running: true,
            engine_busy: false,
        };
        write_json(
            &StatePaths::new(manager.state_dir("s1")).docker_state(),
            &state,
        )?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.watch_front("s1", &mut guard);
        }
        let open = manager.config.health.troubles();
        kill(front.pid);
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!(open[0].refused, 2);
        Ok(())
    }

    /// SPEC-494 slice 2 review round 2: a production holder's full front is journaled, not a
    /// trouble under a name no sandbox has.
    #[test]
    fn a_production_fronts_state_is_no_panel_trouble() -> Result<(), Box<dyn std::error::Error>> {
        use yulius_contracts::sandbox::DockerFrontState;
        let data = tempfile::tempdir()?;
        let config = config(data.path(), 1);
        let health = Arc::clone(&config.health);
        let manager = Manager::new(config);
        let name = production_holder("s1");
        let paths = StatePaths::new(manager.state_dir(&name));
        std::fs::create_dir_all(&paths.dir)?;
        let mut live = Live::new(live_record(), Vec::new());
        live.front_watch.started(77);
        live.front = Some(DockerFrontRecord {
            pid: 77,
            mode: "engine".into(),
            reason: None,
            started_at_ms: 0,
        });
        let mut state = SandboxState {
            live: Some(live),
            ..SandboxState::default()
        };
        let full = DockerFrontState {
            pid: 77,
            held: 256,
            max: 256,
            gave_way: 0,
            refused: 2,
            hung_up: 0,
            closed_after_grace: 0,
            at_limit_since_ms: Some(1),
            engine_running: true,
            engine_busy: false,
        };
        crate::holder::write_json(&paths.docker_state(), &full)?;
        manager.watch_front(&name, &mut state);
        assert!(health.troubles().is_empty(), "{:?}", health.troubles());
        Ok(())
    }

    /// SPEC-196 DKL-003 (#597), the upgrade: a holder kept from before engines (adopted after
    /// a restart, its runs untouched) gains a front serving the engine; a dead front is started
    /// again; a server with Docker off replaces it with a refusal, and one with Docker on again
    /// brings the engine back, each time at the same socket path.
    #[tokio::test]
    async fn an_adopted_holder_from_before_engines_gains_an_engine_front()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        assert_eq!(manager.adopt().await, 1);
        let front = front_of(&manager).await.ok_or("no front")?;
        assert_eq!(front.mode, "engine");
        assert_eq!(ask_front(&manager).await?, "mode:engine:");
        // Review round 1 (high): a holder from before #583 has no engine storage; its engine's
        // bwrap would refuse the missing bind source.
        assert!(
            data.path().join("sysroots/s1/docker").is_dir(),
            "the engine's storage exists"
        );
        let state = StatePaths::new(manager.state_dir("s1"));
        let spec: DockerFrontSpec = read_json(&state.docker_spec())?;
        assert_eq!(spec.holder_pid, std::process::id());
        assert!(
            spec.cgroup.is_some_and(|c| c.contains("sandboxes/s1")),
            "the sandbox's group"
        );
        assert!(matches!(spec.serve, DockerServe::Engine { .. }));

        kill(front.pid);
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.keep_front("s1", &mut guard).await;
        }
        let again = front_of(&manager).await.ok_or("not restarted")?;
        assert_ne!(again.pid, front.pid);
        assert_eq!(ask_front(&manager).await?, "mode:engine:");

        // Another server process with Docker off adopts the holder and its running front.
        let off = adopting(data.path(), Err(DOCKER_OFF.into()))?;
        assert_eq!(off.adopt().await, 1);
        let refusing = front_of(&off).await.ok_or("no front")?;
        assert_eq!(refusing.mode, "refuse");
        // Review round 3 (low): the reason reaches whoever asks for the front (the smoke).
        assert_eq!(
            off.docker_front("s1")
                .await
                .and_then(|f| f.reason)
                .as_deref(),
            Some(DOCKER_OFF)
        );
        assert!(
            !crate::holder::exists(again.pid),
            "the engine front was stopped"
        );
        assert_eq!(ask_front(&off).await?, format!("mode:refuse:{DOCKER_OFF}"));
        // And the next with Docker on: the engine again, the refusal gone.
        let on = adopting(data.path(), Ok(()))?;
        assert_eq!(on.adopt().await, 1);
        let back = front_of(&on).await.ok_or("no front")?;
        assert_eq!(back.mode, "engine");
        assert_eq!(ask_front(&on).await?, "mode:engine:");
        // An unchanged front is kept across server restarts.
        let same = adopting(data.path(), Ok(()))?;
        assert_eq!(same.adopt().await, 1);
        assert_eq!(front_of(&same).await.map(|f| f.pid), Some(back.pid));
        kill(back.pid);
        Ok(())
    }

    /// #1160 (SPEC-477 SMK-001): a server that starts while a Docker smoke test runs leaves the
    /// smoke's sandbox to it (the first `yulius host setup` starts `yulius.service` seconds
    /// before its smoke). Adopted, the smoke's Docker-off holder got this server's engine front
    /// in place of the refusal the smoke must read, and the server kept both holders in its
    /// upkeep. A smoke whose process is gone left ordinary holders: adopted as before.
    #[tokio::test]
    async fn a_starting_server_leaves_a_running_smoke_tests_holders_alone()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        // The smoke is this process: its Docker-off holder with the refusing front, and its
        // production holder.
        let smoke = format!("docker-smoke-{}", std::process::id());
        let production = production_holder(&smoke);
        let own = adopting_as(data.path(), &smoke, Err(DOCKER_OFF.into()))?;
        adopting_as(data.path(), &production, Ok(()))?;
        // The server beside it has Docker on and a sandbox of its own.
        let server = adopting(data.path(), Ok(()))?;
        // From here fronts run: nothing returns or asserts until they are ended below.
        let own_adopted = own.adopt_one(&smoke).await;
        let refusing = own.docker_front(&smoke).await;

        // The server starts.
        let adopted = server.adopt().await;
        let mut known: Vec<String> = lock(&server.sandboxes).keys().cloned().collect();
        known.sort();
        let answer = ask_front_of(&own, &smoke).await;
        let refusing_runs = refusing
            .as_ref()
            .is_some_and(|front| crate::holder::exists(front.pid));
        let production_front = StatePaths::new(server.state_dir(&production))
            .docker_record()
            .exists();
        // Also the fronts a server without the rule started.
        for sandbox in ["s1", smoke.as_str(), production.as_str()] {
            let state = StatePaths::new(server.state_dir(sandbox));
            if let Ok(front) = read_json::<DockerFrontRecord>(&state.docker_record()) {
                kill(front.pid);
            }
        }
        if let Some(front) = &refusing {
            kill(front.pid);
        }
        assert!(own_adopted, "the smoke adopts its own holder");
        assert_eq!(refusing.map(|f| f.mode).as_deref(), Some("refuse"));
        assert_eq!(answer?, format!("mode:refuse:{DOCKER_OFF}"));
        assert!(refusing_runs, "the smoke's refusing front was not stopped");
        assert_eq!(
            known,
            ["s1"],
            "only the server's own sandbox is in its upkeep"
        );
        assert_eq!(adopted, 1);
        assert!(!production_front, "the production holder got no front");

        // A smoke that ended (its process is gone) left holders like any other.
        let mut ended = std::process::Command::new("true").spawn()?;
        let pid = ended.id();
        ended.wait()?;
        let left = format!("docker-smoke-{pid}");
        let restarted = adopting_as(data.path(), &left, Ok(()))?;
        std::fs::remove_dir_all(server.state_dir(&smoke))?;
        std::fs::remove_dir_all(server.state_dir(&production))?;
        let adopted = restarted.adopt().await;
        let front = restarted.docker_front(&left).await;
        for sandbox in ["s1", left.as_str()] {
            if let Some(front) = restarted.docker_front(sandbox).await {
                kill(front.pid);
            }
        }
        assert_eq!(adopted, 2, "the server's sandbox and the ended smoke's");
        assert_eq!(front.map(|f| f.mode).as_deref(), Some("engine"));
        Ok(())
    }

    /// SPEC-196 DKL-002/005: a holder gets an engine front, a missing prerequisite a refusal
    /// that names it; and runs see the Docker directory.
    #[tokio::test]
    async fn the_front_mode_follows_the_holder_and_the_prerequisites()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let request = request(data.path());
        let on = adopting(data.path(), Ok(()))?;
        assert!(matches!(
            on.front_mode(&request, &live_record()),
            DockerServe::Engine { .. }
        ));
        // Review round 1 (low): a system without `dockerd` (SPEC-199 DSY-003: the agent can
        // install it in the sandbox, or an operator on the host for new bases).
        let mut older = live_record();
        older.base_id = "base-0".into();
        assert_eq!(
            on.front_mode(&request, &older),
            DockerServe::Refuse {
                reason: DOCKER_NO_DOCKERD.into()
            }
        );
        // SPEC-201 DBX-001/002 (#610): Ubuntu's `docker.io` only suggests buildx and compose, so
        // the in-sandbox and host installs name the plugins too.
        for needle in [
            "sudo apt-get install -y docker.io docker-buildx docker-compose-v2",
            "on the host",
            "docker-buildx-plugin",
            "docker-compose-plugin",
            "newer base",
        ] {
            assert!(DOCKER_NO_DOCKERD.contains(needle), "{needle}");
        }
        let off = adopting(data.path(), Err(DOCKER_OFF.into()))?;
        assert_eq!(
            off.front_mode(&request, &live_record()),
            DockerServe::Refuse {
                reason: DOCKER_OFF.into()
            }
        );
        let mut undelegated = config(data.path(), 1);
        undelegated.docker = Ok(());
        let undelegated = Manager::new(undelegated);
        assert_eq!(
            undelegated.docker_unavailable().as_deref(),
            Some(DOCKER_NO_DELEGATION)
        );
        // Forwards of a holder from before engines listen on loopback only: the front serves
        // them on the bridge for containers. SPEC-187 and newer holders listen everywhere.
        let mut forwarding = config(data.path(), 1);
        forwarding.forwards = BTreeMap::from([(8081, "127.0.0.1:8081".parse()?)]);
        let forwarding = Manager::new(forwarding);
        let spec_file = StatePaths::new(forwarding.state_dir("s1")).spec();
        for (json, expected) in [
            ("{}", vec![8081]),
            (r#"{"containers": false}"#, vec![8081]),
            (r#"{"containers": true}"#, vec![]),
            (r#"{"engine": {"command": []}}"#, vec![]),
        ] {
            std::fs::write(&spec_file, json)?;
            assert_eq!(forwarding.bridge_forwards("s1"), expected, "{json}");
        }
        for reason in [
            DOCKER_OFF,
            DOCKER_NO_DOCKERD,
            DOCKER_NO_DELEGATION,
            DOCKER_WRITABLE_EXE,
            DOCKER_NO_CGROUP,
        ] {
            assert!(reason.contains("an operator"), "{reason}");
            assert!(!reason.contains("  "), "{reason}");
        }
        Ok(())
    }

    /// SPEC-199 DSY-002 (task #185): the engine runs the sandbox's own `dockerd`, so its system
    /// decides: an entry in its upper layer (where `sudo apt-get install` writes) over its base.
    /// A link the sandbox planted there is never followed by the server.
    #[tokio::test]
    async fn a_sandbox_whose_own_system_has_dockerd_gets_an_engine()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let request = request(data.path());
        let manager = adopting(data.path(), Ok(()))?;
        let mode = |base: &str| {
            let mut record = live_record();
            record.base_id = base.into();
            manager.front_mode(&request, &record)
        };
        let engine = |serve: DockerServe| matches!(serve, DockerServe::Engine { .. });
        // base-0 has no `dockerd`; base-1 has one (`adopting`).
        assert!(!engine(mode("base-0")));
        assert!(engine(mode("base-1")));

        let upper = manager.sysroot("s1").join("upper");
        std::fs::create_dir_all(&upper)?;
        let elsewhere = data.path().join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("sbin"))?;
        std::fs::write(elsewhere.join("sbin/dockerd"), "")?;
        std::os::unix::fs::symlink(&elsewhere, upper.join("usr"))?;
        assert!(!engine(mode("base-0")), "a planted link was followed");
        assert!(!engine(mode("base-1")), "a link over usr hides the base's");
        std::fs::remove_file(upper.join("usr"))?;
        // Review round 1 (low): what every sandbox that ran `apt-get` has, directories on the
        // way without `dockerd`, leaves the base's.
        std::fs::create_dir_all(upper.join("usr/bin"))?;
        std::fs::create_dir_all(upper.join("usr/sbin"))?;
        std::fs::create_dir_all(upper.join("usr/local/bin"))?;
        assert!(engine(mode("base-1")));
        assert!(!engine(mode("base-0")));
        // `/usr/local/sbin` comes first on the engine's PATH.
        std::fs::create_dir_all(upper.join("usr/local/sbin"))?;
        std::fs::write(upper.join("usr/local/sbin/dockerd"), "")?;
        assert!(engine(mode("base-0")));
        std::fs::remove_file(upper.join("usr/local/sbin/dockerd"))?;

        // `apt-get install docker.io` in the sandbox: /usr/sbin/dockerd in its upper layer.
        std::fs::create_dir_all(upper.join("usr/sbin"))?;
        std::fs::write(upper.join("usr/sbin/dockerd"), "")?;
        assert!(engine(mode("base-0")));
        std::fs::remove_file(upper.join("usr/sbin/dockerd"))?;
        // A link at `dockerd` itself resolves inside the sandbox: it counts, unread.
        std::os::unix::fs::symlink("/nowhere/dockerd", upper.join("usr/sbin/dockerd"))?;
        assert!(engine(mode("base-0")));
        std::fs::remove_file(upper.join("usr/sbin/dockerd"))?;

        // Anything else over the base's `dockerd` removes it: a directory here, the same branch
        // as a whiteout (a character device, which this test cannot make).
        std::fs::create_dir_all(upper.join("usr/bin/dockerd"))?;
        assert!(!engine(mode("base-1")));
        Ok(())
    }

    /// Review round 2 (low): deciding a front's mode, at every tick, only reads the sandbox's
    /// cgroup paths; preparing the groups (limits, the CPU split) happens once, when an engine
    /// front starts.
    #[tokio::test]
    async fn deciding_a_front_prepares_no_cgroup() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()))?;
        let mut request = request(data.path());
        request.id = "s2".into();
        let serve = manager.front_mode(&request, &live_record());
        assert!(matches!(serve, DockerServe::Engine { .. }), "{serve:?}");
        let unit = manager.config.delegation.as_ref().map(|d| d.unit.clone());
        let group = unit.ok_or("delegated")?.join("sandboxes/s2");
        assert!(!group.exists(), "{} was prepared", group.display());
        Ok(())
    }

    fn front(mode: &str, reason: Option<&str>) -> DockerFrontRecord {
        DockerFrontRecord {
            pid: 42,
            mode: mode.into(),
            reason: reason.map(str::to_owned),
            started_at_ms: 0,
        }
    }

    /// SPEC-196 DKL-003: a front is kept while it serves as wanted, started when none runs,
    /// replaced when the mode or the refusal changed. Review rounds 1 and 2 (low): the wanted
    /// mode needs no cgroup write (`deciding_a_front_prepares_no_cgroup`), so a failed one never
    /// replaces a running engine.
    #[test]
    fn a_front_is_replaced_only_when_its_mode_changed() {
        let engine = || DockerServe::Engine {
            engine: EngineSpec {
                command: vec!["/y".into()],
                cgroup: "/c".into(),
                idle_secs: 300,
            },
        };
        let refuse = |reason: &str| DockerServe::Refuse {
            reason: reason.into(),
        };
        let running = front("engine", None);
        assert_eq!(
            front_action(Some((&running, true)), engine(), false),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(Some((&running, false)), engine(), false),
            FrontAction::Start(engine())
        );
        assert_eq!(
            front_action(None, engine(), false),
            FrontAction::Start(engine())
        );
        assert_eq!(
            front_action(Some((&running, true)), refuse("off"), false),
            FrontAction::Replace(refuse("off"))
        );
        let refusing = front("refuse", Some("off"));
        assert_eq!(
            front_action(Some((&refusing, true)), refuse("off"), false),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(Some((&refusing, true)), refuse("other"), false),
            FrontAction::Replace(refuse("other"))
        );
        assert_eq!(
            front_action(Some((&refusing, true)), engine(), false),
            FrontAction::Replace(engine())
        );
        // SPEC-199 review round 1 (nit): `dockerd` gone from the sandbox's system for a moment
        // (a docker.io → docker-ce swap) never cuts a running engine and its containers.
        assert_eq!(
            front_action(Some((&running, true)), refuse(DOCKER_NO_DOCKERD), false),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(None, refuse(DOCKER_NO_DOCKERD), false),
            FrontAction::Start(refuse(DOCKER_NO_DOCKERD))
        );
        // SPEC-458 LAY-005: the sandbox's groups moved to another unit, where the engine was
        // ended: a front that would start the next engine in the group that is gone is replaced.
        assert_eq!(
            front_action(Some((&running, true)), engine(), true),
            FrontAction::Replace(engine())
        );
        assert_eq!(
            front_action(Some((&running, true)), refuse(DOCKER_NO_DOCKERD), true),
            FrontAction::Keep,
            "only an engine front that stays an engine front is replaced for its group"
        );
        assert_eq!(
            front_action(Some((&refusing, true)), refuse("off"), true),
            FrontAction::Keep
        );
    }

    /// SPEC-458 LAY-005: a front's engine group counts as moved only when the front was given
    /// another group than the sandbox's engine group now and that group is gone. One that is
    /// still there may hold a running engine, which is never cut.
    #[test]
    fn an_engine_group_has_moved_only_when_the_fronts_own_is_gone() {
        let wanted = DockerServe::Engine {
            engine: EngineSpec {
                command: vec!["/y".into()],
                cgroup: "/new/sandboxes/s1/engine".into(),
                idle_secs: 300,
            },
        };
        let gone = |_: &Path| false;
        let there = |_: &Path| true;
        let old = Some("/old/sandboxes/s1/engine");
        assert!(engine_group_moved(old, &wanted, gone));
        assert!(!engine_group_moved(old, &wanted, there));
        assert!(!engine_group_moved(
            Some("/new/sandboxes/s1/engine"),
            &wanted,
            gone
        ));
        assert!(!engine_group_moved(None, &wanted, gone));
        let refuse = DockerServe::Refuse {
            reason: "off".into(),
        };
        assert!(!engine_group_moved(old, &refuse, gone));
    }

    /// The group a front was given is read from the spec it was started with.
    #[test]
    fn the_group_a_front_serves_is_read_from_its_spec() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        assert_eq!(served_engine_group(&paths), None, "no spec");
        let mut spec = DockerFrontSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: "s1".into(),
            state_dir: dir.path().to_string_lossy().into_owned(),
            holder_pid: 7,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: Some("/old/sandboxes/s1/system".into()),
            serve: DockerServe::Engine {
                engine: EngineSpec {
                    command: vec!["/y".into()],
                    cgroup: "/old/sandboxes/s1/engine".into(),
                    idle_secs: 300,
                },
            },
            bridge_forwards: Vec::new(),
        };
        write_json(&paths.docker_spec(), &spec)?;
        assert_eq!(
            served_engine_group(&paths).as_deref(),
            Some("/old/sandboxes/s1/engine")
        );
        spec.serve = DockerServe::Refuse {
            reason: "off".into(),
        };
        write_json(&paths.docker_spec(), &spec)?;
        assert_eq!(served_engine_group(&paths), None, "not an engine front");
        Ok(())
    }

    /// SPEC-196 DKL-004, review round 1 (medium): a run needing the directory is tried at most
    /// [`BIND_TRIES`] times, bound or skipped ones are done, and an ended namespace is forgotten
    /// so a later one reusing its inode number is looked at again.
    #[test]
    fn runs_are_bound_at_most_three_times_and_ended_namespaces_forgotten() {
        use crate::docker_bind::Plan;
        let mut known = HashMap::new();
        let found = [
            (10, 100, Plan::Bind),
            (11, 101, Plan::AlreadyBound),
            (12, 102, Plan::HostSystem),
        ];
        assert_eq!(bind_plan(&mut known, &found), vec![(10, 100)]);
        assert_eq!(known.get(&101), Some(&BIND_DONE));
        assert_eq!(known.get(&102), Some(&BIND_DONE));
        for attempt in 1..=BIND_TRIES {
            bind_record(&mut known, &[(10, 100)], "failed 10 move_mount: EPERM\n");
            assert_eq!(known.get(&100), Some(&attempt));
        }
        assert!(bind_plan(&mut known, &found).is_empty(), "given up");
        // Missing from the helper's output counts as an attempt; bound is done.
        let found = [(20, 200, Plan::Bind), (21, 201, Plan::Bind)];
        let pids = bind_plan(&mut known, &found);
        assert_eq!(pids, vec![(20, 200), (21, 201)]);
        assert!(!known.contains_key(&100), "ended namespaces are forgotten");
        bind_record(&mut known, &pids, "bound 20\n");
        assert_eq!(known.get(&200), Some(&BIND_DONE));
        assert_eq!(known.get(&201), Some(&1));
        assert_eq!(bind_plan(&mut known, &found), vec![(21, 201)]);
        // A new namespace with the number of one given up is tried again once the old ended.
        let mut known = HashMap::from([(300, BIND_TRIES)]);
        assert!(bind_plan(&mut known, &[(30, 300, Plan::Bind)]).is_empty());
        assert!(bind_plan(&mut known, &[]).is_empty());
        assert_eq!(
            bind_plan(&mut known, &[(31, 300, Plan::Bind)]),
            vec![(31, 300)]
        );
    }

    /// SPEC-196 DKL-003 (#597): a holder adopted from before engines, or whose socket failed,
    /// gets a front of its own, so its agents are told Docker works as with a fresh holder. (It
    /// used to serve none for as long as it lived: SPEC-187 asserted that here.)
    #[tokio::test]
    async fn every_holder_gives_its_runs_docker_whatever_its_age()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let off = manager(data.path(), 1);
        assert!(!off.docker_for("s1").await, "the setting is off");
        let mut on = config(data.path(), 1);
        on.docker = Ok(());
        on.delegation = Some(delegation(data.path())?);
        let manager = Manager::new(on);
        assert!(manager.docker(), "the setting with the delegation");
        assert!(manager.docker_for("s1").await, "its next holder serves one");
        let entry = manager.entry("s1");
        entry.lock().await.live = Some(Live::new(live_record(), Vec::new()));
        assert!(
            manager.docker_for("s1").await,
            "a holder from before engines gains one"
        );
        Ok(())
    }

    /// ADR-0315: root in the sandbox owns `/etc` of its upper layer. Its `passwd` is read
    /// without following a link (the server would read a host file into people's
    /// `/etc/passwd`), without waiting on a FIFO, and only up to a megabyte; what hides the
    /// base's file there gives nothing, and a layer without one gives the base's.
    #[test]
    fn a_sandbox_s_passwd_is_read_without_following_a_link()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let (base, upper) = (data.path().join("base"), data.path().join("upper"));
        std::fs::create_dir_all(base.join("etc"))?;
        std::fs::write(base.join("etc/passwd"), "root:x:0:0::/root:/bin/sh\n")?;
        let text = |name: &str| system_file_text(&base, &upper, name);
        assert_eq!(
            text("passwd").as_deref(),
            Some("root:x:0:0::/root:/bin/sh\n")
        );
        assert_eq!(text("group"), None);

        std::fs::create_dir_all(upper.join("etc"))?;
        assert_eq!(
            text("passwd").as_deref(),
            Some("root:x:0:0::/root:/bin/sh\n")
        );
        std::fs::write(upper.join("etc/passwd"), "app:x:999:999::/:/bin/false\n")?;
        assert_eq!(
            text("passwd").as_deref(),
            Some("app:x:999:999::/:/bin/false\n")
        );

        // A link to a file of the host: not read, and the base's is hidden as in the sandbox.
        let host = data.path().join("workspace-token.key");
        std::fs::write(&host, "a secret of the host\n")?;
        std::fs::remove_file(upper.join("etc/passwd"))?;
        std::os::unix::fs::symlink(&host, upper.join("etc/passwd"))?;
        assert_eq!(text("passwd"), None);

        // A FIFO nobody writes, and more than the bound.
        std::fs::remove_file(upper.join("etc/passwd"))?;
        assert!(
            std::process::Command::new("mkfifo")
                .arg(upper.join("etc/passwd"))
                .status()?
                .success()
        );
        let started = std::time::Instant::now();
        assert_eq!(text("passwd"), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::fs::remove_file(upper.join("etc/passwd"))?;
        std::fs::write(upper.join("etc/passwd"), vec![b'a'; (1 << 20) + 1])?;
        assert_eq!(text("passwd"), None);

        // `etc` itself a link out of the layer.
        std::fs::remove_dir_all(upper.join("etc"))?;
        let outside = data.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("passwd"), "outside:x:1:1::/:/bin/sh\n")?;
        std::os::unix::fs::symlink(&outside, upper.join("etc"))?;
        assert_eq!(text("passwd"), None);
        Ok(())
    }

    /// Review of SPEC-187 (low): the engine's read-only binds are not locked (same user
    /// namespace), so an executable the sandbox's root owns, or that its group or anyone may
    /// write, could be remounted and rewritten. Such a host gets no engine.
    #[test]
    fn an_executable_the_sandbox_owns_gets_no_engine() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt as _;
        let data = tempfile::tempdir()?;
        let exe = data.path().join("yulius");
        std::fs::write(&exe, "")?;
        let owner = std::fs::metadata(&exe)?.uid();
        let mut owned = config(data.path(), 1);
        owned.docker = Ok(());
        owned.delegation = Some(delegation(data.path())?);
        owned.exe = exe;
        owned.uid.host = owner;
        let mut other = owned.clone();
        other.uid.host = owner.wrapping_add(1);
        assert!(!Manager::new(owned).docker());
        let set_mode = |mode| {
            std::fs::set_permissions(
                &other.exe,
                std::os::unix::fs::PermissionsExt::from_mode(mode),
            )
        };
        set_mode(0o755)?;
        assert!(Manager::new(other.clone()).docker());
        // Review round 2 (low): writable by the service group (root's group inside) or anyone.
        for mode in [0o775, 0o757] {
            set_mode(mode)?;
            assert!(!Manager::new(other.clone()).docker(), "{mode:o}");
        }
        Ok(())
    }

    /// An API answering every request with `SpaceReclaimed: 500`, counting its connections.
    fn fake_api(
        listener: tokio::net::UnixListener,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.0 200 OK\r\n\r\n{\"SpaceReclaimed\":500}")
                    .await;
            }
        });
        (server, count)
    }

    /// SPEC-504 CLB-004/005/006: a job's end removes its containers from the running engine
    /// through its control socket, the sweep those of every job whose group has no process,
    /// keeping a running job's; each removal is a Docker event. Without a running engine
    /// nothing is asked.
    #[tokio::test]
    async fn a_jobs_end_and_the_sweep_remove_what_ci_jobs_left()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let data = tempfile::tempdir()?;
        let mut config = config(data.path(), 1 << 40);
        config.delegation = Some(delegation(data.path())?);
        let delegation = config.delegation.clone().ok_or("no delegation")?;
        let manager = Manager::new(config);
        manager.ci_job_ended("s1", "j1").await;
        manager.sweep_ci_leftovers("s1").await;
        assert!(
            manager.take_docker_events().is_empty(),
            "no holder: nothing"
        );
        delegation.sandbox_group("s1", &crate::cgroup::Limits::default())?;
        let running = delegation.ci_job_group("s1", "j2")?;
        std::fs::write(running.join("cgroup.events"), "populated 1\n")?;
        let state = StatePaths::new(manager.state_dir("s1"));
        std::fs::create_dir_all(state.engine_control())?;
        let listener = tokio::net::UnixListener::bind(state.engine_control().join("docker.sock"))?;
        let deleted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&deleted);
        let engine = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = vec![0u8; 4096];
                let n = stream.read(&mut head).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&head[..n]).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let (status, body) = if head.starts_with("DELETE ") {
                    if let Ok(mut seen) = seen.lock() {
                        seen.push(path);
                    }
                    (204, String::new())
                } else if path.starts_with("/containers/json") {
                    let all = r#"[{"Id":"c1","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j1"}},
                                  {"Id":"c2","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j2"}}]"#;
                    let one =
                        r#"[{"Id":"c1","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j1"}}]"#;
                    let three =
                        r#"[{"Id":"c3","ImageID":"sha256:i","Labels":{"yulius.ci.job":"j3"}}]"#;
                    let listed = if path.contains("%3Dj1") {
                        one
                    } else if path.contains("%3Dj3") {
                        three
                    } else {
                        all
                    };
                    (200, listed.to_owned())
                } else if path.starts_with("/networks") {
                    (200, "[]".to_owned())
                } else if path.starts_with("/images/") {
                    (200, r#"{"Config":{"Labels":{}}}"#.to_owned())
                } else {
                    (404, "{}".to_owned())
                };
                let _ = stream
                    .write_all(format!("HTTP/1.0 {status} X\r\n\r\n{body}").as_bytes())
                    .await;
            }
        });
        let entry = manager.entry("s1");
        entry.lock().await.live = Some(Live::new(live_record(), Vec::new()));
        manager.ci_job_ended("s1", "j1").await;
        assert!(
            deleted.lock().map_err(|_| "poisoned")?.is_empty(),
            "no engine"
        );
        entry.lock().await.live = Some(engine_live(live_record()));
        manager.ci_job_ended("s1", "j1").await;
        assert_eq!(
            deleted.lock().map_err(|_| "poisoned")?.clone(),
            ["/containers/c1?force=1"]
        );
        manager.sweep_ci_leftovers("s1").await;
        assert_eq!(
            deleted.lock().map_err(|_| "poisoned")?.clone(),
            ["/containers/c1?force=1", "/containers/c1?force=1"],
            "the running job's container stays"
        );
        // Review round 3: the ptyhost leaves the job's group a moment after it reported the
        // job's end; the removal waits for that rather than leaving all to the sweep.
        let leaving = delegation.ci_job_group("s1", "j3")?;
        std::fs::write(leaving.join("cgroup.events"), "populated 1\n")?;
        let events = leaving.join("cgroup.events");
        let exits = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::fs::write(events, "populated 0\n")
        });
        manager.ci_job_ended("s1", "j3").await;
        exits.await??;
        assert_eq!(
            deleted
                .lock()
                .map_err(|_| "poisoned")?
                .last()
                .map(String::as_str),
            Some("/containers/c3?force=1"),
            "removed once its last process left"
        );
        // Review round 2 (major): a job id told ended while its group still holds a process (a
        // requeued run's new attempt under the same id) has nothing removed; its own end, or the
        // sweep once the group is empty, takes it.
        manager.ci_job_ended("s1", "j2").await;
        assert_eq!(
            deleted.lock().map_err(|_| "poisoned")?.len(),
            3,
            "a running job's leftovers stay"
        );
        assert!(running.exists(), "a group with a process stays");
        let events: Vec<(String, String)> = manager
            .take_docker_events()
            .into_iter()
            .map(|e| (e.kind.to_owned(), e.detail))
            .collect();
        assert_eq!(
            events,
            [
                ("ci_leftovers".to_owned(), "1 0 job-end".to_owned()),
                ("ci_leftovers".to_owned(), "1 0 sweep".to_owned()),
                ("ci_leftovers".to_owned(), "1 0 job-end".to_owned())
            ]
        );
        engine.abort();
        Ok(())
    }

    /// SPEC-187 DKR-006, review round 2 (medium): pruning reaches only an engine that already
    /// runs, through its own control socket: never through the sandbox's socket, which would
    /// start a stopped engine and keep it (and the holder) from idling; never through a link.
    #[tokio::test]
    async fn only_a_running_engine_is_pruned_through_its_control_socket()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        assert_eq!(manager.prune_engine("s1").await, None, "no holder");
        // SPEC-497 HBA-003: the networks are counted and pruned the same way (review round 1).
        assert_eq!(manager.engine_networks("s1").await, None, "no holder");
        assert_eq!(manager.prune_engine_networks("s1").await, None, "no holder");
        let state = StatePaths::new(manager.state_dir("s1"));
        std::fs::create_dir_all(state.engine_control())?;
        let (front, front_hits) = fake_api(tokio::net::UnixListener::bind(state.engine_socket())?);
        let entry = manager.entry("s1");
        entry.lock().await.live = Some(Live::new(live_record(), Vec::new()));
        assert_eq!(
            manager.prune_engine("s1").await,
            None,
            "a holder without an engine"
        );
        assert_eq!(manager.engine_networks("s1").await, None, "no engine");
        entry.lock().await.live = Some(engine_live(live_record()));
        assert_eq!(manager.prune_engine("s1").await, None, "a stopped engine");
        assert_eq!(
            manager.engine_networks("s1").await,
            None,
            "a stopped engine"
        );
        assert_eq!(
            manager.prune_engine_networks("s1").await,
            None,
            "a stopped engine"
        );
        // A link the engine planted at its control socket is not followed.
        let decoy_path = data.path().join("decoy.sock");
        let (decoy, decoy_hits) = fake_api(tokio::net::UnixListener::bind(&decoy_path)?);
        let control = state.engine_control().join("docker.sock");
        std::os::unix::fs::symlink(&decoy_path, &control)?;
        assert_eq!(manager.prune_engine("s1").await, None, "a link");
        std::fs::remove_file(&control)?;
        let (engine, engine_hits) = fake_api(tokio::net::UnixListener::bind(&control)?);
        let measured = |containers: u64| crate::usage::Usage {
            bytes: containers,
            largest: vec![("containers".into(), containers)],
            ..crate::usage::Usage::default()
        };
        entry.lock().await.usage = Some((measured(5_000), Instant::now()));
        assert_eq!(manager.prune_engine("s1").await, Some(1000));
        assert_eq!(engine_hits.load(Ordering::SeqCst), 2);
        // Review round 3 (low): the engine's figure is credited at most up to the storage last
        // measured, so a forged answer cannot spare the other caches.
        entry.lock().await.usage = Some((measured(300), Instant::now()));
        assert_eq!(manager.prune_engine("s1").await, Some(300));
        entry.lock().await.usage = None;
        assert_eq!(
            manager.prune_engine("s1").await,
            Some(0),
            "nothing measured"
        );
        assert_eq!(
            front_hits.load(Ordering::SeqCst),
            0,
            "never the sandbox's socket"
        );
        assert_eq!(decoy_hits.load(Ordering::SeqCst), 0);
        for server in [front, decoy, engine] {
            server.abort();
        }
        Ok(())
    }

    /// SPEC-187 DKR-006: the engine's storage counts toward the quota as one consumer.
    #[test]
    fn engine_storage_counts_as_containers() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let dir = data.path().join("sandboxes/s1");
        let sysroot = data.path().join("sysroots/s1");
        std::fs::create_dir_all(dir.join("worktrees/t1"))?;
        std::fs::create_dir_all(sysroot.join("upper/usr"))?;
        std::fs::create_dir_all(sysroot.join("docker/overlay2/l1"))?;
        std::fs::write(dir.join("worktrees/t1/a"), vec![1u8; 64 * 1024])?;
        std::fs::write(sysroot.join("upper/usr/b"), vec![1u8; 128 * 1024])?;
        std::fs::write(sysroot.join("docker/overlay2/l1/c"), vec![1u8; 512 * 1024])?;
        let usage = measure_sandbox(&dir, &sysroot, &data.path().join("sysroots/s1.prod"));
        assert_eq!(
            usage.largest.first().map(|(name, _)| name.as_str()),
            Some("containers"),
            "{:?}",
            usage.largest
        );
        let names: Vec<&str> = usage.largest.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"system") && names.contains(&"worktrees/t1"),
            "{names:?}"
        );
        assert!(usage.bytes >= (64 + 128 + 512) * 1024, "{}", usage.bytes);
        Ok(())
    }

    /// SPEC-187 DKR-001/003/005: the engine the holder starts is `sandbox-exec --engine`, which
    /// joins the engine's group and then execs the engine's `bwrap` (its storage under the
    /// sandbox's sysroot, the sandbox directory, the system layer).
    #[test]
    fn the_engine_joins_its_group_and_keeps_its_storage_by_the_system_layer()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        let cgroup = PathBuf::from("/sys/fs/cgroup/yulius.service/sandboxes/s1/engine");
        let spec = manager.engine_spec(&request(data.path()), &cgroup);
        let d = data.path().to_string_lossy().into_owned();
        assert_eq!(
            spec.command[..6],
            [
                "/usr/local/bin/yulius",
                "sandbox-exec",
                "--engine",
                "--cgroup",
                "/sys/fs/cgroup/yulius.service/sandboxes/s1/engine",
                "--guard",
            ]
        );
        // The sandbox's harness folders, masked for the engine (SPEC-652 MEM-010).
        let masks: Vec<yulius_contracts::ptyhost::GuardPath> =
            serde_json::from_str(&spec.command[6])?;
        assert_eq!(
            masks,
            yulius_contracts::ptyhost::sandbox_masks(&format!("{d}/sandboxes/s1"))
        );
        assert_eq!(spec.command[7], "--");
        assert_eq!(spec.command[8], "/usr/bin/bwrap");
        let has = |window: &[String]| spec.command.windows(window.len()).any(|w| w == window);
        assert!(has(&[
            "--bind".into(),
            format!("{d}/sysroots/s1/docker"),
            "/var/lib/docker".into()
        ]));
        assert!(has(&[
            "--bind".into(),
            format!("{d}/sandboxes/s1"),
            format!("{d}/sandboxes/s1")
        ]));
        assert!(has(&[
            "--bind-try".into(),
            format!("{d}/runtime/sandboxes/s1/root/usr"),
            "/usr".into()
        ]));
        assert!(has(&["--tmpfs".into(), d.clone()]));
        assert!(has(&["--hostname".into(), "s1".into()]));
        assert_eq!(spec.cgroup, cgroup.to_string_lossy());
        assert_eq!(spec.idle_secs, 300);
        Ok(())
    }

    fn request(data: &Path) -> SandboxRequest {
        SandboxRequest {
            id: "s1".into(),
            hostname: "s1".into(),
            dir: data.join("sandboxes/s1"),
            port_first: 20_000,
            port_last: 20_019,
        }
    }

    #[tokio::test]
    async fn an_old_over_quota_reading_is_measured_again_before_it_refuses()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 10_000_000);
        let request = request(data.path());
        std::fs::create_dir_all(&request.dir)?;
        // The old reading says 112 GB; the directory is nearly empty now.
        manager
            .set_usage_aged(
                "s1",
                112_000_000_000,
                QUOTA_RECHECK + Duration::from_secs(1),
            )
            .await;
        let entry = manager.entry("s1");
        let mut state = entry.lock().await;
        assert!(manager.quota_refusal(&request, &mut state).await.is_none());
        let fresh = state
            .usage
            .as_ref()
            .map(|(u, _)| u.bytes)
            .unwrap_or(u64::MAX);
        assert!(fresh < 10_000_000, "the reading was replaced: {fresh}");
        Ok(())
    }

    #[tokio::test]
    async fn a_fresh_over_quota_reading_refuses_without_measuring()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 10_000_000);
        let request = request(data.path());
        std::fs::create_dir_all(&request.dir)?;
        manager.set_usage("s1", 112_000_000_000).await;
        let entry = manager.entry("s1");
        let mut state = entry.lock().await;
        let refusal = manager.quota_refusal(&request, &mut state).await;
        assert!(matches!(refusal, Some(PrepareError::OverQuota { .. })));
        assert_eq!(
            state.usage.as_ref().map(|(u, _)| u.bytes),
            Some(112_000_000_000)
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_old_reading_that_is_still_over_quota_still_refuses()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        let request = request(data.path());
        std::fs::create_dir_all(&request.dir)?;
        std::fs::write(request.dir.join("big"), vec![1u8; 64 * 1024])?;
        manager
            .set_usage_aged("s1", 5, QUOTA_RECHECK + Duration::from_secs(1))
            .await;
        let entry = manager.entry("s1");
        let mut state = entry.lock().await;
        assert!(matches!(
            manager.quota_refusal(&request, &mut state).await,
            Some(PrepareError::OverQuota { .. })
        ));
        Ok(())
    }

    /// SPEC-459 HSL-006: the quota is changed while the manager runs. A sandbox over the old
    /// quota is let in under a bigger one, refused again under a smaller one, and reports the
    /// quota in force; without a quota nothing is refused or reported.
    #[tokio::test]
    async fn a_changed_quota_applies_to_the_next_check() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 100);
        let request = request(data.path());
        manager.set_usage("s1", 500).await;
        let refused = |manager: Arc<Manager>, request: SandboxRequest| async move {
            let entry = manager.entry("s1");
            let mut state = entry.lock().await;
            manager.quota_refusal(&request, &mut state).await
        };
        assert!(refused(manager.clone(), request.clone()).await.is_some());
        assert_eq!(manager.disk("s1").await, Some((500, 100)));

        manager.set_disk_quota(Some(1_000));
        assert!(refused(manager.clone(), request.clone()).await.is_none());
        assert_eq!(manager.disk("s1").await, Some((500, 1_000)));

        manager.set_disk_quota(Some(400));
        assert_eq!(
            refused(manager.clone(), request.clone()).await,
            Some(PrepareError::OverQuota {
                used: human(500),
                limit: human(400),
            })
        );

        manager.set_disk_quota(None);
        assert!(refused(manager.clone(), request.clone()).await.is_none());
        assert_eq!(manager.disk("s1").await, None);
        Ok(())
    }

    /// SPEC-206 RMV-004: a retired sandbox's holder stops once it was retired for the grace and
    /// no run holds a lease, whatever else runs in it; any other stops only once nothing runs
    /// and it was idle.
    #[test]
    fn a_retired_holder_stops_after_its_grace_once_no_lease_is_left() {
        let after = Duration::from_secs(300);
        let now = Duration::ZERO;
        let retired = Some(RETIRE_GRACE);
        assert!(
            holder_stops(retired, 0, 5, now, after, false),
            "leftovers do not hold it up"
        );
        assert!(
            !holder_stops(retired, 1, 0, now, after, false),
            "a run still holds a lease"
        );
        assert!(
            !holder_stops(Some(Duration::from_secs(5)), 0, 5, now, after, false),
            "its runs are still ending"
        );
        assert!(
            !holder_stops(None, 0, 5, after, after, false),
            "something runs in it"
        );
        assert!(
            !holder_stops(None, 0, 0, now, after, false),
            "not idle long enough"
        );
        assert!(holder_stops(None, 0, 0, after, after, false));
        assert!(
            holder_stops(None, 0, 0, Duration::from_secs(60), after, true),
            "superseded"
        );
    }

    /// SPEC-206 RMV-004 (review of #193): a start under way at the removal starts no holder for
    /// the removed sandbox.
    #[tokio::test]
    async fn a_retired_sandbox_is_not_prepared() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 40);
        manager.retire("s1").await;
        assert_eq!(
            manager.prepare(&request(data.path())).await.err(),
            Some(PrepareError::Retired)
        );
        Ok(())
    }

    /// A missed DELETE still retires an existing holder on disk. An unreadable record or
    /// a lease keeps the range held and the other holder's absent tree stays absent.
    #[tokio::test]
    async fn recovering_a_retirement_waits_for_records_and_leases()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 40);
        let state = manager.state_dir("s1");
        std::fs::create_dir_all(&state)?;
        let record = state.join(HOLDER_RECORD_FILE);
        std::fs::write(&record, b"{")?;
        assert!(!manager.retired_ports_idle("s1").await);
        assert!(state.join(RETIRED).is_file());
        assert!(!manager.state_dir("s1.prod").exists());
        std::fs::remove_file(record)?;
        manager.entry("s1").lock().await.leases = 1;
        assert!(!manager.retired_ports_idle("s1").await);
        manager.release("s1").await;
        assert!(manager.retired_ports_idle("s1").await);
        let restarted = Manager::new(manager.config().clone());
        assert_eq!(
            restarted.prepare(&request(data.path())).await,
            Err(PrepareError::Retired)
        );
        Ok(())
    }

    /// An absent production holder needs no marker. A non-directory at its state path
    /// is an error marking retirement, so even an idle development holder keeps the range.
    #[tokio::test]
    async fn a_retirement_marker_error_keeps_the_removed_range_reserved()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 40);
        let dev = manager.state_dir("s1");
        std::fs::create_dir_all(&dev)?;
        let production = manager.state_dir("s1.prod");
        std::fs::write(&production, b"not a state directory")?;
        assert!(
            !manager.holder_live("s1").await,
            "the development holder has ended"
        );
        assert!(
            !manager.retired_ports_idle("s1").await,
            "production retirement could not be marked"
        );
        std::fs::remove_file(&production)?;
        assert!(manager.retired_ports_idle("s1").await);
        Ok(())
    }

    /// Dates the retirement of `s1` `ago` back.
    fn retired_ago(manager: &Manager, ago: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let marker = std::fs::File::options()
            .write(true)
            .open(manager.state_dir("s1").join(RETIRED))?;
        marker.set_modified(std::time::SystemTime::now() - ago)?;
        Ok(())
    }

    /// A manager whose adopted holder of `s1` is a child `sleep` of this process (its namespaces
    /// are this process's, so it reads as alive and stopping it ends only the child). The child
    /// is reaped in the background; returns its pid.
    fn adopting_child(data: &Path) -> Result<(Arc<Manager>, u32), Box<dyn std::error::Error>> {
        let manager = adopting(data, Err(DOCKER_OFF.into()))?;
        let mut child = std::process::Command::new("sleep").arg("600").spawn()?;
        let pid = child.id();
        std::thread::spawn(move || child.wait());
        let state = StatePaths::new(manager.state_dir("s1"));
        let record = HolderRecord {
            inner_pid: pid,
            ..read_json::<HolderRecord>(&state.record())?
        };
        write_json(&state.record(), &record)?;
        Ok((manager, pid))
    }

    fn gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::holder::exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        !crate::holder::exists(pid)
    }

    /// SPEC-206 RMV-004 (#624): a removed sandbox's holder stopped only once nothing ran in its
    /// namespaces, so a process there (here this test itself; in a sandbox its Docker engine and
    /// containers) kept it up for good. Retired, a tick after the grace stops it.
    #[tokio::test]
    async fn the_tick_stops_a_retired_holder_while_other_processes_run_in_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let (manager, pid) = adopting_child(data.path())?;
        assert!(manager.adopt_one("s1").await, "adopted");
        assert!(
            manager.processes("s1").await > 0,
            "this test runs in its namespaces"
        );
        manager.tick().await;
        assert!(
            crate::holder::exists(pid),
            "not retired: kept while something runs"
        );

        manager.retire("s1").await;
        manager.tick().await;
        assert!(
            crate::holder::exists(pid),
            "within the grace: its runs are still ending"
        );

        retired_ago(&manager, RETIRE_GRACE)?;
        manager.tick().await;
        let stopped = gone(pid);
        kill(pid);
        assert!(stopped, "the retired holder is stopped");
        assert!(manager.entry("s1").lock().await.live.is_none());
        Ok(())
    }

    /// SPEC-215 LFT-002: a holder recorded alive or adopted is live; once it stopped and
    /// nothing uses its namespaces any more, it is not, and its system may go.
    #[tokio::test]
    async fn a_holder_is_live_until_it_stopped_and_nothing_uses_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let (manager, pid) = adopting_child(data.path())?;
        assert!(!manager.holder_live("other").await, "no holder");
        let unreadable = manager.state_dir("torn");
        std::fs::create_dir_all(&unreadable)?;
        std::fs::write(unreadable.join(HOLDER_RECORD_FILE), "{\"inner_pid\": 4")?;
        assert!(
            manager.holder_live("torn").await,
            "an unreadable record is never guessed dead"
        );
        assert!(manager.holder_live("s1").await, "recorded alive");
        assert!(manager.adopt_one("s1").await);
        assert!(manager.holder_live("s1").await, "adopted");
        manager.retire("s1").await;
        retired_ago(&manager, RETIRE_GRACE)?;
        manager.tick().await;
        let stopped = gone(pid);
        kill(pid);
        assert!(stopped, "the retired holder is stopped");
        // The stand-in shares this test's namespaces: the test itself still uses them.
        let stale = manager.entry("s1").lock().await.stale.clone();
        if let Some(stale) = stale {
            assert_eq!(
                manager.holder_live("s1").await,
                Manager::runs_in(stale.net_ns, &[]) > 0,
                "an ended holder is live while processes use its namespaces"
            );
            manager.entry("s1").lock().await.stale = Some(HolderRecord {
                net_ns: u64::MAX,
                ..stale
            });
        }
        assert!(!manager.holder_live("s1").await, "stopped and unused");
        Ok(())
    }

    /// Security scan (high): a sandbox name is one path component of the id alphabet; any other
    /// name never reaches outside the state and system trees, and retiring it writes nothing.
    #[tokio::test]
    async fn a_name_outside_the_id_alphabet_never_leaves_its_tree()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 30);
        let states = data.path().join("runtime/sandboxes");
        for name in ["../../etc", "..", "/tmp", "a/b", "", "x\0"] {
            assert!(manager.state_dir(name).starts_with(&states), "{name:?}");
            assert_eq!(
                manager.state_dir(name).parent(),
                Some(states.as_path()),
                "{name:?}"
            );
            assert!(
                manager
                    .sysroot(name)
                    .starts_with(data.path().join("sysroots")),
                "{name:?}"
            );
            assert!(!manager.holder_live(name).await, "{name:?}");
        }
        manager.retire("../../escaped").await;
        assert!(!data.path().join("escaped").exists());
        assert!(!data.path().join("runtime/escaped").exists());
        assert_eq!(
            manager.state_dir("s-1_A"),
            states.join("s-1_A"),
            "a real id is kept"
        );
        Ok(())
    }

    /// SPEC-215: only a system tree of the data directory is removed through the mapped
    /// namespace; a tree the service user owns goes without it.
    #[tokio::test]
    async fn only_a_sandbox_s_system_tree_is_removed() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 30);
        let d = data.path();
        for refused in [
            d.join("master.key"),
            d.join("users/u1"),
            d.join("sandboxes"),
            d.join("sysroots"),
            d.join("sysroots/../sandboxes/s1"),
            PathBuf::from("/tmp"),
            // Of the CD directory only production runs' files (SPEC-279 PDE-004).
            d.join("cd"),
            d.join("cd/s1"),
            d.join("cd/s1/source.git"),
            d.join("cd/s1/cache/prod"),
            d.join("cd/s1/runs"),
            d.join("cd/s1/runs/../source.git"),
        ] {
            assert!(
                manager.remove_owned_tree(&refused).await.is_err(),
                "{}",
                refused.display()
            );
        }
        std::fs::create_dir_all(d.join("sandboxes/s1"))?;
        for tree in [
            "sysroots/s1/upper/usr",
            "runtime/sandboxes/s1/engine",
            "system/base-0/usr",
        ] {
            std::fs::create_dir_all(d.join(tree))?;
            std::fs::write(d.join(tree).join("f"), "x")?;
        }
        for root in ["sysroots/s1", "runtime/sandboxes/s1", "system/base-0"] {
            manager.remove_owned_tree(&d.join(root)).await?;
            assert!(!d.join(root).exists(), "{root} removed");
        }
        assert!(d.join("sandboxes/s1").exists());
        manager.remove_owned_tree(&d.join("sysroots/s1")).await?;
        Ok(())
    }

    /// ADR-0176 (review of SPEC-228): a tree below `sandboxes/` is removed only by the walk
    /// from the data directory, never by path; the system trees keep the by-path attempt.
    #[test]
    fn only_trees_below_sandboxes_skip_the_by_path_removal() {
        let d = Path::new("/data");
        for walked in [
            "sandboxes/s1/ci",
            "sandboxes/s1/ci/jobs/old",
            "sandboxes/docker-smoke-1",
            // A production run and its engine's containers write there (SPEC-279 PDE-004).
            "cd/s1/runs/cd-r",
            // And a CD environment's cache, emptied when a run passed its disk bound (SPEC-300).
            "cd/s1/cache/prod",
            "cd/s1/cache/dev/go/pkg",
        ] {
            assert!(reachable_by_runs(d, &d.join(walked)), "{walked}");
        }
        for neither in ["cd/s1/cache", "cd/s1/source.git", "cd/../cache/x"] {
            assert!(!reachable_by_runs(d, &d.join(neither)), "{neither}");
        }
        for by_path in ["sysroots/s1", "runtime/sandboxes/s1", "system/base-0"] {
            assert!(!reachable_by_runs(d, &d.join(by_path)), "{by_path}");
        }
    }

    /// SPEC-215 / ADR-0161: a base that is neither current nor mounted by a live holder is
    /// unused; the current one and a live holder's are not.
    #[tokio::test]
    async fn unused_bases_are_neither_current_nor_mounted() -> Result<(), Box<dyn std::error::Error>>
    {
        let data = tempfile::tempdir()?;
        let (manager, pid) = adopting_child(data.path())?;
        for base in ["base-0", "base-2"] {
            std::fs::create_dir_all(manager.bases.path(base))?;
        }
        write_json(
            &manager.bases.dir.join("current.json"),
            &crate::base::BaseInfo {
                id: "base-2".into(),
                fingerprint: "f".into(),
                built_at_ms: 1,
                format: crate::base::BASE_FORMAT,
            },
        )?;
        let unused = manager.unused_bases().await;
        kill(pid);
        assert_eq!(
            unused,
            vec![manager.bases.path("base-0")],
            "base-1 is the live holder's, base-2 is current"
        );
        Ok(())
    }

    /// SPEC-206 RMV-004: after a restart, a retired sandbox's holder is stopped, not adopted.
    #[tokio::test]
    async fn a_retired_holder_is_stopped_instead_of_adopted()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let (manager, pid) = adopting_child(data.path())?;
        manager.retire("s1").await;
        let restarted = Manager::new(manager.config().clone());
        assert!(!restarted.adopt_one("s1").await, "not adopted");
        let stopped = gone(pid);
        kill(pid);
        assert!(stopped, "the retired holder is stopped");
        assert!(restarted.entry("s1").lock().await.live.is_none());
        Ok(())
    }

    /// SPEC-279 PDE-001, PDE-005: a sandbox's production holder has trees of its own (none
    /// below the sandbox's), a valid name for every by-name operation, and is retired with its
    /// sandbox.
    #[tokio::test]
    async fn a_production_holder_has_its_own_trees_and_is_retired_with_its_sandbox()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 40);
        let d = data.path();
        assert_eq!(
            manager.state_dir("s1.prod"),
            d.join("runtime/sandboxes/s1.prod")
        );
        assert_eq!(manager.sysroot("s1.prod"), d.join("sysroots/s1.prod"));
        assert_eq!(
            manager.state_dir("x.y"),
            d.join("runtime/sandboxes/.invalid")
        );
        assert!(!manager.holder_live("s1.prod").await, "no record yet");
        manager.retire("s1").await;
        assert!(manager.retired("s1") && manager.retired("s1.prod"));
        assert_eq!(
            manager.prepare_production(&request(d)).await.err(),
            Some(PrepareError::Retired)
        );
        Ok(())
    }

    /// SPEC-279 PDE-001, PDE-004: the production request has no dev ports, a host name of its
    /// own and the production runs directory; an adopted production holder gets the same.
    #[test]
    fn a_production_request_has_its_runs_directory_and_no_ports()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 40);
        let production = manager.production_request(&request(data.path()));
        assert_eq!(
            production,
            SandboxRequest {
                id: "s1.prod".into(),
                hostname: "s1-prod".into(),
                dir: data.path().join("cd/s1/runs"),
                port_first: 0,
                port_last: 0,
            }
        );
        let mut long = request(data.path());
        long.hostname = "a".repeat(63);
        let hostname = manager.production_request(&long).hostname;
        assert_eq!(hostname, format!("{}-prod", "a".repeat(58)));
        assert!(yulius_contracts::sandbox::valid_hostname(&hostname));
        let state = StatePaths::new(manager.state_dir("s1.prod"));
        std::fs::create_dir_all(&state.dir)?;
        let spec = serde_json::json!({"hostname": "s1-prod", "port_first": 0, "port_last": 0});
        std::fs::write(state.spec(), serde_json::to_vec(&spec)?)?;
        let adopted = manager.adopted_request("s1.prod");
        assert_eq!(adopted.dir, data.path().join("cd/s1/runs"));
        assert_eq!(adopted.hostname, "s1-prod");
        Ok(())
    }

    /// SPEC-279 PDE-002, PDE-004: the production engine keeps its storage by its own system
    /// layer, binds the production runs directory and nothing of the sandbox.
    #[test]
    fn the_production_engine_binds_its_runs_and_nothing_of_the_sandbox()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        let cgroup = PathBuf::from("/sys/fs/cgroup/yulius.service/sandboxes/s1.prod/engine");
        let production = manager.production_request(&request(data.path()));
        let spec = manager.engine_spec(&production, &cgroup);
        let d = data.path().to_string_lossy().into_owned();
        let has = |window: &[String]| spec.command.windows(window.len()).any(|w| w == window);
        assert!(has(&[
            "--bind".into(),
            format!("{d}/sysroots/s1.prod/docker"),
            "/var/lib/docker".into()
        ]));
        assert!(has(&[
            "--bind".into(),
            format!("{d}/cd/s1/runs"),
            format!("{d}/cd/s1/runs")
        ]));
        assert!(has(&[
            "--bind-try".into(),
            format!("{d}/runtime/sandboxes/s1.prod/root/usr"),
            "/usr".into()
        ]));
        assert!(has(&["--hostname".into(), "s1-prod".into()]));
        let everything = spec.command.join(" ");
        for sandbox in ["/sandboxes/s1 ", "/sandboxes/s1/", "/sysroots/s1/"] {
            assert!(!everything.contains(sandbox), "{sandbox} in {everything}");
        }
        Ok(())
    }

    /// SPEC-279 PDE-001: a production holder has no forwarded ports (no relay to Yulius's
    /// internal port) and no relays on the host.
    #[tokio::test]
    async fn a_production_holder_has_no_forwards_and_no_relays() {
        let data = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut config = config(data.path(), 1 << 40);
        config.forwards = BTreeMap::from([(
            8081,
            "127.0.0.1:8081".parse().unwrap_or_else(|e| panic!("{e}")),
        )]);
        let manager = Manager::new(config);
        assert_eq!(manager.forwards_of("s1"), vec![8081]);
        assert!(manager.forwards_of("s1.prod").is_empty());
        // Review finding 5: nor on its engine's bridge, whatever its holder spec says.
        assert_eq!(
            manager.bridge_forwards("s1"),
            vec![8081],
            "no spec: an old holder"
        );
        assert!(manager.bridge_forwards("s1.prod").is_empty());
        let record = live_record();
        for name in ["s1", "s1.prod"] {
            std::fs::create_dir_all(manager.state_dir(name)).unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(manager.start_relays("s1.prod", &record).is_empty());
        assert!(!manager.state_dir("s1.prod").join("out.sock").exists());
        let relays = manager.start_relays("s1", &record);
        assert_eq!(relays.len(), 2, "out.sock and the dev ports");
        for relay in relays {
            relay.abort();
        }
    }

    /// SPEC-279 PDE-006: a production prepare is refused while its sandbox is over its quota,
    /// and the production holder's layer and storage count toward it as one consumer.
    #[tokio::test]
    async fn production_counts_toward_and_is_refused_by_its_sandbox_s_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 10_000_000);
        let request = request(data.path());
        std::fs::create_dir_all(&request.dir)?;
        manager.set_usage("s1", 112_000_000_000).await;
        assert!(matches!(
            manager.prepare_production(&request).await,
            Err(PrepareError::OverQuota { .. })
        ));
        let production = data.path().join("sysroots/s1.prod");
        std::fs::create_dir_all(production.join("docker/overlay2/l1"))?;
        std::fs::write(
            production.join("docker/overlay2/l1/c"),
            vec![1u8; 512 * 1024],
        )?;
        // Review finding 1: production's own files never refuse a production deploy (a
        // deploy starts the engine whose storage the upkeep then prunes); the agents' do.
        manager.entry("s1").lock().await.usage = Some((
            crate::usage::Usage {
                bytes: 20_000_000,
                largest: vec![("production".into(), 15_000_000)],
                ..crate::usage::Usage::default()
            },
            Instant::now(),
        ));
        assert_eq!(manager.prepare_production(&request).await, Ok(None));
        manager.entry("s1").lock().await.usage = Some((
            crate::usage::Usage {
                bytes: 20_000_000,
                largest: vec![("production".into(), 5_000_000)],
                ..crate::usage::Usage::default()
            },
            Instant::now(),
        ));
        assert!(matches!(
            manager.prepare_production(&request).await,
            Err(PrepareError::OverQuota { .. })
        ));
        let usage = manager.measure_disk(&request).await;
        assert!(usage.is_some_and(|(used, _)| used >= 512 * 1024));
        let largest = manager.largest("s1").await;
        assert_eq!(
            largest.first().map(|(name, _)| name.as_str()),
            Some("production"),
            "{largest:?}"
        );
        Ok(())
    }

    /// SPEC-279 PDE-006: the disk upkeep's prune of a sandbox reaches its running production
    /// engine through that engine's control socket, credited up to the production storage
    /// measured.
    #[tokio::test]
    async fn pruning_a_sandbox_prunes_its_running_production_engine()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        let state = StatePaths::new(manager.state_dir("s1.prod"));
        std::fs::create_dir_all(state.engine_control())?;
        let control = state.engine_control().join("docker.sock");
        let (engine, hits) = fake_api(tokio::net::UnixListener::bind(&control)?);
        manager.entry("s1.prod").lock().await.live = Some(engine_live(HolderRecord {
            sandbox: "s1.prod".into(),
            ..live_record()
        }));
        manager.entry("s1").lock().await.usage = Some((
            crate::usage::Usage {
                bytes: 5_000,
                largest: vec![("production".into(), 700)],
                ..crate::usage::Usage::default()
            },
            Instant::now(),
        ));
        assert_eq!(manager.prune_engine("s1").await, Some(700));
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        engine.abort();
        Ok(())
    }
}
