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
//!   - stops holders idle for [`IdleConfig::stop_after`];
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
    EngineSpec, HOLDER_RECORD_VERSION, HolderRecord, HolderSpec, IdMap,
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
    /// Give each sandbox a Docker engine (ADR-0151); it also needs the cgroup delegation.
    pub docker: bool,
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
}

struct Live {
    record: HolderRecord,
    relays: Vec<JoinHandle<()>>,
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
    checked: Option<Instant>,
    last_error: Option<String>,
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

    /// Whether sandboxes get a Docker engine: the setting, the cgroup delegation, and a `yulius`
    /// executable the sandbox's root neither owns nor may write through its group or as anyone
    /// (the engine's read-only binds are not locked, so an engine could remount and rewrite it;
    /// ADR-0151).
    #[must_use]
    pub fn docker(&self) -> bool {
        use std::os::unix::fs::MetadataExt as _;
        // Owned by the sandbox's root, or writable by its group (the service group) or anyone.
        let exe_writable = std::fs::metadata(&self.config.exe)
            .is_ok_and(|meta| meta.uid() == self.config.uid.host || meta.mode() & 0o022 != 0);
        self.config.docker && self.config.delegation.is_some() && !exe_writable
    }

    /// Whether `sandbox`'s runs reach a Docker engine: its live holder serves one, or, without a
    /// live holder, the next one will (SPEC-187 DKR-010). A holder from before engines, or one
    /// whose socket failed, serves none.
    pub async fn docker_for(&self, sandbox: &str) -> bool {
        if !self.docker() {
            return false;
        }
        let entry = self.entry(sandbox);
        let state = entry.lock().await;
        state.live.as_ref().is_none_or(|live| live.record.engine)
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
                let runs = tokio::task::spawn_blocking(move || Self::runs_in(stale.net_ns, 0))
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
            state.live = Some(Live { record, relays });
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
                engine_socket: record.engine.then(|| {
                    StatePaths::new(self.state_dir(&request.id))
                        .engine_socket()
                        .to_string_lossy()
                        .into_owned()
                }),
            },
        }))
    }

    /// Processes running in `sandbox`'s namespaces besides its holder, as `/proc` shows them (0
    /// without a live holder). Yulius's own records of runs, shells and the browser are checked
    /// beside it.
    pub async fn processes(&self, sandbox: &str) -> usize {
        let record = {
            let entry = self.entry(sandbox);
            let state = entry.lock().await;
            match &state.live {
                Some(live) => live.record.clone(),
                None => return 0,
            }
        };
        tokio::task::spawn_blocking(move || Self::runs_in(record.net_ns, record.inner_pid))
            .await
            .unwrap_or(1)
    }

    /// Ends a lease taken by [`Manager::prepare`].
    pub async fn release(&self, sandbox: &str) {
        let entry = self.entry(sandbox);
        let mut state = entry.lock().await;
        state.leases = state.leases.saturating_sub(1);
        state.last_used = Some(Instant::now());
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
        let engine = groups
            .as_ref()
            .filter(|_| self.docker())
            .map(|groups| self.engine_spec(request, &groups.engine));
        for dir in [
            state.dir.clone(),
            state.root(),
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
            engine,
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
            let state = StatePaths::new(entry.path());
            let Ok(record) = read_json::<HolderRecord>(&state.record()) else {
                continue;
            };
            let slot = self.entry(&sandbox);
            let mut guard = slot.lock().await;
            if alive(&record) {
                let relays = self.start_relays(&sandbox, &record);
                guard.live = Some(Live { record, relays });
                guard.last_used = Some(Instant::now());
                adopted += 1;
            } else {
                self.stop_holder(&sandbox, &record).await;
                guard.stale = Some(record);
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

    /// Processes in `net_ns` other than `holder` (the sandbox's runs).
    fn runs_in(net_ns: u64, holder: u32) -> usize {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        entries
            .flatten()
            .take(1 << 20)
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
            .filter(|pid| *pid != holder)
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
                    let runs = tokio::task::spawn_blocking(move || {
                        Self::runs_in(record.net_ns, record.inner_pid)
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
                    let stop = runs == 0
                        && state.leases == 0
                        && (idle_for >= self.config.idle.stop_after
                            || (superseded && idle_for >= Duration::from_secs(60)));
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
            if !state.live.as_ref()?.record.engine {
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
        let fingerprint = {
            let mut build = lock(&self.build);
            let due = build
                .checked
                .is_none_or(|at| at.elapsed() >= FINGERPRINT_EVERY)
                || current.is_none();
            if !due {
                return;
            }
            build.checked = Some(Instant::now());
            crate::base::fingerprint(Path::new("/"), &self.config.sudo_shim)
        };
        let wanted = match &current {
            None => true,
            Some(base) if base.fingerprint == fingerprint => {
                lock(&self.build).pending = None;
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
            build.last_error = result.err();
            build.pending = None;
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
            docker: false,
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
            docker: false,
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
        on.docker = true;
        on.delegation = Some(delegation(data.path())?);
        let manager = Manager::new(on);
        assert!(manager.docker(), "the setting with the delegation");
        assert!(manager.docker_for("s1").await, "its next holder serves one");
        let entry = manager.entry("s1");
        entry.lock().await.live = Some(Live {
            record: live_record(false),
            relays: Vec::new(),
        });
        assert!(
            manager.docker_for("s1").await,
            "a holder from before engines gains one"
        );
        entry.lock().await.live = Some(Live {
            record: live_record(true),
            relays: Vec::new(),
        });
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
        owned.docker = true;
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
        entry.lock().await.live = Some(Live {
            record: live_record(false),
            relays: Vec::new(),
        });
        assert_eq!(
            manager.prune_engine("s1").await,
            None,
            "a holder without an engine"
        );
        entry.lock().await.live = Some(Live {
            record: live_record(true),
            relays: Vec::new(),
        });
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
}
