//! Sandbox isolation port (SPEC-002 ADR-22): the per-sandbox namespace holder a contained run
//! joins. Without an adapter (development hosts, tests, missing prerequisites) contained runs
//! keep the previous containment (ADR-03/ADR-12).

use std::path::PathBuf;

use async_trait::async_trait;
use yulius_domain::ids::SandboxId;

use crate::error::AppResult;

/// The sandbox a run asks to enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRef {
    /// Identifier.
    pub id: SandboxId,
    /// Host name inside (from the slug).
    pub hostname: String,
    /// The sandbox directory.
    pub dir: PathBuf,
    /// First dev-server port (PRJ-010).
    pub port_first: u16,
    /// Last dev-server port (inclusive).
    pub port_last: u16,
}

/// What a run needs to join its sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxJoin {
    /// The holder process inside the namespaces.
    pub holder_pid: u32,
    /// Expected inode of its user namespace.
    pub user_ns: u64,
    /// Expected inode of its network namespace.
    pub net_ns: u64,
    /// Expected inode of its mount namespace.
    pub mnt_ns: u64,
    /// The sandbox's cgroup for agent runs, shells and browsers, when limits apply.
    pub cgroup: Option<PathBuf>,
    /// The sandbox's cgroup for CI jobs, weighted above `cgroup` (SPEC-053 CPU-203).
    pub ci_cgroup: Option<PathBuf>,
    /// The merged system layer inside the holder's mount namespace.
    pub system_root: PathBuf,
    /// Host name inside.
    pub hostname: String,
    /// `yulius` executable that installs the seccomp filter (`sandbox-exec`).
    pub exec_shim: Option<PathBuf>,
    /// Host paths bound read-only on top of the system layer (the agent CLI's install).
    pub extra_readonly: Vec<PathBuf>,
    /// Directories covered by an empty tmpfs before the run's binds (the data directory).
    pub private_dirs: Vec<PathBuf>,
    /// Host path of the sandbox system's current `/etc/passwd` (its upper layer's, else the
    /// base's): users' identity files are built on it.
    pub system_passwd: Option<PathBuf>,
    /// Same for `/etc/group`.
    pub system_group: Option<PathBuf>,
    /// Run on the host's read-only system instead of the system layer, in the holder's network
    /// (a production CD run, SPEC-134); the resolver file shown at the host's resolver path.
    pub host_system: Option<PathBuf>,
    /// The sandbox's Docker directory on the host (ADR-0158), shown to the run at
    /// `/run/yulius/docker` with `DOCKER_HOST`; `None` for a host-system run.
    pub docker_dir: Option<PathBuf>,
}

/// The CPUs and memory every sandbox may use, where the adapter enforces them (SPEC-082
/// RES-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SandboxLimits {
    /// Whole CPUs (`cpu.max`); `None`: no CPU limit.
    pub cpus: Option<u32>,
    /// Bytes (`memory.max`); `None`: no memory limit.
    pub memory_bytes: Option<u64>,
}

/// A sandbox's disk against its soft quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    /// Bytes in use.
    pub used: u64,
    /// The quota in bytes.
    pub limit: u64,
}

/// Whether sandboxes get Docker on this server (SPEC-203 HST-014), from the isolation adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DockerStatus {
    /// The verdict holds.
    pub ok: bool,
    /// The verdict's words.
    pub detail: String,
    /// The current base: id, and whether it has `dockerd`.
    pub base: Option<(String, bool)>,
    /// The host has `dockerd`.
    pub host_dockerd: bool,
    /// A build runs since (ms since the epoch).
    pub building_since_ms: Option<i64>,
    /// The host changed then; a build waits.
    pub pending_since_ms: Option<i64>,
    /// When that build starts.
    pub starts_at_ms: Option<i64>,
    /// The last build's duration.
    pub last_build_secs: Option<u64>,
    /// The last build's error.
    pub build_error: Option<String>,
    /// Sandboxes on their own Docker: (sandbox id, its `dockerd`).
    pub own_engines: Vec<(String, String)>,
}

/// One check of the Docker smoke test as it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeCheck {
    /// Name.
    pub name: String,
    /// It held.
    pub ok: bool,
    /// What was seen.
    pub detail: String,
}

/// See the module docs.
#[async_trait]
pub trait SandboxIsolation: Send + Sync {
    /// Prepares `sandbox` (its holder, limits, quota) and takes a lease that
    /// [`SandboxIsolation::release`] ends once the run's process joined. `Ok(None)`: this run
    /// uses the previous containment.
    ///
    /// # Errors
    /// `Conflict` over the disk quota; `Upstream` when the holder cannot start.
    async fn prepare(&self, sandbox: &SandboxRef) -> AppResult<Option<SandboxJoin>>;

    /// The last measurement of `sandbox` against its quota; `None` when unmeasured or without
    /// a quota.
    async fn disk(&self, _sandbox: &SandboxRef) -> Option<DiskUsage> {
        None
    }

    /// Measures `sandbox` now (after space was freed) and keeps the reading.
    async fn measure_disk(&self, _sandbox: &SandboxRef) -> Option<DiskUsage> {
        None
    }

    /// The largest consumers of the last measurement, largest first, as paths relative to the
    /// sandbox directory (`worktrees/<task>`, `ci/jobs`) or `system` (SPEC-059 DSK-202).
    async fn largest(&self, _sandbox: &SandboxRef) -> Vec<(String, u64)> {
        Vec::new()
    }

    /// Processes running in `sandbox`'s namespaces besides its holder: 0 without a live holder,
    /// and where nothing isolates sandboxes (a removed playground's purge waits for them,
    /// SPEC-171).
    async fn processes(&self, _sandbox: &SandboxId) -> usize {
        0
    }

    /// Ends a lease of [`SandboxIsolation::prepare`] (may finish in the background).
    fn release(&self, sandbox: &SandboxId);

    /// Prunes `sandbox`'s Docker engine: build cache and images no container uses (never
    /// containers or volumes). Returns the bytes freed; `None` without an engine or a live holder
    /// (SPEC-187 DKR-006).
    async fn prune_engine(&self, _sandbox: &SandboxRef) -> Option<u64> {
        None
    }

    /// Whether `sandbox`'s runs reach a Docker engine of its own (ADR-0151): its holder serves
    /// one.
    async fn docker(&self, _sandbox: &SandboxRef) -> bool {
        false
    }

    /// Whether sandboxes get Docker, why not, the base's rebuild and the sandboxes on their own
    /// Docker (SPEC-203 HST-014); `None` where the adapter cannot tell.
    async fn docker_status(&self) -> Option<DockerStatus> {
        None
    }

    /// Runs the Docker smoke test (`yulius doctor --docker-smoke`) in a throwaway sandbox,
    /// reporting each check as it ends; `Ok(passed)`, or why it could not run (SPEC-203 HST-015).
    ///
    /// # Errors
    /// It could not start or did not finish.
    async fn docker_smoke(
        &self,
        _report: &(dyn Fn(SmokeCheck) + Send + Sync),
    ) -> Result<bool, String> {
        Err("this server cannot run the Docker smoke test".into())
    }

    /// The limits every sandbox runs under; none where nothing enforces them (no cgroup
    /// delegation).
    fn limits(&self) -> SandboxLimits {
        SandboxLimits::default()
    }
}
