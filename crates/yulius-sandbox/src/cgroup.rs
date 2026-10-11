//! Per-sandbox cgroup v2 limits through systemd delegation (SPEC-002 ADR-20 §5).
//!
//! With `Delegate=yes` and `DelegateSubgroup=server` on `yulius.service`, systemd starts the
//! server in `<unit>/server` and hands `<unit>` to the service user. The server then:
//! - moves processes left in `<unit>` from before delegation (runs, browsers) to `<unit>/legacy`;
//! - enables the `cpu`, `memory` and `pids` controllers for `<unit>` and `<unit>/sandboxes`;
//! - gives each sandbox `<unit>/sandboxes/<sandbox>` with its limits.
//!
//! The holder, `slirp4netns` and every run of a sandbox join that group before they start
//! anything ([`join`]). Without delegation (the parent of the server's cgroup is not ours),
//! nothing changes and sandboxes run without limits.

use std::io;
use std::path::{Path, PathBuf};

/// Where cgroup v2 is mounted.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";
/// Controllers enabled for sandboxes (those available are used).
const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];
/// Processes moved out of the unit's root at most (a bound for a pathological unit).
const MAX_MOVED: usize = 4096;
/// The leaf of processes found in the unit's root.
const LEGACY_LEAF: &str = "legacy";
/// The parent of sandbox groups.
const SANDBOXES: &str = "sandboxes";

/// Limits of one sandbox; `None` leaves a limit unset (`max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// `memory.max` in bytes.
    pub memory_max: Option<u64>,
    /// `cpu.max` as whole CPUs (quota = CPUs × 100 ms per 100 ms).
    pub cpus: Option<u32>,
    /// `pids.max`.
    pub pids_max: Option<u64>,
}

impl Limits {
    /// Defaults for a host: half the memory (at least 2 GiB), every CPU but two (at least one),
    /// 8192 tasks.
    #[must_use]
    pub fn defaults(total_memory: u64, cpus: u32) -> Self {
        Self {
            memory_max: Some((total_memory / 2).max(2 << 30)),
            cpus: Some(cpus.saturating_sub(2).max(1)),
            pids_max: Some(8192),
        }
    }

    /// The files written for these limits.
    #[must_use]
    pub fn files(&self) -> Vec<(&'static str, String)> {
        let mut files = Vec::new();
        files.push((
            "memory.max",
            self.memory_max
                .map_or_else(|| "max".to_owned(), |b| b.to_string()),
        ));
        files.push((
            "cpu.max",
            self.cpus.map_or_else(
                || "max 100000".to_owned(),
                |c| format!("{} 100000", u64::from(c) * 100_000),
            ),
        ));
        files.push((
            "pids.max",
            self.pids_max
                .map_or_else(|| "max".to_owned(), |n| n.to_string()),
        ));
        files
    }
}

/// The delegated subtree (the unit's cgroup directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    /// `<unit>` under [`CGROUP_ROOT`].
    pub unit: PathBuf,
    /// Controllers enabled for sandboxes.
    pub controllers: Vec<String>,
}

/// The cgroup path of this process from the text of `/proc/self/cgroup` (cgroup v2 line).
#[must_use]
pub fn own_path(proc_self_cgroup: &str) -> Option<String> {
    proc_self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::to_owned)
}

fn writable(path: &Path) -> bool {
    nix::unistd::access(path, nix::unistd::AccessFlags::W_OK).is_ok()
}

/// The delegated unit of a server whose cgroup is `own` (a path relative to `fs_root`), when the
/// parent of `own` is ours: `DelegateSubgroup=` placed the server one level down.
///
/// # Errors
/// Why there is no usable delegation (for logs and `yulius doctor`).
pub fn detect(fs_root: &Path, own: &str) -> Result<PathBuf, String> {
    let own = own.trim().trim_start_matches('/');
    if own.is_empty() {
        return Err("the server runs in the root cgroup".into());
    }
    let Some((parent, _leaf)) = own.rsplit_once('/') else {
        return Err(format!(
            "the server's cgroup /{own} has no delegated parent"
        ));
    };
    let unit = fs_root.join(parent);
    let needed = [
        unit.join("cgroup.procs"),
        unit.join("cgroup.subtree_control"),
        unit.clone(),
    ];
    if needed.iter().all(|p| writable(p)) {
        Ok(unit)
    } else {
        Err(format!(
            "/{parent} is not delegated to this user (set Delegate=yes and \
             DelegateSubgroup=server on the service)"
        ))
    }
}

fn write(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text)
}

fn controllers_of(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("cgroup.controllers"))
        .unwrap_or_default()
        .split_whitespace()
        .filter(|c| CONTROLLERS.contains(c))
        .map(str::to_owned)
        .collect()
}

fn enable(dir: &Path, controllers: &[String]) -> io::Result<()> {
    if controllers.is_empty() {
        return Ok(());
    }
    let line = controllers
        .iter()
        .map(|c| format!("+{c}"))
        .collect::<Vec<_>>()
        .join(" ");
    write(&dir.join("cgroup.subtree_control"), &line)
}

/// Prepares the delegated `unit`: processes in its root move to `legacy/`, controllers are
/// enabled for it and for `sandboxes/`.
///
/// # Errors
/// A cgroup file cannot be written.
pub fn prepare(unit: &Path) -> io::Result<Delegation> {
    let stray: Vec<String> = std::fs::read_to_string(unit.join("cgroup.procs"))?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(MAX_MOVED)
        .map(str::to_owned)
        .collect();
    if !stray.is_empty() {
        let legacy = unit.join(LEGACY_LEAF);
        std::fs::create_dir_all(&legacy)?;
        for pid in &stray {
            // A process that exited meanwhile is not an error.
            let _ = write(&legacy.join("cgroup.procs"), pid);
        }
    }
    let controllers = controllers_of(unit);
    enable(unit, &controllers)?;
    let sandboxes = unit.join(SANDBOXES);
    std::fs::create_dir_all(&sandboxes)?;
    enable(&sandboxes, &controllers)?;
    Ok(Delegation {
        unit: unit.to_path_buf(),
        controllers,
    })
}

impl Delegation {
    /// The group of `sandbox` (created when missing) with `limits` applied.
    ///
    /// # Errors
    /// The group cannot be created or a limit cannot be written.
    pub fn sandbox_group(&self, sandbox: &str, limits: &Limits) -> io::Result<PathBuf> {
        let dir = self.unit.join(SANDBOXES).join(sandbox);
        std::fs::create_dir_all(&dir)?;
        for (file, value) in limits.files() {
            let controller = file.split('.').next().unwrap_or_default();
            if self.controllers.iter().any(|c| c == controller) {
                write(&dir.join(file), &value)?;
            }
        }
        Ok(dir)
    }
}

/// Moves the calling process into the cgroup directory `dir` (its children follow).
///
/// # Errors
/// `dir` is not a cgroup directory under [`CGROUP_ROOT`], or the move is refused.
pub fn join(dir: &Path) -> io::Result<()> {
    if !dir.starts_with(CGROUP_ROOT) || dir.components().any(|c| c.as_os_str() == "..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a cgroup directory",
        ));
    }
    write(&dir.join("cgroup.procs"), "0")
}

/// Total memory in bytes from the text of `/proc/meminfo`.
#[must_use]
pub fn mem_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_room_for_the_host() {
        let limits = Limits::defaults(48 << 30, 12);
        assert_eq!(limits.memory_max, Some(24 << 30));
        assert_eq!(limits.cpus, Some(10));
        assert_eq!(limits.pids_max, Some(8192));
        let small = Limits::defaults(2 << 30, 2);
        assert_eq!(small.memory_max, Some(2 << 30));
        assert_eq!(small.cpus, Some(1));
        assert_eq!(
            Limits::default().files(),
            vec![
                ("memory.max", "max".to_owned()),
                ("cpu.max", "max 100000".to_owned()),
                ("pids.max", "max".to_owned()),
            ]
        );
        assert_eq!(limits.files()[1], ("cpu.max", "1000000 100000".to_owned()));
    }

    #[test]
    fn the_cgroup_line_and_memory_are_read() {
        assert_eq!(
            own_path("0::/system.slice/yulius.service/server\n").as_deref(),
            Some("/system.slice/yulius.service/server")
        );
        assert_eq!(own_path("1:name=systemd:/x\n"), None);
        assert_eq!(
            mem_total("MemTotal:       49283528 kB\nMemFree: 1 kB\n"),
            Some(49_283_528 * 1024)
        );
    }

    #[test]
    fn delegation_needs_a_writable_parent() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let unit = root.path().join("system.slice/yulius.service");
        std::fs::create_dir_all(unit.join("server"))?;
        std::fs::write(unit.join("cgroup.procs"), "")?;
        std::fs::write(unit.join("cgroup.subtree_control"), "")?;
        assert_eq!(
            detect(root.path(), "/system.slice/yulius.service/server")?,
            unit
        );
        assert!(detect(root.path(), "/").is_err());
        assert!(detect(root.path(), "/lonely").is_err());
        assert!(detect(root.path(), "/system.slice/other.service/server").is_err());
        Ok(())
    }

    #[test]
    fn preparing_a_fake_tree_writes_the_expected_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let unit = root.path().join("u");
        std::fs::create_dir_all(&unit)?;
        std::fs::write(unit.join("cgroup.procs"), "")?;
        std::fs::write(
            unit.join("cgroup.controllers"),
            "cpuset cpu io memory pids\n",
        )?;
        let delegation = prepare(&unit)?;
        assert_eq!(delegation.controllers, vec!["cpu", "memory", "pids"]);
        assert_eq!(
            std::fs::read_to_string(unit.join("cgroup.subtree_control"))?,
            "+cpu +memory +pids"
        );
        let group = delegation.sandbox_group(
            "s1",
            &Limits {
                memory_max: Some(1 << 30),
                cpus: Some(2),
                pids_max: Some(64),
            },
        )?;
        assert_eq!(
            std::fs::read_to_string(group.join("memory.max"))?,
            "1073741824"
        );
        assert_eq!(
            std::fs::read_to_string(group.join("cpu.max"))?,
            "200000 100000"
        );
        assert_eq!(std::fs::read_to_string(group.join("pids.max"))?, "64");
        assert!(join(&group).is_err(), "only under /sys/fs/cgroup");
        Ok(())
    }
}
