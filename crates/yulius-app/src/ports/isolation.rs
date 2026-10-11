//! Sandbox isolation port (SPEC-002 ADR-20): the per-sandbox namespace holder a contained run
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
    /// The sandbox's cgroup, when limits apply.
    pub cgroup: Option<PathBuf>,
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

    /// Ends a lease of [`SandboxIsolation::prepare`] (may finish in the background).
    fn release(&self, sandbox: &SandboxId);
}
