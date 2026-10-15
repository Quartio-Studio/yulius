//! Sandbox isolation (SPEC-002 ADR-22): settings, prerequisite checks and the
//! [`SandboxIsolation`] port over `yulius-sandbox`'s [`Manager`].

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clap::Args;
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::isolation::{
    DiskUsage, SandboxIsolation, SandboxJoin, SandboxLimits, SandboxRef,
};
use yulius_domain::ids::SandboxId;
use yulius_sandbox::cgroup::Limits;
use yulius_sandbox::manager::{
    IdleConfig, IsolationConfig, Manager, PrepareError, SandboxRequest, TICK,
};

/// Which containment contained runs use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum IsolationMode {
    /// The namespace holder when its prerequisites hold, else the previous containment.
    Auto,
    /// Always the namespace holder; runs are refused while it is unavailable.
    Namespaces,
    /// The previous containment (ADR-03/ADR-12).
    Legacy,
}

/// Egress of sandboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Egress {
    /// Direct egress through `slirp4netns` (the host's loopback excluded).
    Direct,
    /// Only the forwarded endpoints (the internal listener, which also serves the secrets
    /// proxy, and `--sandbox-forward` ports).
    None,
}

/// Isolation settings of `yulius serve` (environment variables in `docs/operations.md`).
#[derive(Debug, Clone, Args)]
pub struct IsolationArgs {
    /// Sandbox isolation (SPEC-002 ADR-22): `auto`, `namespaces` or `legacy`.
    #[arg(long = "isolation", env = "YULIUS_ISOLATION", value_enum, default_value_t = IsolationMode::Auto)]
    pub mode: IsolationMode,
    /// Sandbox egress: `direct` or `none`.
    #[arg(long = "sandbox-egress", env = "YULIUS_SANDBOX_EGRESS", value_enum, default_value_t = Egress::Direct)]
    pub egress: Egress,
    /// Memory limit per sandbox in GiB (default: the host's memory but an eighth, at least 4 GiB).
    #[arg(long = "sandbox-memory-gb", env = "YULIUS_SANDBOX_MEMORY_GB")]
    pub memory_gb: Option<u64>,
    /// CPU limit per sandbox in CPUs (`cpu.max`; default: none, the CPU is shared by weight).
    #[arg(long = "sandbox-cpus", env = "YULIUS_SANDBOX_CPUS")]
    pub cpus: Option<u32>,
    /// Task (process and thread) limit per sandbox.
    #[arg(
        long = "sandbox-pids",
        env = "YULIUS_SANDBOX_PIDS",
        default_value_t = 8192
    )]
    pub pids: u64,
    /// Soft disk quota per sandbox in GB (sandbox directory plus installed packages); 0: none.
    #[arg(
        long = "sandbox-disk-gb",
        env = "YULIUS_SANDBOX_DISK_GB",
        default_value_t = 100
    )]
    pub disk_gb: u64,
    /// Extra host loopback ports reachable from inside every sandbox at the same port
    /// (comma-separated), for example a loopback MCP server.
    #[arg(
        long = "sandbox-forward",
        env = "YULIUS_SANDBOX_FORWARD",
        value_delimiter = ','
    )]
    pub forward: Vec<u16>,
    /// Minutes a sandbox's holder stays without runs before it is stopped.
    #[arg(
        long = "sandbox-idle-minutes",
        env = "YULIUS_SANDBOX_IDLE_MINUTES",
        default_value_t = 5
    )]
    pub idle_minutes: u64,
}

impl Default for IsolationArgs {
    fn default() -> Self {
        Self {
            mode: IsolationMode::Auto,
            egress: Egress::Direct,
            memory_gb: None,
            cpus: None,
            pids: 8192,
            disk_gb: 100,
            forward: Vec::new(),
            idle_minutes: 5,
        }
    }
}

/// An executable on `PATH` (or an absolute path that exists).
#[must_use]
pub fn which(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .chain(["/usr/bin".into(), "/usr/local/bin".into(), "/bin".into()])
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The directory of the Claude Code npm package `claude` resolves to, bound read-only over the
/// system layer so sandboxes run the host's current CLI.
#[must_use]
pub fn agent_cli_dir() -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(which(yulius_harness::launch::CLAUDE_PROGRAM)?).ok()?;
    resolved
        .ancestors()
        .find(|dir| dir.ends_with("node_modules/@anthropic-ai/claude-code"))
        .map(Path::to_path_buf)
}

/// The prerequisites of the namespace holder on this host: the configuration, or what is
/// missing.
///
/// # Errors
/// Every missing prerequisite, one message each.
pub fn resolve(
    args: &IsolationArgs,
    data_dir: &Path,
    exe: &Path,
    internal_port: u16,
) -> Result<IsolationConfig, Vec<String>> {
    let mut missing = Vec::new();
    if !cfg!(target_os = "linux") {
        missing.push("sandbox namespaces need Linux".to_owned());
    }
    let mut program = |name: &str| {
        let found = which(name);
        if found.is_none() {
            missing.push(format!("`{name}` is not installed"));
        }
        found.unwrap_or_else(|| PathBuf::from(name))
    };
    let bwrap = program("bwrap");
    let newuidmap = program("newuidmap");
    let newgidmap = program("newgidmap");
    let slirp4netns = match args.egress {
        Egress::Direct => Some(program("slirp4netns")),
        Egress::None => None,
    };
    let maps = yulius_sandbox::subid::current_maps();
    let (uid, gid) = match maps {
        Ok(maps) => maps,
        Err(message) => {
            missing.push(message);
            let fallback = yulius_contracts::sandbox::IdMap {
                host: 0,
                sub_start: 1,
                count: 1,
            };
            (fallback, fallback)
        }
    };
    if !missing.is_empty() {
        return Err(missing);
    }
    let total = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| yulius_sandbox::cgroup::mem_total(&t))
        .unwrap_or(8 << 30);
    let defaults = Limits::defaults(total);
    let limits = Limits {
        memory_max: args.memory_gb.map(|g| g << 30).or(defaults.memory_max),
        cpus: args.cpus.or(defaults.cpus),
        pids_max: Some(args.pids),
    };
    let loopback = |port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut forwards = BTreeMap::from([(internal_port, loopback(internal_port))]);
    for port in &args.forward {
        forwards.insert(*port, loopback(*port));
    }
    Ok(IsolationConfig {
        data_dir: data_dir.to_path_buf(),
        exe: exe.to_path_buf(),
        bwrap,
        newuidmap,
        newgidmap,
        slirp4netns,
        uid,
        gid,
        delegation: None,
        limits,
        disk_max: (args.disk_gb > 0).then(|| args.disk_gb.saturating_mul(1_000_000_000)),
        forwards,
        sudo_shim: yulius_app::identity::SUDO_SHIM.to_owned(),
        idle: IdleConfig {
            stop_after: Duration::from_secs(args.idle_minutes.max(1) * 60),
        },
        auto_build: true,
    })
}

/// The delegated cgroup unit of this process, without changing anything.
///
/// # Errors
/// Why there is none.
pub fn delegation_detect() -> Result<String, String> {
    let own = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let own = yulius_sandbox::cgroup::own_path(&own).ok_or("no cgroup v2 hierarchy")?;
    yulius_sandbox::cgroup::detect(Path::new(yulius_sandbox::cgroup::CGROUP_ROOT), &own)
        .map(|unit| unit.display().to_string())
}

/// The delegated cgroup of this server, prepared for sandboxes, or why there is none.
///
/// # Errors
/// Why limits do not apply.
pub fn delegation() -> Result<yulius_sandbox::cgroup::Delegation, String> {
    let own = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let own = yulius_sandbox::cgroup::own_path(&own).ok_or("no cgroup v2 hierarchy")?;
    let unit =
        yulius_sandbox::cgroup::detect(Path::new(yulius_sandbox::cgroup::CGROUP_ROOT), &own)?;
    yulius_sandbox::cgroup::prepare(&unit).map_err(|e| format!("{}: {e}", unit.display()))
}

fn request_of(sandbox: &SandboxRef) -> SandboxRequest {
    SandboxRequest {
        id: sandbox.id.as_str().to_owned(),
        hostname: sandbox.hostname.clone(),
        dir: sandbox.dir.clone(),
        port_first: sandbox.port_first,
        port_last: sandbox.port_last,
    }
}

/// [`SandboxIsolation`] over the manager.
pub struct NamespaceIsolation {
    manager: Arc<Manager>,
    strict: bool,
    extra_readonly: Vec<PathBuf>,
}

impl NamespaceIsolation {
    /// The port over `manager`; `strict` refuses runs while no base exists.
    #[must_use]
    pub fn new(manager: Arc<Manager>, strict: bool) -> Self {
        Self {
            manager,
            strict,
            extra_readonly: agent_cli_dir().into_iter().collect(),
        }
    }
}

#[async_trait]
impl SandboxIsolation for NamespaceIsolation {
    async fn prepare(&self, sandbox: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        match self.manager.prepare(&request_of(sandbox)).await {
            Ok(Some(joined)) => {
                let join = joined.join;
                Ok(Some(SandboxJoin {
                    holder_pid: join.holder_pid,
                    user_ns: join.user_ns,
                    net_ns: join.net_ns,
                    mnt_ns: join.mnt_ns,
                    cgroup: join.cgroup.map(PathBuf::from),
                    ci_cgroup: joined.ci_cgroup,
                    system_root: PathBuf::from(join.system_root),
                    hostname: join.hostname,
                    exec_shim: join.exec_shim.map(PathBuf::from),
                    extra_readonly: self.extra_readonly.clone(),
                    private_dirs: join.private_dirs.iter().map(PathBuf::from).collect(),
                    system_passwd: joined.system_passwd,
                    system_group: joined.system_group,
                    host_system: None,
                }))
            }
            Ok(None) if self.strict => Err(AppError::Conflict(
                "sandbox isolation is still being prepared (the base system is being built); \
                 try again in a few minutes"
                    .into(),
            )),
            Ok(None) => Ok(None),
            Err(error @ PrepareError::OverQuota { .. }) => {
                Err(AppError::Conflict(error.to_string()))
            }
            Err(error @ PrepareError::Holder(_)) if self.strict => {
                Err(AppError::Upstream(error.to_string()))
            }
            Err(error @ PrepareError::Holder(_)) => {
                // `auto` never blocks work: this run uses the previous containment.
                tracing::warn!(sandbox = %sandbox.id, %error, "sandbox holder unavailable; this run uses the previous containment");
                Ok(None)
            }
        }
    }

    async fn disk(&self, sandbox: &SandboxRef) -> Option<DiskUsage> {
        let (used, limit) = self.manager.disk(sandbox.id.as_str()).await?;
        Some(DiskUsage { used, limit })
    }

    async fn measure_disk(&self, sandbox: &SandboxRef) -> Option<DiskUsage> {
        let (used, limit) = self.manager.measure_disk(&request_of(sandbox)).await?;
        Some(DiskUsage { used, limit })
    }

    async fn largest(&self, sandbox: &SandboxRef) -> Vec<(String, u64)> {
        self.manager.largest(sandbox.id.as_str()).await
    }

    async fn processes(&self, sandbox: &SandboxId) -> usize {
        self.manager.processes(sandbox.as_str()).await
    }

    fn release(&self, sandbox: &SandboxId) {
        let manager = Arc::clone(&self.manager);
        let sandbox = sandbox.as_str().to_owned();
        tokio::spawn(async move { manager.release(&sandbox).await });
    }

    /// The limits apply only through the cgroup delegation (SPEC-082 RES-001).
    fn limits(&self) -> SandboxLimits {
        let config = self.manager.config();
        if config.delegation.is_none() {
            return SandboxLimits::default();
        }
        SandboxLimits {
            cpus: config.limits.cpus,
            memory_bytes: config.limits.memory_max,
        }
    }
}

/// Sets up isolation for a contained server: resolves the prerequisites, prepares the cgroup
/// delegation, adopts holders of the previous process, starts the upkeep loop and installs the
/// port. Returns the manager (`None`: the previous containment stays).
///
/// # Errors
/// `namespaces` mode without its prerequisites.
pub async fn install(
    app: &yulius_app::context::App,
    args: &IsolationArgs,
    internal_port: u16,
) -> Result<Option<Arc<Manager>>, String> {
    if !app.config.contained || args.mode == IsolationMode::Legacy {
        return Ok(None);
    }
    let mut config = match resolve(
        args,
        &app.config.data_dir,
        &app.config.yulius_exe,
        internal_port,
    ) {
        Ok(config) => config,
        Err(missing) => {
            let message = missing.join("; ");
            if args.mode == IsolationMode::Namespaces {
                return Err(format!("sandbox isolation unavailable: {message}"));
            }
            tracing::warn!(
                missing = %message,
                "sandbox isolation unavailable; contained runs keep the previous containment \
                 (see `yulius doctor` and docs/operations.md)"
            );
            return Ok(None);
        }
    };
    match delegation() {
        Ok(delegation) => {
            tracing::info!(cgroup = %delegation.unit.display(), limits = ?config.limits, "per-sandbox limits on");
            config.delegation = Some(delegation);
        }
        Err(why) => tracing::warn!(%why, "per-sandbox CPU, memory and task limits are off"),
    }
    let manager = Manager::new(config);
    manager.adopt().await;
    let strict = args.mode == IsolationMode::Namespaces;
    app.set_isolation(Arc::new(NamespaceIsolation::new(
        Arc::clone(&manager),
        strict,
    )));
    let ticker = Arc::clone(&manager);
    tokio::spawn(async move {
        loop {
            ticker.tick().await;
            tokio::time::sleep(TICK).await;
        }
    });
    let status = manager.status().await;
    tracing::info!(
        base = ?status.base.as_ref().map(|b| b.id.clone()),
        egress = status.egress,
        limits = status.limits,
        "sandbox isolation on (namespace holder per sandbox)"
    );
    Ok(Some(manager))
}
