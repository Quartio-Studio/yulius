//! The server's side of sandbox isolation (SPEC-002 ADR-22): holders on demand, their relays,
//! limits, quotas, idle stops, base builds and adoption after a restart.
//!
//! Ownership and bounds:
//! - **One holder per sandbox**, started by [`Manager::prepare`] under a per-sandbox lock.
//!   Starting runs `yulius sandbox-host start` as a child process within [`START_TIMEOUT`].
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use yulius_contracts::ptyhost::SandboxJoin;
use yulius_contracts::sandbox::{
    DockerFrontRecord, DockerFrontSpec, DockerServe, EngineSpec, HOLDER_RECORD_FILE,
    HOLDER_RECORD_VERSION, HolderRecord, HolderSpec, IdMap,
};

use crate::base::{BaseInfo, Bases};
use crate::cgroup::{Delegation, Limits, SandboxGroups};
use crate::holder::{StatePaths, alive, read_json, write_json};
use crate::usage::Usage;

/// Bound on starting a holder.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
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

/// Where a system layer has `dockerd` (`docker-ce` puts it in `usr/bin`, `docker.io` in
/// `usr/sbin`; `usr/local/sbin` comes first on the engine's `PATH`).
pub const DOCKERD_PATHS: [&str; 4] = [
    "usr/local/sbin/dockerd",
    "usr/local/bin/dockerd",
    "usr/sbin/dockerd",
    "usr/bin/dockerd",
];

/// Whether the system a sandbox's engine runs has `dockerd` (SPEC-199 DSY-002): the sandbox's
/// overlay `upper` layer decides where it has an entry, else its `base`. The upper layer is the
/// sandbox's to write, so it is walked from one directory descriptor to the next without
/// following links (no path is looked up twice): a link at `dockerd` counts (it resolves inside
/// the sandbox), and anything else over a path (a whiteout, a directory, a link over a parent)
/// hides the base's.
#[must_use]
pub fn system_has_dockerd(base: &Path, upper: &Path) -> bool {
    DOCKERD_PATHS
        .iter()
        .any(|path| upper_has(upper, path).unwrap_or_else(|| base.join(path).is_file()))
}

/// The `dockerd` a sandbox's engine runs when Docker installed in the sandbox shadows its
/// base's (SPEC-203 HST-017): the first of [`DOCKERD_PATHS`] in the merged system comes from the
/// upper layer, at another path than the base's own (Ubuntu's `docker.io` in `usr/sbin` over
/// `docker-ce` in `usr/bin`). `None` when the base's runs, the base has none, or the sandbox
/// reinstalled Docker at the base's path.
#[must_use]
pub fn own_engine(base: &Path, upper: &Path) -> Option<&'static str> {
    let base_first = DOCKERD_PATHS
        .iter()
        .find(|path| base.join(path).is_file())?;
    for path in DOCKERD_PATHS {
        match upper_has(upper, path) {
            Some(true) => return (path != *base_first).then_some(path),
            Some(false) => {}
            None if base.join(path).is_file() => return None,
            None => {}
        }
    }
    None
}

/// What an overlay upper layer says about the file at `relative`: `Some(true)` it provides one,
/// `Some(false)` it hides the lower one, `None` it has no entry on the way. Never follows a link.
#[cfg(target_os = "linux")]
fn upper_has(upper: &Path, relative: &str) -> Option<bool> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
    use rustix::io::Errno;
    let walk = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = openat(rustix::fs::CWD, upper, walk, Mode::empty()).ok()?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return match statat(&dir, part, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => Some(matches!(
                    FileType::from_raw_mode(stat.st_mode),
                    FileType::RegularFile | FileType::Symlink
                )),
                Err(_) => None,
            };
        }
        dir = match openat(&dir, part, walk, Mode::empty()) {
            Ok(next) => next,
            // A link or a file (a whiteout too) over the directory hides the lower one.
            Err(Errno::LOOP | Errno::NOTDIR) => return Some(false),
            Err(_) => return None,
        };
    }
    None
}

/// Sandboxes are Linux only: elsewhere the base decides.
#[cfg(not(target_os = "linux"))]
fn upper_has(_upper: &Path, _relative: &str) -> Option<bool> {
    None
}

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
    /// Host path of the sandbox system's `/etc/passwd` (upper layer, else base).
    pub system_passwd: Option<PathBuf>,
    /// Host path of its `/etc/group`.
    pub system_group: Option<PathBuf>,
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
}

impl Live {
    fn new(record: HolderRecord, relays: Vec<JoinHandle<()>>) -> Self {
        Self {
            record,
            relays,
            front: None,
            bound: HashMap::new(),
        }
    }

    /// The front's pid, when it has one.
    fn front_pid(&self) -> u32 {
        self.front.as_ref().map_or(0, |f| f.pid)
    }

    /// Whether an engine may run (its own front's, or a SPEC-187 holder's).
    fn serves_engine(&self) -> bool {
        self.record.engine || self.front.as_ref().is_some_and(|f| f.mode != "refuse")
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
    bases: Bases,
    sandboxes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<SandboxState>>>>,
    build: Mutex<BuildState>,
    building: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            bases: Bases::new(&config.data_dir),
            config,
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

    fn state_dir(&self, sandbox: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runtime")
            .join("sandboxes")
            .join(sandbox)
    }

    fn sysroot(&self, sandbox: &str) -> PathBuf {
        self.config.data_dir.join("sysroots").join(sandbox)
    }

    /// Why sandboxes get no Docker engine (SPEC-196 DKL-005), `None` when they may (each one
    /// that has `dockerd` in its own system, SPEC-199): the setting, the cgroup delegation, and a
    /// `yulius` executable the sandbox's root neither owns nor may write through its group or as
    /// anyone (the engine's read-only binds are not locked, so an engine could remount and
    /// rewrite it; ADR-0151).
    #[must_use]
    pub fn docker_unavailable(&self) -> Option<String> {
        use std::os::unix::fs::MetadataExt as _;
        if let Err(reason) = &self.config.docker {
            return Some(reason.clone());
        }
        if self.config.delegation.is_none() {
            return Some(DOCKER_NO_DELEGATION.to_owned());
        }
        // Owned by the sandbox's root, or writable by its group (the service group) or anyone.
        let exe_writable = std::fs::metadata(&self.config.exe)
            .is_ok_and(|meta| meta.uid() == self.config.uid.host || meta.mode() & 0o022 != 0);
        exe_writable.then(|| DOCKER_WRITABLE_EXE.to_owned())
    }

    /// Whether sandboxes get a Docker engine ([`Manager::docker_unavailable`] says why not).
    #[must_use]
    pub fn docker(&self) -> bool {
        self.docker_unavailable().is_none()
    }

    /// Whether `sandbox`'s runs reach a Docker engine (SPEC-187 DKR-010). Every live holder gets
    /// a front whatever its age (SPEC-196 DKL-003), so this is the host's answer.
    pub async fn docker_for(&self, _sandbox: &str) -> bool {
        self.docker()
    }

    /// How `request`'s front serves its socket beside the holder of `record` (ADR-0158). Asked
    /// at every tick, it only reads paths: the sandbox's groups are prepared when an engine front
    /// starts ([`Manager::start_front`]).
    fn front_mode(&self, request: &SandboxRequest, record: &HolderRecord) -> DockerServe {
        if let Some(reason) = self.docker_unavailable() {
            return DockerServe::Refuse { reason };
        }
        if record.engine {
            let state = StatePaths::new(self.state_dir(&request.id));
            return DockerServe::Relay {
                socket: state.engine_socket().to_string_lossy().into_owned(),
            };
        }
        let base = self.bases.path(&record.base_id);
        if !system_has_dockerd(&base, &self.sysroot(&request.id).join("upper")) {
            return DockerServe::Refuse {
                reason: DOCKER_NO_DOCKERD.to_owned(),
            };
        }
        match self.group_paths(&request.id) {
            Some(groups) => DockerServe::Engine {
                engine: self.engine_spec(request, &groups.engine),
            },
            None => DockerServe::Refuse {
                reason: DOCKER_NO_DELEGATION.to_owned(),
            },
        }
    }

    /// The paths of `sandbox`'s groups, without preparing them; `None` without delegation.
    fn group_paths(&self, sandbox: &str) -> Option<SandboxGroups> {
        Some(self.config.delegation.as_ref()?.sandbox_paths(sandbox))
    }

    /// The request of a sandbox known only from its holder's files (an adopted holder before
    /// any run asked for it): its host name and ports from `holder.json`.
    fn adopted_request(&self, sandbox: &str) -> SandboxRequest {
        let spec = read_json::<HolderSpec>(&StatePaths::new(self.state_dir(sandbox)).spec()).ok();
        SandboxRequest {
            id: sandbox.to_owned(),
            hostname: spec
                .as_ref()
                .map_or_else(|| sandbox.to_owned(), |s| s.hostname.clone()),
            dir: self.config.data_dir.join("sandboxes").join(sandbox),
            port_first: spec.as_ref().map_or(0, |s| s.port_first),
            port_last: spec.as_ref().map_or(0, |s| s.port_last),
        }
    }

    /// Forwarded ports the front also serves on the engine's bridge: those of a holder whose
    /// forwards listen on loopback only (started before engines; SPEC-187 specs carry `engine`,
    /// newer ones `containers`).
    fn bridge_forwards(&self, sandbox: &str) -> Vec<u16> {
        let spec = std::fs::read(StatePaths::new(self.state_dir(sandbox)).spec())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        let containers = spec
            .as_ref()
            .is_some_and(|s| s["containers"].as_bool() == Some(true) || !s["engine"].is_null());
        if containers {
            Vec::new()
        } else {
            self.config.forwards.keys().copied().collect()
        }
    }

    /// Keeps `sandbox`'s front serving in the mode it should (SPEC-196 DKL-003): started when the
    /// holder has none or it ended, replaced when the mode changed. An engine's other settings
    /// change only with a new holder, so a running engine is never cut for them.
    async fn keep_front(&self, sandbox: &str, state: &mut SandboxState) {
        let request = state
            .request
            .clone()
            .unwrap_or_else(|| self.adopted_request(sandbox));
        let Some(live) = state.live.as_mut() else {
            return;
        };
        let net_ns = live.record.net_ns;
        let current = live
            .front
            .as_ref()
            .map(|front| (front, crate::docker_front::is_front(front.pid, net_ns)));
        let serve = match front_action(current, self.front_mode(&request, &live.record)) {
            FrontAction::Keep => {
                // A restart of `yulius.service` prunes an idle engine's empty leaf, and a front
                // from an older binary does not make it again (SPEC-201 DBX-006); a SPEC-187
                // holder's own engine, behind a relay front, uses the same leaf.
                if live
                    .front
                    .as_ref()
                    .is_some_and(|f| f.mode == "engine" || f.mode == "relay")
                    && let Some(groups) = self.group_paths(sandbox)
                {
                    match crate::cgroup::restore_leaf(&groups.engine) {
                        Ok(true) => tracing::info!(sandbox, "recreated the engine's cgroup"),
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(sandbox, %error, "cannot recreate the engine's cgroup");
                        }
                    }
                }
                return;
            }
            FrontAction::Start(serve) => {
                if live.front.is_some() {
                    tracing::warn!(
                        sandbox,
                        "the sandbox's Docker front ended; starting it again"
                    );
                }
                serve
            }
            FrontAction::Replace(serve) => {
                tracing::info!(
                    sandbox,
                    to = serve.kind(),
                    "replacing the sandbox's Docker front"
                );
                let paths = StatePaths::new(self.state_dir(sandbox));
                let _ = tokio::task::spawn_blocking(move || {
                    crate::docker_front::stop(&paths, net_ns);
                })
                .await;
                serve
            }
        };
        live.front = None;
        let forwards = self.bridge_forwards(sandbox);
        match self
            .start_front(sandbox, &live.record, serve, forwards)
            .await
        {
            Ok(front) => {
                tracing::info!(sandbox, mode = %front.mode, pid = front.pid, "sandbox Docker front serving");
                live.front = Some(front);
            }
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot start the sandbox's Docker front");
            }
        }
    }

    /// Starts `sandbox`'s front beside the holder of `record` and waits until it serves.
    async fn start_front(
        &self,
        sandbox: &str,
        record: &HolderRecord,
        serve: DockerServe,
        bridge_forwards: Vec<u16>,
    ) -> Result<DockerFrontRecord, String> {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        use tokio::io::AsyncBufReadExt as _;
        let paths = StatePaths::new(self.state_dir(sandbox));
        std::fs::create_dir_all(paths.docker_dir()).map_err(|e| format!("docker/: {e}"))?;
        // The sandbox's groups are prepared here, once per start: the engine's leaf must exist.
        let groups = self.cgroup_of(sandbox);
        let serve = match serve {
            DockerServe::Engine { .. } if groups.is_none() => DockerServe::Refuse {
                reason: DOCKER_NO_CGROUP.to_owned(),
            },
            serve => serve,
        };
        if matches!(serve, DockerServe::Engine { .. }) {
            // A holder from before engines has no storage yet (review round 1).
            let storage = self.sysroot(sandbox).join("docker");
            std::fs::create_dir_all(&storage).map_err(|e| format!("{}: {e}", storage.display()))?;
        }
        let _ =
            std::fs::set_permissions(paths.docker_dir(), std::fs::Permissions::from_mode(0o755));
        let reason = match &serve {
            DockerServe::Refuse { reason } => Some(reason.clone()),
            _ => None,
        };
        let mode = serve.kind();
        let spec = DockerFrontSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: sandbox.to_owned(),
            state_dir: paths.dir.to_string_lossy().into_owned(),
            holder_pid: record.inner_pid,
            user_ns: record.user_ns,
            net_ns: record.net_ns,
            mnt_ns: record.mnt_ns,
            cgroup: groups.map(|g| g.system.to_string_lossy().into_owned()),
            serve,
            bridge_forwards,
        };
        spec.validate().map_err(|e| e.to_string())?;
        write_json(&paths.docker_spec(), &spec).map_err(|e| e.to_string())?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(paths.docker_log())
            .map_err(|e| format!("docker-front.log: {e}"))?;
        // Detached like the holder: it outlives this server process.
        let mut child = tokio::process::Command::new(&self.config.exe)
            .args(["sandbox-host", "docker", "--spec"])
            .arg(paths.docker_spec())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .process_group(0)
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| format!("sandbox-host docker: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let line = tokio::time::timeout(
            FRONT_START_TIMEOUT,
            tokio::io::BufReader::new(stdout).lines().next_line(),
        )
        .await;
        let line = match line {
            Ok(Ok(Some(line))) => line,
            Ok(_) => String::from("it exited"),
            Err(_) => String::from("it did not report in time"),
        };
        if line.trim() != crate::docker_front::READY {
            let _ = child.kill().await;
            return Err(line.chars().take(2000).collect());
        }
        let pid = child.id().ok_or("the front exited")?;
        // Reaped while this server runs; it keeps running when the server stops.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let front = DockerFrontRecord {
            pid,
            mode: mode.to_owned(),
            reason,
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        };
        write_json(&paths.docker_record(), &front).map_err(|e| e.to_string())?;
        Ok(front)
    }

    /// Moves the Docker directory into `sandbox`'s runs that started without it (SPEC-196
    /// DKL-004): their mount namespaces are planned here, and only those needing a bind go to
    /// `yulius sandbox-host bind-docker`, at most [`BIND_TRIES`] times each.
    async fn bind_docker_dirs(&self, sandbox: &str, state: &mut SandboxState) {
        let Some(live) = state.live.as_mut() else {
            return;
        };
        if live.front.is_none() {
            return;
        }
        let (record, front) = (live.record.clone(), live.front_pid());
        // Without delegation there is no engine, so no container to leave out.
        let engine = self.group_paths(sandbox).map(|g| g.engine);
        let known = live.bound.clone();
        let planned = tokio::task::spawn_blocking(move || {
            crate::docker_bind::run_namespaces(
                record.net_ns,
                &[record.inner_pid, front],
                &[record.mnt_ns],
                engine.as_deref(),
            )
            .into_iter()
            .map(|(pid, mnt)| {
                // Done with or given up: no need to read its mounts again.
                let plan = if known.get(&mnt).is_some_and(|tries| *tries >= BIND_TRIES) {
                    crate::docker_bind::Plan::NotARun
                } else {
                    std::fs::read_to_string(format!("/proc/{pid}/mountinfo"))
                        .map_or(crate::docker_bind::Plan::NotARun, |t| {
                            crate::docker_bind::plan(&t)
                        })
                };
                (pid, mnt, plan)
            })
            .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let pids = bind_plan(&mut live.bound, &planned);
        if pids.is_empty() {
            return;
        }
        let paths = StatePaths::new(self.state_dir(sandbox));
        let mut command = tokio::process::Command::new(&self.config.exe);
        command
            .args(["sandbox-host", "bind-docker", "--spec"])
            .arg(paths.docker_spec());
        for (pid, _) in &pids {
            command.arg("--pid").arg(pid.to_string());
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = match command.spawn() {
            Ok(child) => tokio::time::timeout(BIND_TIMEOUT, child.wait_with_output()).await,
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot start bind-docker");
                return;
            }
        };
        let text = match output {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    tracing::warn!(sandbox, error = %String::from_utf8_lossy(&output.stderr).trim(), "bind-docker failed");
                }
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            _ => String::new(),
        };
        for line in text.lines() {
            if line.starts_with("bound") {
                tracing::info!(sandbox, %line, "gave a running run the sandbox's Docker directory");
            } else if line.starts_with("failed") {
                tracing::warn!(sandbox, %line, "cannot give a run the sandbox's Docker directory");
            }
        }
        bind_record(&mut live.bound, &pids, &text);
    }

    /// How the holder of `request` starts its Docker engine, in `cgroup` (SPEC-187).
    fn engine_spec(&self, request: &SandboxRequest, cgroup: &Path) -> EngineSpec {
        let state = StatePaths::new(self.state_dir(&request.id));
        let text = |path: &Path| path.to_string_lossy().into_owned();
        let view = crate::engine::EngineView {
            bwrap: text(&self.config.bwrap),
            exe: text(&self.config.exe),
            hostname: request.hostname.clone(),
            state_dir: text(&state.dir),
            system_root: text(&state.root()),
            data_dir: text(&self.config.data_dir),
            sandbox_dir: text(&request.dir),
            storage: text(&self.sysroot(&request.id).join("docker")),
            cgroup: text(cgroup),
        };
        let mut command = vec![
            text(&self.config.exe),
            "sandbox-exec".into(),
            "--engine".into(),
            "--cgroup".into(),
            text(cgroup),
            "--".into(),
        ];
        command.extend(crate::engine::bwrap_argv(&view));
        EngineSpec {
            command,
            cgroup: text(cgroup),
            idle_secs: self.config.idle.stop_after.as_secs().clamp(60, 86_400),
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
        if self.retired(&request.id) {
            return Err(PrepareError::Retired);
        }
        let Some(base) = self.bases.current() else {
            return Ok(None);
        };
        let entry = self.entry(&request.id);
        let mut state = entry.lock().await;
        state.request = Some(request.clone());
        if let Some(refusal) = self.quota_refusal(request, &mut state).await {
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
            [
                self.sysroot(&request.id).join("upper/etc").join(name),
                self.bases.path(&record.base_id).join("etc").join(name),
            ]
            .into_iter()
            .find(|p| p.is_file())
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

    /// Whether `sandbox`'s holder runs, ended while processes still use its namespaces (and its
    /// overlay mount), holds a lease, or is recorded alive in its state directory: its system
    /// layer and state are never deleted then (SPEC-215 LFT-002). A record that cannot be read
    /// counts as not alive.
    pub async fn holder_live(&self, sandbox: &str) -> bool {
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
            read_json::<HolderRecord>(&record).is_ok_and(|record| alive(&record))
        })
        .await
        .unwrap_or(true)
    }

    /// Removes `path`, under `sysroots/`, `runtime/sandboxes/` or `system/` of the data
    /// directory, as root of a mapped namespace where the service user cannot (its files belong
    /// to a sandbox's mapped ids; SPEC-215).
    ///
    /// # Errors
    /// `path` lies elsewhere, or the removal failed.
    pub async fn remove_owned_tree(&self, path: &Path) -> Result<(), String> {
        let data = &self.config.data_dir;
        let allowed = [
            data.join("sysroots"),
            data.join("runtime").join("sandboxes"),
            data.join("system"),
        ]
        .iter()
        .any(|root| path.starts_with(root) && path != root.as_path())
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
        tokio::task::spawn_blocking(move || crate::base::remove_tree(&mapper, &exe, &path))
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
        let dir = self.state_dir(sandbox);
        let marked = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(RETIRED), b"")
        })
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)));
        match marked {
            Ok(()) => tracing::info!(
                sandbox,
                "sandbox retired: its holder stops once its runs end"
            ),
            Err(error) => tracing::warn!(sandbox, %error, "the sandbox could not be retired"),
        }
    }

    /// Whether `sandbox` was retired ([`Manager::retire`]).
    fn retired(&self, sandbox: &str) -> bool {
        self.state_dir(sandbox).join(RETIRED).exists()
    }

    /// How long ago `sandbox` was retired; `None` when it was not.
    fn retired_for(&self, sandbox: &str) -> Option<Duration> {
        let modified = std::fs::metadata(self.state_dir(sandbox).join(RETIRED))
            .and_then(|meta| meta.modified())
            .ok()?;
        Some(modified.elapsed().unwrap_or_default())
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
            forwards: self.config.forwards.keys().copied().collect(),
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
            docker = record.engine,
            "sandbox holder started"
        );
        Ok(record)
    }

    fn start_relays(&self, sandbox: &str, record: &HolderRecord) -> Vec<JoinHandle<()>> {
        let state = StatePaths::new(self.state_dir(sandbox));
        let mut tasks = Vec::new();
        let _ = std::fs::remove_file(state.outbound());
        match tokio::net::UnixListener::bind(state.outbound()) {
            Ok(listener) => {
                let targets = Arc::new(self.config.forwards.clone());
                tasks.push(tokio::spawn(crate::relay::serve_outbound(
                    listener, targets,
                )));
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
        let state = StatePaths::new(self.state_dir(sandbox));
        let record = record.clone();
        let _ = tokio::task::spawn_blocking(move || crate::holder::stop(&state, &record)).await;
        tracing::info!(sandbox, "sandbox holder stopped");
    }

    /// Adopts holders a previous server process started; removes the records of dead ones.
    pub async fn adopt(&self) -> usize {
        let root = self.config.data_dir.join("runtime").join("sandboxes");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        let mut adopted = 0;
        for entry in entries.flatten().take(10_000) {
            let sandbox = entry.file_name().to_string_lossy().into_owned();
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

    /// The Docker front `sandbox`'s live holder has: its pid, mode and refusal reason.
    pub async fn docker_front(&self, sandbox: &str) -> Option<DockerFrontRecord> {
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        state.live.as_ref()?.front.clone()
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
        }
        self.maybe_build(in_use);
    }

    /// Prunes `sandbox`'s Docker engine when it runs, through its own control socket: build cache
    /// and unused images (SPEC-187 DKR-006). The bytes credited are the engine's figure, at most
    /// the engine storage last measured (0 when unmeasured or not among the largest consumers, so
    /// the other caches are pruned as before): the engine controls its answer. Never through the sandbox's socket, which would
    /// start a stopped engine and reset its idle time, keeping it and the holder up for as long
    /// as the disk is under pressure. `None` without a live holder serving one, with the engine
    /// stopped, or when it could not prune.
    pub async fn prune_engine(&self, sandbox: &str) -> Option<u64> {
        let (socket, measured) = {
            let entry = self.entry(sandbox);
            let state = entry.lock().await;
            if !state.live.as_ref()?.serves_engine() {
                return None;
            }
            let measured = state.usage.as_ref().map_or(0, |(usage, _)| {
                usage
                    .largest
                    .iter()
                    .find(|(name, _)| name == "containers")
                    .map_or(0, |(_, bytes)| *bytes)
            });
            (
                StatePaths::new(self.state_dir(sandbox))
                    .engine_control()
                    .join(yulius_contracts::sandbox::ENGINE_SOCKET),
                measured,
            )
        };
        match crate::engine::prune(&socket).await {
            Ok(reported) => {
                let bytes = reported.min(measured);
                tracing::info!(sandbox, freed = %human(bytes), "pruned the sandbox's Docker engine");
                Some(bytes)
            }
            Err(error) => {
                tracing::warn!(sandbox, %error, "cannot prune the sandbox's Docker engine");
                None
            }
        }
    }

    /// The refusal when the sandbox is over its quota. A reading older than [`QUOTA_RECHECK`]
    /// that says "over" is replaced by a fresh measurement first (space may have been freed).
    async fn quota_refusal(
        &self,
        request: &SandboxRequest,
        state: &mut SandboxState,
    ) -> Option<PrepareError> {
        let limit = self.config.disk_max?;
        let (usage, at) = state.usage.as_ref()?;
        if usage.bytes <= limit {
            return None;
        }
        if at.elapsed() >= QUOTA_RECHECK {
            let (dir, sysroot) = (request.dir.clone(), self.sysroot(&request.id));
            let fresh = tokio::task::spawn_blocking(move || measure_sandbox(&dir, &sysroot))
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
        if self.config.disk_max.is_none() || state.measuring {
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
        tokio::spawn(async move {
            let usage =
                tokio::task::spawn_blocking(move || measure_sandbox(&request.dir, &sysroot))
                    .await
                    .unwrap_or_default();
            let entry = manager.entry(&sandbox);
            let mut state = entry.lock().await;
            state.measuring = false;
            state.usage = Some((usage, Instant::now()));
        });
    }

    fn maybe_build(self: &Arc<Self>, in_use: Vec<String>) {
        if !self.config.auto_build || self.building.load(Ordering::SeqCst) {
            return;
        }
        let current = self.bases.current();
        let base_lacks_dockerd = current.as_ref().is_some_and(|base| {
            crate::docker_status::has_dockerd(Path::new("/"))
                && !crate::docker_status::has_dockerd(&self.bases.path(&base.id))
        });
        let fingerprint = {
            let mut build = lock(&self.build);
            let due = fingerprint_due(
                build.checked.map(|at| at.elapsed()),
                current.is_some(),
                base_lacks_dockerd,
            );
            if !due {
                return;
            }
            build.checked = Some(Instant::now());
            crate::base::fingerprint(Path::new("/"), &self.config.sudo_shim)
        };
        let wanted = match &current {
            None => true,
            Some(base) if base.fingerprint == fingerprint => {
                let mut build = lock(&self.build);
                build.pending = None;
                build.pending_at_ms = None;
                false
            }
            Some(_) => {
                let mut build = lock(&self.build);
                match &build.pending {
                    Some((seen, since)) if *seen == fingerprint => {
                        since.elapsed() >= REBUILD_DEBOUNCE
                    }
                    _ => {
                        build.pending = Some((fingerprint, Instant::now()));
                        build.pending_at_ms = Some(epoch_ms());
                        // Check again after the debounce instead of waiting a full interval.
                        build.checked =
                            Instant::now().checked_sub(FINGERPRINT_EVERY - REBUILD_DEBOUNCE);
                        false
                    }
                }
            }
        };
        if wanted {
            self.spawn_build(in_use);
        }
    }

    /// Starts a base build as a child process (when none runs).
    pub fn spawn_build(self: &Arc<Self>, in_use: Vec<String>) {
        if self.building.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.build).building_since_ms = Some(epoch_ms());
        let started = Instant::now();
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!("building the sandbox base (a copy of the host system)");
            let mut command = tokio::process::Command::new(&manager.config.exe);
            command
                .args(["sandbox-base", "build", "--data-dir"])
                .arg(&manager.config.data_dir);
            if !in_use.is_empty() {
                command.arg("--keep").arg(in_use.join(","));
            }
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .process_group(0)
                .kill_on_drop(true);
            let result = match command.spawn() {
                Ok(child) => {
                    match tokio::time::timeout(BUILD_TIMEOUT, child.wait_with_output()).await {
                        Ok(Ok(output)) if output.status.success() => Ok(()),
                        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr)
                            .trim()
                            .chars()
                            .take(2000)
                            .collect()),
                        Ok(Err(error)) => Err(error.to_string()),
                        Err(_) => Err("the base build timed out".to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            match &result {
                Ok(()) => {
                    tracing::info!(base = ?manager.bases.current().map(|b| b.id), "sandbox base ready")
                }
                Err(error) => tracing::warn!(%error, "sandbox base build failed"),
            }
            let mut build = lock(&manager.build);
            if result.is_ok() {
                build.last_build = Some(started.elapsed());
            }
            build.last_error = result.err();
            build.pending = None;
            build.pending_at_ms = None;
            build.building_since_ms = None;
            drop(build);
            manager.building.store(false, Ordering::SeqCst);
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
        let limit = self.config.disk_max?;
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
        let limit = self.config.disk_max?;
        let (dir, sysroot) = (request.dir.clone(), self.sysroot(&request.id));
        let usage = tokio::task::spawn_blocking(move || measure_sandbox(&dir, &sysroot))
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

/// What [`Manager::keep_front`] does with a sandbox's front.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FrontAction {
    /// It serves as it should.
    Keep,
    /// None runs: start one.
    Start(DockerServe),
    /// One runs in another mode: stop it, then start this one.
    Replace(DockerServe),
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

/// The front decision (SPEC-196 DKL-003) from the current front (its record, whether it still
/// runs) and the wanted mode. The wanted mode needs no cgroup write, so a failed one never cuts
/// a running engine.
fn front_action(current: Option<(&DockerFrontRecord, bool)>, wanted: DockerServe) -> FrontAction {
    let running = current
        .filter(|(_, running)| *running)
        .map(|(front, _)| front);
    let reason = match &wanted {
        DockerServe::Refuse { reason } => Some(reason),
        _ => None,
    };
    match running {
        Some(front) if front.mode == wanted.kind() && front.reason.as_ref() == reason => {
            FrontAction::Keep
        }
        // `dockerd` gone from the sandbox's system for a moment (a package swap) never cuts a
        // running engine and its containers (SPEC-199); a new holder decides again.
        Some(front) if front.mode == "engine" && reason.is_some_and(|r| r == DOCKER_NO_DOCKERD) => {
            FrontAction::Keep
        }
        Some(_) => FrontAction::Replace(wanted),
        None => FrontAction::Start(wanted),
    }
}

/// Which runs `bind-docker` should get now (SPEC-196 DKL-004), from `found` (one pid per mount
/// namespace, with its [`Plan`](crate::docker_bind::Plan)), updating `known`: namespaces that
/// ended are forgotten (their inode numbers are reused), those needing nothing are done, and one
/// is given up after [`BIND_TRIES`] attempts.
fn bind_plan(
    known: &mut HashMap<u64, u8>,
    found: &[(u32, u64, crate::docker_bind::Plan)],
) -> Vec<(u32, u64)> {
    known.retain(|mnt, _| found.iter().any(|(_, m, _)| m == mnt));
    let mut pids = Vec::new();
    for (pid, mnt, plan) in found {
        if known.get(mnt).is_some_and(|tries| *tries >= BIND_TRIES) {
            continue;
        }
        if *plan == crate::docker_bind::Plan::Bind {
            pids.push((*pid, *mnt));
        } else {
            known.insert(*mnt, BIND_DONE);
        }
    }
    pids
}

/// Records the helper's `output` for the runs it was given: bound or skipped ones are done,
/// others (failed, or missing from the output) took one more attempt.
fn bind_record(known: &mut HashMap<u64, u8>, tried: &[(u32, u64)], output: &str) {
    let outcomes: HashMap<u32, bool> = output
        .lines()
        .filter_map(crate::docker_bind::Outcome::parse)
        .collect();
    for (pid, mnt) in tried {
        let tries = known.entry(*mnt).or_insert(0);
        if outcomes.get(pid) == Some(&true) {
            *tries = BIND_DONE;
        } else {
            *tries = tries.saturating_add(1);
        }
    }
}

/// A sandbox's usage: its directory, split by consumer, its system layer (`sysroot/upper`) and
/// its Docker engine's storage (`sysroot/docker`, SPEC-187 DKR-006) as one each.
fn measure_sandbox(dir: &Path, sysroot: &Path) -> Usage {
    crate::usage::measure(&[
        (dir, None),
        (&sysroot.join("upper"), Some("system")),
        (&sysroot.join("docker"), Some("containers")),
    ])
}

#[cfg(test)]
mod tests {
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

    fn live_record(engine: bool) -> HolderRecord {
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
            engine,
        }
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

    /// A manager with Docker possible (or not, `docker`) whose executable is [`FAKE_FRONT`],
    /// and an adopted "holder" that is this test process (its namespaces are alive). Never
    /// stop that holder: it is this process.
    fn adopting(
        data: &Path,
        docker: Result<(), String>,
        engine: bool,
    ) -> Result<Arc<Manager>, Box<dyn std::error::Error>> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let exe = data.join("fake-yulius");
        if !exe.exists() {
            std::fs::write(&exe, FAKE_FRONT)?;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
        }
        let mut config = config(data, 1 << 40);
        config.exe = exe.clone();
        config.uid.host = std::fs::metadata(&exe)?.uid().wrapping_add(1);
        config.docker = docker;
        config.delegation = Some(delegation(data)?);
        let manager = Manager::new(config);
        let state = StatePaths::new(manager.state_dir("s1"));
        std::fs::create_dir_all(&state.dir)?;
        let dockerd = manager.bases.path("base-1").join("usr/bin");
        std::fs::create_dir_all(&dockerd)?;
        std::fs::write(dockerd.join("dockerd"), "")?;
        let me = std::process::id();
        let ns = |kind| crate::holder::ns_inode(me, kind).ok_or("no namespace");
        let record = HolderRecord {
            inner_pid: me,
            bwrap_pid: 0,
            user_ns: ns("user")?,
            net_ns: ns("net")?,
            mnt_ns: ns("mnt")?,
            boot_id: crate::holder::boot_id(),
            ..live_record(engine)
        };
        write_json(&state.record(), &record)?;
        Ok(manager)
    }

    async fn ask_front(manager: &Manager) -> std::io::Result<String> {
        use tokio::io::AsyncReadExt as _;
        let socket = StatePaths::new(manager.state_dir("s1")).docker_socket();
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
    /// even for a front from an older binary, which no update replaces. Review round 1 (major):
    /// also for a SPEC-187 holder, whose own engine uses the same leaf behind a relay front.
    #[tokio::test]
    async fn a_kept_engine_front_gets_its_pruned_leaf_back()
    -> Result<(), Box<dyn std::error::Error>> {
        for (holder_engine, mode) in [(false, "engine"), (true, "relay")] {
            kept_front_restores_the_leaf(holder_engine, mode).await?;
        }
        Ok(())
    }

    async fn kept_front_restores_the_leaf(
        holder_engine: bool,
        mode: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = adopting(data.path(), Ok(()), holder_engine)?;
        assert_eq!(manager.adopt().await, 1);
        let front = front_of(&manager).await.ok_or("no front")?;
        assert_eq!(front.mode, mode);
        let leaf = manager.group_paths("s1").ok_or("no delegation")?.engine;
        assert!(leaf.is_dir());
        std::fs::remove_dir(&leaf)?;
        {
            let entry = manager.entry("s1");
            let mut guard = entry.lock().await;
            manager.keep_front("s1", &mut guard).await;
        }
        assert_eq!(
            front_of(&manager).await.map(|f| f.pid),
            Some(front.pid),
            "{mode}: the running front is kept"
        );
        assert!(leaf.is_dir(), "{mode}: the engine's leaf is back");
        kill(front.pid);
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
        let manager = adopting(data.path(), Ok(()), false)?;
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
        let off = adopting(data.path(), Err(DOCKER_OFF.into()), false)?;
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
        let on = adopting(data.path(), Ok(()), false)?;
        assert_eq!(on.adopt().await, 1);
        let back = front_of(&on).await.ok_or("no front")?;
        assert_eq!(back.mode, "engine");
        assert_eq!(ask_front(&on).await?, "mode:engine:");
        // An unchanged front is kept across server restarts.
        let same = adopting(data.path(), Ok(()), false)?;
        assert_eq!(same.adopt().await, 1);
        assert_eq!(front_of(&same).await.map(|f| f.pid), Some(back.pid));
        kill(back.pid);
        Ok(())
    }

    /// SPEC-196 DKL-002/005: a SPEC-187 holder serving its own engine gets a relay to it; a
    /// missing prerequisite a refusal that names it; and runs see the Docker directory.
    #[tokio::test]
    async fn the_front_mode_follows_the_holder_and_the_prerequisites()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let request = request(data.path());
        let on = adopting(data.path(), Ok(()), true)?;
        assert!(matches!(
            on.front_mode(&request, &live_record(true)),
            DockerServe::Relay { socket } if socket.ends_with("runtime/sandboxes/s1/docker.sock")
        ));
        assert!(matches!(
            on.front_mode(&request, &live_record(false)),
            DockerServe::Engine { .. }
        ));
        // Review round 1 (low): a system without `dockerd` (SPEC-199 DSY-003: the agent can
        // install it in the sandbox, or an operator on the host for new bases).
        let mut older = live_record(false);
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
        let off = adopting(data.path(), Err(DOCKER_OFF.into()), true)?;
        assert_eq!(
            off.front_mode(&request, &live_record(true)),
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
        let manager = adopting(data.path(), Ok(()), false)?;
        let mode = |base: &str| {
            let mut record = live_record(false);
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
        let manager = adopting(data.path(), Ok(()), false)?;
        let mut request = request(data.path());
        request.id = "s2".into();
        let serve = manager.front_mode(&request, &live_record(false));
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
            front_action(Some((&running, true)), engine()),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(Some((&running, false)), engine()),
            FrontAction::Start(engine())
        );
        assert_eq!(front_action(None, engine()), FrontAction::Start(engine()));
        assert_eq!(
            front_action(Some((&running, true)), refuse("off")),
            FrontAction::Replace(refuse("off"))
        );
        let refusing = front("refuse", Some("off"));
        assert_eq!(
            front_action(Some((&refusing, true)), refuse("off")),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(Some((&refusing, true)), refuse("other")),
            FrontAction::Replace(refuse("other"))
        );
        assert_eq!(
            front_action(Some((&refusing, true)), engine()),
            FrontAction::Replace(engine())
        );
        // SPEC-199 review round 1 (nit): `dockerd` gone from the sandbox's system for a moment
        // (a docker.io → docker-ce swap) never cuts a running engine and its containers.
        assert_eq!(
            front_action(Some((&running, true)), refuse(DOCKER_NO_DOCKERD)),
            FrontAction::Keep
        );
        assert_eq!(
            front_action(None, refuse(DOCKER_NO_DOCKERD)),
            FrontAction::Start(refuse(DOCKER_NO_DOCKERD))
        );
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
        entry.lock().await.live = Some(Live::new(live_record(false), Vec::new()));
        assert!(
            manager.docker_for("s1").await,
            "a holder from before engines gains one"
        );
        entry.lock().await.live = Some(Live::new(live_record(true), Vec::new()));
        assert!(manager.docker_for("s1").await);
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

    /// SPEC-187 DKR-006, review round 2 (medium): pruning reaches only an engine that already
    /// runs, through its own control socket: never through the sandbox's socket, which would
    /// start a stopped engine and keep it (and the holder) from idling; never through a link.
    #[tokio::test]
    async fn only_a_running_engine_is_pruned_through_its_control_socket()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1);
        assert_eq!(manager.prune_engine("s1").await, None, "no holder");
        let state = StatePaths::new(manager.state_dir("s1"));
        std::fs::create_dir_all(state.engine_control())?;
        let (front, front_hits) = fake_api(tokio::net::UnixListener::bind(state.engine_socket())?);
        let entry = manager.entry("s1");
        entry.lock().await.live = Some(Live::new(live_record(false), Vec::new()));
        assert_eq!(
            manager.prune_engine("s1").await,
            None,
            "a holder without an engine"
        );
        entry.lock().await.live = Some(Live::new(live_record(true), Vec::new()));
        assert_eq!(manager.prune_engine("s1").await, None, "a stopped engine");
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
        let usage = measure_sandbox(&dir, &sysroot);
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
                "--",
            ]
        );
        assert_eq!(spec.command[6], "/usr/bin/bwrap");
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
        let manager = adopting(data, Err(DOCKER_OFF.into()), false)?;
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

    /// SPEC-215: only a system tree of the data directory is removed through the mapped
    /// namespace; a tree the service user owns goes without it.
    #[tokio::test]
    async fn only_a_sandbox_s_system_tree_is_removed() -> Result<(), Box<dyn std::error::Error>> {
        let data = tempfile::tempdir()?;
        let manager = manager(data.path(), 1 << 30);
        let d = data.path();
        for refused in [
            d.join("sandboxes/s1"),
            d.join("sysroots"),
            d.join("sysroots/../sandboxes/s1"),
            PathBuf::from("/tmp"),
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
}
