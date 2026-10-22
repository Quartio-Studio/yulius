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
    DiskUsage, DockerStatus, LimitCounters, SandboxIsolation, SandboxJoin, SandboxLimits,
    SandboxRef, SmokeCheck,
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

/// Whether isolated sandboxes get a Docker engine of their own (ADR-0151).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DockerMode {
    /// When the cgroup delegation is on (and the sandbox's system has `dockerd`).
    On,
    /// Never.
    Off,
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
    /// Task (process and thread) limit per sandbox (default: a quarter of the host's
    /// `kernel.threads-max` or `kernel.pid_max`, whichever is smaller, at least 8192, at most
    /// three quarters of the service's own `pids.max`).
    #[arg(long = "sandbox-pids", env = "YULIUS_SANDBOX_PIDS")]
    pub pids: Option<u64>,
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
    /// A Docker engine per isolated sandbox (`on` needs the cgroup delegation and `dockerd` in
    /// the sandbox's system).
    #[arg(long = "sandbox-docker", env = "YULIUS_SANDBOX_DOCKER", value_enum, default_value_t = DockerMode::On)]
    pub docker: DockerMode,
}

impl Default for IsolationArgs {
    fn default() -> Self {
        Self {
            mode: IsolationMode::Auto,
            egress: Egress::Direct,
            memory_gb: None,
            cpus: None,
            pids: None,
            disk_gb: 100,
            forward: Vec::new(),
            idle_minutes: 5,
            docker: DockerMode::On,
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
    let sysctl = |name: &str| std::fs::read_to_string(format!("/proc/sys/kernel/{name}"));
    let host_tasks = match (sysctl("threads-max"), sysctl("pid_max")) {
        (Ok(threads), Ok(pids)) => yulius_sandbox::cgroup::host_tasks(&threads, &pids),
        _ => None,
    };
    let defaults = Limits::defaults(total, host_tasks);
    let limits = Limits {
        memory_max: args.memory_gb.map(|g| g << 30).or(defaults.memory_max),
        cpus: args.cpus.or(defaults.cpus),
        pids_max: args.pids.or(defaults.pids_max),
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
        docker: docker_setting(args.docker),
    })
}

/// The Docker setting (SPEC-196 DKL-005: the reason when off). The host's `dockerd` does not
/// count: an engine runs the sandbox's own, which its front checks (SPEC-199 DSY-001).
///
/// # Errors
/// Why sandboxes get no engine from it.
pub fn docker_setting(mode: DockerMode) -> Result<(), String> {
    match mode {
        DockerMode::Off => Err(yulius_sandbox::manager::DOCKER_OFF.to_owned()),
        DockerMode::On => Ok(()),
    }
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
                    docker_dir: join.docker_dir.map(PathBuf::from),
                }))
            }
            Ok(None) if self.strict => Err(AppError::Conflict(
                "sandbox isolation is still being prepared (the base system is being built); \
                 try again in a few minutes"
                    .into(),
            )),
            Ok(None) => Ok(None),
            // Never the previous containment: nothing runs for a removed sandbox (SPEC-206).
            Err(PrepareError::Retired) => Err(AppError::NotFound),
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

    async fn limit_counters(&self, sandbox: &SandboxId) -> Option<LimitCounters> {
        let delegation = self.manager.config().delegation.clone()?;
        let sandbox = sandbox.as_str().to_owned();
        let counters = tokio::task::spawn_blocking(move || delegation.limit_counters(&sandbox))
            .await
            .ok()?;
        Some(LimitCounters {
            task_limit_hits: counters.task_limit_hits,
            oom_kills: counters.oom_kills,
        })
    }

    fn release(&self, sandbox: &SandboxId) {
        let manager = Arc::clone(&self.manager);
        let sandbox = sandbox.as_str().to_owned();
        tokio::spawn(async move { manager.release(&sandbox).await });
    }

    fn retire(&self, sandbox: &SandboxId) {
        let manager = Arc::clone(&self.manager);
        let sandbox = sandbox.as_str().to_owned();
        tokio::spawn(async move { manager.retire(&sandbox).await });
    }

    async fn prune_engine(&self, sandbox: &SandboxRef) -> Option<u64> {
        self.manager.prune_engine(sandbox.id.as_str()).await
    }

    async fn holder_live(&self, name: &str) -> bool {
        self.manager.holder_live(name).await
    }

    async fn remove_owned_tree(&self, path: &std::path::Path) -> Result<(), String> {
        self.manager.remove_owned_tree(path).await
    }

    async fn unused_bases(&self) -> Vec<PathBuf> {
        self.manager.unused_bases().await
    }

    async fn docker(&self, sandbox: &SandboxRef) -> bool {
        self.manager.docker_for(sandbox.id.as_str()).await
    }

    async fn docker_status(&self) -> Option<DockerStatus> {
        let manager = Arc::clone(&self.manager);
        tokio::task::spawn_blocking(move || docker_status(&manager))
            .await
            .ok()
    }

    async fn docker_smoke(
        &self,
        report: &(dyn Fn(SmokeCheck) + Send + Sync),
    ) -> Result<bool, String> {
        let config = self.manager.config();
        run_smoke(&config.exe, &config.data_dir, SMOKE_TIMEOUT, report).await
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

/// Longest smoke test started from Settings → Instance (it pulls images; SPEC-203 HST-015).
pub const SMOKE_TIMEOUT: Duration = Duration::from_secs(40 * 60);

/// The "sandbox Docker" verdict as this server's manager sees it (SPEC-203 HST-014).
fn docker_status(manager: &Manager) -> DockerStatus {
    use yulius_sandbox::docker_status::{Facts, own_engines, verdict};
    let config = manager.config();
    let rebuild = manager.rebuild();
    let facts = Facts::read(
        &config.data_dir,
        &config.exe,
        config.docker.clone(),
        config.delegation.is_some(),
        rebuild.building_since_ms.is_some(),
    );
    let verdict = verdict(&facts);
    DockerStatus {
        ok: verdict.ok,
        detail: verdict.detail,
        base: facts.base.map(|b| (b.id, b.dockerd)),
        host_dockerd: facts.host_dockerd,
        building_since_ms: rebuild.building_since_ms,
        pending_since_ms: rebuild.pending_since_ms,
        starts_at_ms: rebuild.starts_at_ms,
        last_build_secs: rebuild.last_build_secs,
        build_error: rebuild.last_error,
        own_engines: own_engines(&config.data_dir)
            .into_iter()
            .map(|o| (o.sandbox, o.dockerd.to_owned()))
            .collect(),
    }
}

/// Runs `<exe> doctor --docker-smoke --json` as this server's child (in its delegated cgroup,
/// as the service user), reporting each check line as it arrives; bounded by `timeout`, after
/// which its process group is killed (the next smoke removes its sandbox).
///
/// # Errors
/// It could not start, ended without a check (its last error line), or timed out.
pub async fn run_smoke(
    exe: &Path,
    data_dir: &Path,
    timeout: Duration,
    report: &(dyn Fn(SmokeCheck) + Send + Sync),
) -> Result<bool, String> {
    use tokio::io::AsyncBufReadExt as _;
    let mut child = tokio::process::Command::new(exe)
        .args(["doctor", "--docker-smoke", "--json", "--data-dir"])
        .arg(data_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start the smoke test: {e}"))?;
    let stdout = child.stdout.take().ok_or("no output")?;
    let stderr = child.stderr.take().ok_or("no error output")?;
    let errors = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        let mut last = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                last = line.chars().take(2000).collect();
            }
        }
        last
    });
    let pid = child.id();
    let work = async {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut checks = 0usize;
        let mut all_ok = true;
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(check) =
                serde_json::from_str::<yulius_contracts::api::updates::DockerSmokeCheck>(&line)
            else {
                continue;
            };
            checks += 1;
            all_ok &= check.ok;
            report(SmokeCheck {
                name: check.name,
                ok: check.ok,
                detail: check.detail,
            });
        }
        let status = child.wait().await;
        (checks, all_ok, status)
    };
    let Ok((checks, all_ok, status)) = tokio::time::timeout(timeout, work).await else {
        if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        return Err(format!(
            "the smoke test did not finish within {} minutes",
            timeout.as_secs() / 60
        ));
    };
    let last_error = errors.await.unwrap_or_default();
    let status = status.map_err(|e| e.to_string())?;
    if checks == 0 {
        return Err(if last_error.is_empty() {
            format!("the smoke test ended ({status}) without a check")
        } else {
            last_error.trim_start_matches("yulius: ").to_owned()
        });
    }
    Ok(all_ok && status.success())
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
            let root = Path::new(yulius_sandbox::cgroup::CGROUP_ROOT);
            let service = yulius_sandbox::cgroup::tasks_max_above(root, &delegation.unit);
            if args.pids.is_none() {
                config.limits = config.limits.within_service(service);
            }
            if let Err(error) = delegation.cap_sandboxes(service) {
                tracing::warn!(%error, "sandboxes together have no task limit below the service's");
            }
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
        docker = manager.docker(),
        no_docker = manager.docker_unavailable().unwrap_or_default(),
        "sandbox isolation on (namespace holder per sandbox)"
    );
    Ok(Some(manager))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-199 DSY-001 (task #185): the engine runs the sandbox's own `dockerd`, so only the
    /// setting turns sandboxes' Docker off, never the host's missing `dockerd`.
    #[test]
    fn only_the_setting_turns_sandbox_docker_off() {
        assert_eq!(docker_setting(DockerMode::On), Ok(()));
        assert_eq!(
            docker_setting(DockerMode::Off),
            Err(yulius_sandbox::manager::DOCKER_OFF.to_owned())
        );
    }

    /// SPEC-203 HST-015: the smoke started by the server reports each JSON check line as it
    /// arrives (other output ignored), names why it could not run, and is bounded.
    #[tokio::test]
    async fn the_servers_smoke_reads_check_lines_and_is_bounded()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir()?;
        let script = |name: &str, body: &str| -> std::io::Result<PathBuf> {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            Ok(path)
        };
        let seen = std::sync::Mutex::new(Vec::new());
        let report = |check: SmokeCheck| {
            if let Ok(mut seen) = seen.lock() {
                seen.push((check.name, check.ok));
            }
        };
        let checks = script(
            "checks",
            r#"[ "$*" = "doctor --docker-smoke --json --data-dir /d" ] || exit 9
echo 'running in a delegated transient unit'
echo '{"name":"socket","ok":true,"detail":"ok"}'
echo '{"name":"hello-world","ok":false,"detail":"pull rate limit"}'
exit 1"#,
        )?;
        let passed = run_smoke(&checks, Path::new("/d"), Duration::from_secs(30), &report).await;
        assert_eq!(passed, Ok(false));
        assert_eq!(
            seen.lock().map(|s| s.clone()).unwrap_or_default(),
            vec![
                ("socket".to_owned(), true),
                ("hello-world".to_owned(), false)
            ]
        );

        let refused = script(
            "refused",
            "echo 'yulius: no sandbox base yet: the server builds it in the background' >&2\nexit 1",
        )?;
        let error = run_smoke(&refused, Path::new("/d"), Duration::from_secs(30), &report).await;
        assert_eq!(
            error,
            Err("no sandbox base yet: the server builds it in the background".to_owned())
        );

        let stuck = script(
            "stuck",
            "echo '{\"name\":\"a\",\"ok\":true,\"detail\":\"\"}'\nsleep 600",
        )?;
        let started = std::time::Instant::now();
        let timed_out =
            run_smoke(&stuck, Path::new("/d"), Duration::from_millis(500), &report).await;
        assert!(
            timed_out
                .as_ref()
                .is_err_and(|e| e.contains("did not finish")),
            "{timed_out:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(60));
        Ok(())
    }
}
