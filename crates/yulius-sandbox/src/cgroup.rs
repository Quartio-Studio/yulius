//! Per-sandbox cgroup v2 limits through systemd delegation (SPEC-002 ADR-22 §5).
//!
//! With `Delegate=yes` and `DelegateSubgroup=server` on `yulius.service`, systemd starts the
//! server in `<unit>/server` and hands `<unit>` to the service user. The server then:
//! - moves processes left in `<unit>` from before delegation (runs, browsers) to `<unit>/legacy`;
//! - enables the `cpu`, `memory` and `pids` controllers for `<unit>` and `<unit>/sandboxes`;
//! - gives each sandbox `<unit>/sandboxes/<sandbox>` with its limits, split by CPU weight into
//!   `system/` (the holder), `agents/` (runs) and `ci/` (CI jobs) when `cpu` is available
//!   (SPEC-053); `ci/` and `engine/` then also get 3/8 of its task limit each, so sessions keep
//!   a quarter of it whatever CI jobs and containers do (SPEC-268);
//! - limits `<unit>/sandboxes` to three quarters of the service's own task limit, so the server
//!   keeps a quarter however many sandboxes are busy (SPEC-272).
//!
//! The holder, `slirp4netns` and every run of a sandbox join their leaf before they start
//! anything ([`join`]). Without delegation (the parent of the server's cgroup is not ours),
//! nothing changes and sandboxes run without limits.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

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
/// A sandbox's leaves and their `cpu.weight` (SPEC-053 CPU-201): CI gets four times the agents'
/// share, so up to 80 % of the sandbox under full contention while the holder and the engine
/// idle. The engine's group (SPEC-187 DKR-005) is the root of its cgroup namespace: containers
/// get groups below it.
const LEAVES: [(&str, u32); 4] = [
    ("system", 100),
    ("agents", 100),
    ("ci", 400),
    ("engine", 100),
];
/// The engine's group, also without the `cpu` split.
const ENGINE_LEAF: &str = "engine";

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

/// The task limit of a sandbox at least (the fixed default before SPEC-268).
const MIN_PIDS: u64 = 8192;
/// The share of the host's task table a sandbox may use by default (SPEC-268 PID-001).
const HOST_TASKS_SHARE: u64 = 4;

/// The host's task table: the smaller of `kernel.threads-max` and `kernel.pid_max`, from the text
/// of those files; `None` when one cannot be read as a positive number.
#[must_use]
pub fn host_tasks(threads_max: &str, pid_max: &str) -> Option<u64> {
    let read = |text: &str| text.trim().parse::<u64>().ok().filter(|n| *n > 0);
    Some(read(threads_max)?.min(read(pid_max)?))
}

/// The task limit that binds the group `dir` (the server's delegated unit), below `fs_root`: the
/// smallest `pids.max` of it and its ancestors there; `None` when none sets one (`max`).
#[must_use]
pub fn tasks_max_above(fs_root: &Path, dir: &Path) -> Option<u64> {
    dir.ancestors()
        .take_while(|group| group.starts_with(fs_root) && *group != fs_root)
        .filter_map(|group| std::fs::read_to_string(group.join("pids.max")).ok())
        .filter_map(|text| text.trim().parse::<u64>().ok())
        .min()
}

/// Three quarters of `service`, at least 1: what sandboxes may use of the service's tasks.
fn service_share(service: u64) -> u64 {
    (service / 4 * 3).max(1)
}

/// The common task limit of all sandboxes together under a service limited to `service` tasks
/// ([`tasks_max_above`]): three quarters of it, so the server keeps a quarter however many
/// sandboxes are busy (SPEC-272 SHR-001); `None` (`max`) when the service has no limit.
#[must_use]
pub fn sandboxes_tasks(service: Option<u64>) -> Option<u64> {
    service.map(service_share)
}

impl Limits {
    /// These limits with the task limit at most three quarters of `service` (the delegated unit's
    /// own limit, [`tasks_max_above`]): systemd's `DefaultTasksMax` caps the whole service at
    /// 15 % of the host's table, so a sandbox at a quarter of the host would starve the server
    /// and the other sandboxes before its own limit (SPEC-268 PID-001, review round 1). For the
    /// default only: an explicit `YULIUS_SANDBOX_PIDS` stands.
    #[must_use]
    pub fn within_service(self, service: Option<u64>) -> Self {
        let pids_max = match (self.pids_max, service) {
            (Some(pids), Some(service)) => Some(pids.min(service_share(service))),
            (pids, _) => pids,
        };
        Self { pids_max, ..self }
    }

    /// Defaults for a host: the memory but a reserve for the host and Yulius (an eighth, at
    /// least 4 GiB; the sandbox gets at least 2 GiB), no CPU quota, and a quarter of the host's
    /// task table `host_tasks` ([`host_tasks`]), at least 8192 (SPEC-268 PID-001). The fixed
    /// 8192 tasks (threads) were used up by five tasks building Rust and running tests with
    /// containers, and Claude Code aborts when it cannot start a thread (F 2026-10-22). Half the memory
    /// was too little for one sandbox running all the agents: ten sessions and two CI lanes
    /// thrashed its page cache at 25 GiB while 20 GiB of the host stayed free, and CI jobs timed
    /// out (F 2026-10-13).
    ///
    /// The CPU is shared by weight, not capped (SPEC-082 RES-006): the quota of every CPU but two
    /// throttled the sandbox in half of its busy 100 ms periods (900 801 of 1 792 320 on the VM,
    /// 2026-10-11..13), stopping all of its threads until the next period while the two CPUs it
    /// left stayed nearly idle. Yulius's own group and the host's services keep their share by
    /// weight whenever they need it.
    #[must_use]
    pub fn defaults(total_memory: u64, host_tasks: Option<u64>) -> Self {
        let reserve = (total_memory / 8).max(4 << 30);
        Self {
            memory_max: Some(total_memory.saturating_sub(reserve).max(2 << 30)),
            cpus: None,
            pids_max: Some(host_tasks.map_or(MIN_PIDS, |n| (n / HOST_TASKS_SHARE).max(MIN_PIDS))),
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
#[derive(Debug, Clone)]
pub struct Delegation {
    /// `<unit>` under [`CGROUP_ROOT`].
    pub unit: PathBuf,
    /// Controllers enabled for sandboxes.
    pub controllers: Vec<String>,
    /// Set by [`Delegation::unsplit_all`], which holds it while it undoes the split: a split is
    /// either done before the undo or not at all once the server stops (shared by clones).
    stopping: Arc<Mutex<bool>>,
}

impl PartialEq for Delegation {
    fn eq(&self, other: &Self) -> bool {
        self.unit == other.unit && self.controllers == other.controllers
    }
}

impl Eq for Delegation {}

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

/// Moves the processes of the group `from` (at most [`MAX_MOVED`]) to `to`, created when there
/// is any.
fn move_processes(from: &Path, to: &Path) -> io::Result<()> {
    let stray: Vec<String> = std::fs::read_to_string(from.join("cgroup.procs"))?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(MAX_MOVED)
        .map(str::to_owned)
        .collect();
    if !stray.is_empty() {
        std::fs::create_dir_all(to)?;
        for pid in &stray {
            // A process that exited meanwhile is not an error.
            let _ = write(&to.join("cgroup.procs"), pid);
        }
    }
    Ok(())
}

/// Prepares the delegated `unit`: processes in its root move to `legacy/`, controllers are
/// enabled for it and for `sandboxes/`.
///
/// # Errors
/// A cgroup file cannot be written.
pub fn prepare(unit: &Path) -> io::Result<Delegation> {
    move_processes(unit, &unit.join(LEGACY_LEAF))?;
    let controllers = controllers_of(unit);
    enable(unit, &controllers)?;
    let sandboxes = unit.join(SANDBOXES);
    std::fs::create_dir_all(&sandboxes)?;
    enable(&sandboxes, &controllers)?;
    Ok(Delegation {
        unit: unit.to_path_buf(),
        controllers,
        stopping: Arc::new(Mutex::new(false)),
    })
}

/// The groups a sandbox's processes join (SPEC-053 CPU-201). Without the `cpu` controller the
/// first three are the sandbox's own group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxGroups {
    /// The holder and `slirp4netns`.
    pub system: PathBuf,
    /// Agent runs, shells, browsers and the Cockpit.
    pub agents: PathBuf,
    /// CI jobs.
    pub ci: PathBuf,
    /// The Docker engine and, below it, its containers (ADR-0151); always a group of its own.
    pub engine: PathBuf,
}

impl SandboxGroups {
    fn single(dir: &Path) -> Self {
        Self {
            system: dir.to_path_buf(),
            agents: dir.to_path_buf(),
            ci: dir.to_path_buf(),
            engine: dir.join(ENGINE_LEAF),
        }
    }

    fn leaves(dir: &Path) -> Self {
        let [system, agents, ci, engine] = LEAVES.map(|(leaf, _)| dir.join(leaf));
        Self {
            system,
            agents,
            ci,
            engine,
        }
    }
}

/// Hands the CPU of the sandbox group `dir` to its leaves by weight. cgroup v2 keeps processes
/// out of a group that passes controllers down, so those still in `dir` (a holder and runs from
/// before the split) move to `agents` first.
fn split_cpu(dir: &Path, groups: &SandboxGroups) -> io::Result<()> {
    move_processes(dir, &groups.agents)?;
    write(&dir.join("cgroup.subtree_control"), "+cpu")?;
    for (leaf, weight) in LEAVES {
        write(&dir.join(leaf).join("cpu.weight"), &weight.to_string())?;
    }
    Ok(())
}

/// How often a sandbox was at a limit, as its cgroups count it (SPEC-268 PID-004). The counts
/// only grow while the group lives; a rise between two reads is what tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimitCounters {
    /// Forks and thread creations refused to the sandbox's sessions at a task limit
    /// ([`Delegation::limit_counters`]).
    pub task_limit_hits: u64,
    /// Processes the OOM killer ended in the sandbox (`memory.events` `oom_kill`).
    pub oom_kills: u64,
}

/// The number after `key` in a cgroup's flat keyed file (`max 3\n`), 0 when absent.
fn keyed(text: &str, key: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// Of every eight tasks of a sandbox, what CI jobs and the engine may each use (SPEC-268 PID-002):
/// together three quarters, so the agents' leaf always keeps a quarter.
const LEAF_TASKS_EIGHTHS: u64 = 3;

/// Gives the leaves of the split sandbox group `dir` their task limits below the sandbox's
/// `pids_max`: `ci` and `engine` each 3/8 of it, the agents and the holder none of their own
/// (SPEC-268 PID-002). A fork is refused at the first ancestor at its limit, so CI jobs and the
/// containers stop at their leaf while sessions keep the rest.
fn cap_leaves(dir: &Path, groups: &SandboxGroups, pids_max: u64) -> io::Result<()> {
    write(&dir.join("cgroup.subtree_control"), "+pids")?;
    let cap = (pids_max / 8 * LEAF_TASKS_EIGHTHS + pids_max % 8 * LEAF_TASKS_EIGHTHS / 8).max(1);
    for leaf in [&groups.ci, &groups.engine] {
        write(&leaf.join("pids.max"), &cap.to_string())?;
    }
    Ok(())
}

impl Delegation {
    /// Writes the common task limit of all sandboxes, `<unit>/sandboxes/pids.max`, for a service
    /// limited to `service` tasks ([`sandboxes_tasks`]); `max` without a service limit, since the
    /// group and an earlier server's value survive restarts (`KillMode=process`, SPEC-272
    /// SHR-001). Nothing without the `pids` controller.
    ///
    /// # Errors
    /// The limit cannot be written.
    pub fn limit_sandboxes(&self, service: Option<u64>) -> io::Result<()> {
        if !self.controllers.iter().any(|c| c == "pids") {
            return Ok(());
        }
        let limit = sandboxes_tasks(service).map_or_else(|| "max".to_owned(), |n| n.to_string());
        write(&self.unit.join(SANDBOXES).join("pids.max"), &limit)
    }

    /// The paths of `sandbox`'s groups, as [`Delegation::sandbox_group`] makes them, without
    /// touching anything.
    #[must_use]
    pub fn sandbox_paths(&self, sandbox: &str) -> SandboxGroups {
        let dir = self
            .unit
            .join(SANDBOXES)
            .join(crate::manager::path_name(sandbox));
        if self.controllers.iter().any(|c| c == "cpu") {
            SandboxGroups::leaves(&dir)
        } else {
            SandboxGroups::single(&dir)
        }
    }

    /// `sandbox`'s limit counters (SPEC-268 PID-004, review rounds 1 and 2; SPEC-272 SHR-003).
    /// Refused forks are `max` in `pids.events` of every group of the sandbox (at most 4096) but
    /// CI's and the engine's subtrees, whose processes are stopped at their own leaf limits while
    /// sessions keep forking; a paused run's `agents/run-*` group counts. Kernel 6.8 counts a
    /// refused fork there, in the forking task's own group, wherever the limit was. Where the
    /// sandbox has `pids.events.local` (6.13+) the same groups' `.local` files are read instead,
    /// and `<unit>/sandboxes`'s: by default `max` there counts where a limit was hit (the
    /// sandbox's own, or the common one of all sandboxes, which is shared), with
    /// `pids_localevents` the forking group, as on 6.8. OOM kills are the sandbox's
    /// `memory.events` `oom_kill` (hierarchical). Zero for a group that is not there.
    #[must_use]
    pub fn limit_counters(&self, sandbox: &str) -> LimitCounters {
        let dir = self
            .unit
            .join(SANDBOXES)
            .join(crate::manager::path_name(sandbox));
        let read = |dir: &Path, file: &str, key: &str| {
            std::fs::read_to_string(dir.join(file))
                .ok()
                .map(|text| keyed(&text, key))
        };
        let oom_kills = read(&dir, "memory.events", "oom_kill").unwrap_or(0);
        let local = dir.join("pids.events.local").exists();
        let events = if local {
            "pids.events.local"
        } else {
            "pids.events"
        };
        let groups = self.sandbox_paths(sandbox);
        let apart = [groups.ci.as_path(), groups.engine.as_path()];
        let mut task_limit_hits = if local {
            read(&self.unit.join(SANDBOXES), events, "max").unwrap_or(0)
        } else {
            0
        };
        let mut stack = vec![dir.clone()];
        let mut seen = 0;
        while let Some(group) = stack.pop() {
            seen += 1;
            if seen > MAX_MOVED {
                break;
            }
            let hits = read(&group, events, "max").unwrap_or(0);
            task_limit_hits = task_limit_hits.saturating_add(hits);
            if let Ok(entries) = std::fs::read_dir(&group) {
                stack.extend(
                    entries
                        .flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| e.path())
                        .filter(|path| group != dir || !apart.contains(&path.as_path())),
                );
            }
        }
        LimitCounters {
            task_limit_hits,
            oom_kills,
        }
    }

    /// Prepares `sandbox`'s groups ([`Delegation::sandbox_paths`]): creates them, writes the
    /// limits and splits the CPU.
    ///
    /// # Errors
    /// A group cannot be created or a limit written.
    pub fn sandbox_group(&self, sandbox: &str, limits: &Limits) -> io::Result<SandboxGroups> {
        let dir = self
            .unit
            .join(SANDBOXES)
            .join(crate::manager::path_name(sandbox));
        std::fs::create_dir_all(&dir)?;
        for (file, value) in limits.files() {
            let controller = file.split('.').next().unwrap_or_default();
            if self.controllers.iter().any(|c| c == controller) {
                write(&dir.join(file), &value)?;
            }
        }
        if !self.controllers.iter().any(|c| c == "cpu") {
            let groups = SandboxGroups::single(&dir);
            std::fs::create_dir_all(&groups.engine)?;
            return Ok(groups);
        }
        let groups = SandboxGroups::leaves(&dir);
        for leaf in [&groups.system, &groups.agents, &groups.ci, &groups.engine] {
            std::fs::create_dir_all(leaf)?;
        }
        // Joining a leaf works without the weights too; the next preparation tries again.
        // A failure that persists (a process that cannot be moved) logs at every preparation.
        let stopping = self.stopping.lock().unwrap_or_else(PoisonError::into_inner);
        if *stopping {
            return Ok(groups);
        }
        if let Err(error) = split_cpu(&dir, &groups) {
            tracing::warn!(sandbox, %error, "CI jobs run without CPU priority for now");
        }
        if let Some(pids_max) = limits.pids_max
            && self.controllers.iter().any(|c| c == "pids")
            && let Err(error) = cap_leaves(&dir, &groups, pids_max)
        {
            tracing::warn!(sandbox, %error, "CI jobs and containers run without their own task limit for now");
        }
        drop(stopping);
        Ok(groups)
    }

    /// Takes the task limits and the CPU back from every sandbox's leaves (`-pids`, `-cpu`) when
    /// the server stops (SPEC-053 CPU-205, SPEC-268 PID-003). An older binary started after this
    /// one (a rollback) joins the sandbox group itself, which the kernel refuses while the group
    /// passes a controller down. Processes stay in their leaves; the next preparation splits
    /// again.
    pub fn unsplit_all(&self) {
        let mut stopping = self.stopping.lock().unwrap_or_else(PoisonError::into_inner);
        *stopping = true;
        let Ok(entries) = std::fs::read_dir(self.unit.join(SANDBOXES)) else {
            return;
        };
        for entry in entries.flatten().take(MAX_MOVED) {
            let control = entry.path().join("cgroup.subtree_control");
            let split = std::fs::read_to_string(&control).is_ok_and(|text| {
                text.split_whitespace()
                    .any(|c| matches!(c.trim_start_matches('+'), "cpu" | "pids"))
            });
            if !split {
                continue;
            }
            // Separate writes: one controller a leaf still passes down (the engine's) must not
            // keep the other.
            for line in ["-pids", "-cpu"] {
                if let Err(error) = write(&control, line) {
                    tracing::warn!(group = %entry.path().display(), %error, "cannot undo the split ({line})");
                }
            }
        }
    }
}

/// Creates the group `dir` when it is gone; returns whether it did (SPEC-201 DBX-005, #614).
/// `yulius.service` stops with `KillMode=process`, and systemd then removes every empty group
/// left in the unit, the leaf of an idle engine among them, while its holder and front run on.
/// Only `dir` is created: without its parent (the sandbox's group, which holds the limits) it
/// fails. A leaf made again under a split parent gets the default `cpu.weight`, 100, which is the
/// engine's (`LEAVES`).
///
/// # Errors
/// `dir` has a `..`, the parent is gone, or the group cannot be created.
pub fn restore_leaf(dir: &Path) -> io::Result<bool> {
    if dir.components().any(|c| c.as_os_str() == "..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a cgroup directory",
        ));
    }
    match std::fs::create_dir(dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

/// Moves the calling process into the cgroup directory `dir` (its children follow).
///
/// # Errors
/// `dir` is not a cgroup directory under [`CGROUP_ROOT`], or the move is refused.
pub fn join(dir: &Path) -> io::Result<()> {
    write_procs(dir, "0")
}

/// Moves the process `pid` into the cgroup directory `dir`.
///
/// # Errors
/// As [`join`].
pub fn move_pid(dir: &Path, pid: u32) -> io::Result<()> {
    write_procs(dir, &pid.to_string())
}

fn write_procs(dir: &Path, pid: &str) -> io::Result<()> {
    if !dir.starts_with(CGROUP_ROOT) || dir.components().any(|c| c.as_os_str() == "..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a cgroup directory",
        ));
    }
    write(&dir.join("cgroup.procs"), pid)
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
        let limits = Limits::defaults(48 << 30, None);
        assert_eq!(limits.memory_max, Some(42 << 30), "all but an eighth");
        assert_eq!(
            Limits::defaults(16 << 30, None).memory_max,
            Some(12 << 30),
            "a 4 GiB reserve at least"
        );
        assert_eq!(limits.cpus, None, "shared by weight, not capped (SPEC-082)");
        assert_eq!(limits.files()[1], ("cpu.max", "max 100000".to_owned()));
        assert_eq!(
            limits.pids_max,
            Some(8192),
            "the host's task table is unknown"
        );
        let small = Limits::defaults(2 << 30, None);
        assert_eq!(small.memory_max, Some(2 << 30));
        assert_eq!(
            Limits::default().files(),
            vec![
                ("memory.max", "max".to_owned()),
                ("cpu.max", "max 100000".to_owned()),
                ("pids.max", "max".to_owned()),
            ]
        );
        let capped = Limits {
            cpus: Some(10),
            ..limits
        };
        assert_eq!(capped.files()[1], ("cpu.max", "1000000 100000".to_owned()));
    }

    /// SPEC-268 PID-001: a quarter of the host's task table, never less than the old 8192.
    #[test]
    fn the_task_limit_is_sized_from_the_host() {
        // The VM: 47 GB, threads-max 385 088, pid_max 4 194 304.
        let host = host_tasks("385088\n", "4194304\n");
        assert_eq!(host, Some(385_088));
        assert_eq!(Limits::defaults(48 << 30, host).pids_max, Some(96_272));
        assert_eq!(host_tasks("385088\n", "32768\n"), Some(32_768));
        assert_eq!(
            Limits::defaults(4 << 30, host_tasks("30000", "32768")).pids_max,
            Some(8192),
            "a small host keeps 8192"
        );
        assert_eq!(host_tasks("", "32768"), None);
        assert_eq!(host_tasks("many", "32768"), None);
        assert_eq!(host_tasks("0", "32768"), None);
    }

    /// SPEC-268 PID-001, review round 1: systemd's `DefaultTasksMax` (15 % of the host's table)
    /// caps the whole service; a sandbox's default stays at three quarters of it, so the server
    /// and the other sandboxes keep a quarter.
    #[test]
    fn the_task_limit_stays_below_the_services() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let unit = root.path().join("system.slice/yulius.service");
        std::fs::create_dir_all(&unit)?;
        std::fs::write(root.path().join("system.slice/pids.max"), "max\n")?;
        assert_eq!(tasks_max_above(root.path(), &unit), None, "no limit");
        std::fs::write(unit.join("pids.max"), "57763\n")?;
        assert_eq!(tasks_max_above(root.path(), &unit), Some(57_763));
        std::fs::write(root.path().join("system.slice/pids.max"), "40000\n")?;
        assert_eq!(
            tasks_max_above(root.path(), &unit),
            Some(40_000),
            "an ancestor's"
        );
        assert_eq!(tasks_max_above(root.path(), root.path()), None);
        let vm = Limits::defaults(48 << 30, Some(385_088));
        assert_eq!(vm.within_service(Some(57_763)).pids_max, Some(43_320));
        assert_eq!(vm.within_service(Some(1_000_000)).pids_max, Some(96_272));
        assert_eq!(vm.within_service(None), vm);
        assert_eq!(
            Limits::default().within_service(Some(100)),
            Limits::default(),
            "no limit stays none"
        );
        Ok(())
    }

    /// SPEC-272 SHR-001: all sandboxes together keep the server a quarter of the service's tasks;
    /// without a service limit the common one is `max` again, not what an earlier server wrote
    /// (`<unit>/sandboxes` survives restarts with `KillMode=process`).
    #[test]
    fn the_sandboxes_together_leave_the_server_a_quarter() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(sandboxes_tasks(Some(57_763)), Some(43_320));
        assert_eq!(sandboxes_tasks(Some(1)), Some(1));
        assert_eq!(sandboxes_tasks(None), None);
        let (_root, delegation, _dir) = fake_sandbox("cpu memory pids\n")?;
        let common = delegation.unit.join(SANDBOXES).join("pids.max");
        delegation.limit_sandboxes(Some(57_763))?;
        assert_eq!(std::fs::read_to_string(&common)?, "43320");
        delegation.limit_sandboxes(None)?;
        assert_eq!(std::fs::read_to_string(&common)?, "max", "reset, not stale");
        // Without the `pids` controller there is no such file to write.
        let (_root, delegation, _dir) = fake_sandbox("cpu memory\n")?;
        delegation.limit_sandboxes(Some(57_763))?;
        assert!(!delegation.unit.join(SANDBOXES).join("pids.max").exists());
        Ok(())
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
        let group = delegation
            .sandbox_group(
                "s1",
                &Limits {
                    memory_max: Some(1 << 30),
                    cpus: Some(2),
                    pids_max: Some(64),
                },
            )?
            .ci
            .parent()
            .map(Path::to_path_buf)
            .ok_or("a leaf has a parent")?;
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

    /// SPEC-201 DBX-005: a pruned leaf is made again, an existing one left alone, and a missing
    /// parent is never made up.
    #[test]
    fn a_pruned_leaf_is_restored_under_its_parent() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let leaf = root.path().join("s1/engine");
        assert!(restore_leaf(&leaf).is_err(), "no parent");
        assert!(!root.path().join("s1").exists());
        std::fs::create_dir(root.path().join("s1"))?;
        assert!(restore_leaf(&leaf)?);
        assert!(leaf.is_dir());
        std::fs::write(leaf.join("cgroup.procs"), "")?;
        assert!(!restore_leaf(&leaf)?, "already there");
        assert!(leaf.join("cgroup.procs").exists(), "left as it was");
        // Review round 1 (nit): no `..`, as `join` refuses.
        let around = root.path().join("s1/../s2");
        assert!(restore_leaf(&around).is_err());
        assert!(!root.path().join("s2").exists());
        Ok(())
    }

    /// A delegated unit whose `sandboxes/<sandbox>` group offers `controllers`.
    fn fake_sandbox(
        controllers: &str,
    ) -> Result<(tempfile::TempDir, Delegation, PathBuf), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let unit = root.path().join("u");
        std::fs::create_dir_all(&unit)?;
        std::fs::write(unit.join("cgroup.procs"), "")?;
        std::fs::write(unit.join("cgroup.controllers"), controllers)?;
        let delegation = prepare(&unit)?;
        let dir = unit.join(SANDBOXES).join("s1");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("cgroup.procs"), "")?;
        Ok((root, delegation, dir))
    }

    #[test]
    fn a_sandbox_group_splits_into_weighted_leaves() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(
            groups,
            SandboxGroups {
                system: dir.join("system"),
                agents: dir.join("agents"),
                ci: dir.join("ci"),
                engine: dir.join("engine"),
            }
        );
        // Only the CPU goes down to the leaves; the limits stay on the sandbox.
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "+cpu"
        );
        assert_eq!(std::fs::read_to_string(dir.join("cpu.max"))?, "max 100000");
        for (leaf, weight) in [
            ("system", "100"),
            ("agents", "100"),
            ("ci", "400"),
            ("engine", "100"),
        ] {
            assert_eq!(
                std::fs::read_to_string(dir.join(leaf).join("cpu.weight"))?,
                weight,
                "{leaf}"
            );
        }
        // Preparing again (every run does) changes nothing.
        assert_eq!(delegation.sandbox_group("s1", &Limits::default())?, groups);
        // SPEC-196 review round 2: the same paths, read without preparing anything.
        assert_eq!(delegation.sandbox_paths("s1"), groups);
        Ok(())
    }

    /// SPEC-268 PID-002, PID-003: CI jobs and the engine each get 3/8 of the sandbox's tasks, so
    /// the agents' leaf keeps a quarter; a stopping server takes them back with the CPU.
    #[test]
    fn ci_and_the_engine_leave_the_agents_a_reserve_of_tasks()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let limits = Limits {
            pids_max: Some(64),
            ..Limits::default()
        };
        let groups = delegation.sandbox_group("s1", &limits)?;
        assert_eq!(std::fs::read_to_string(dir.join("pids.max"))?, "64");
        // A fake file keeps the last write: `+cpu`, then `+pids`.
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "+pids"
        );
        assert_eq!(std::fs::read_to_string(groups.ci.join("pids.max"))?, "24");
        assert_eq!(
            std::fs::read_to_string(groups.engine.join("pids.max"))?,
            "24"
        );
        assert!(
            !groups.agents.join("pids.max").exists(),
            "the agents keep the rest"
        );
        assert!(!groups.system.join("pids.max").exists());
        assert_eq!(
            std::fs::read_to_string(groups.ci.join("cpu.weight"))?,
            "400"
        );
        delegation.unsplit_all();
        // `-pids`, then `-cpu` (kept by the fake file).
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "-cpu"
        );
        // A tiny limit still leaves each leaf a task.
        let (_root, delegation, _dir) = fake_sandbox("cpu memory pids\n")?;
        let tiny = Limits {
            pids_max: Some(2),
            ..Limits::default()
        };
        let groups = delegation.sandbox_group("s1", &tiny)?;
        assert_eq!(std::fs::read_to_string(groups.ci.join("pids.max"))?, "1");
        Ok(())
    }

    /// SPEC-268 PID-004, review rounds 1 and 2. Kernel 6.8 counts a refused fork in the forking
    /// task's own group: every group of the sandbox but CI's and the engine's counts, also a
    /// paused run's `agents/run-*` group. Kernels with `pids.events.local` (6.13+) count where the
    /// limit was hit: the sandbox's own limit counts. OOM kills are the sandbox's (hierarchical).
    #[test]
    fn the_limit_counters_count_what_sessions_meet() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        assert_eq!(delegation.limit_counters("s1"), LimitCounters::default());
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(dir.join("pids.events"), "max 2\n")?;
        std::fs::write(groups.agents.join("pids.events"), "max 3\n")?;
        std::fs::create_dir_all(groups.agents.join("run-t1"))?;
        std::fs::write(groups.agents.join("run-t1/pids.events"), "max 7\n")?;
        std::fs::write(groups.system.join("pids.events"), "max 1\n")?;
        std::fs::write(groups.ci.join("pids.events"), "max 11\n")?;
        std::fs::create_dir_all(groups.ci.join("job"))?;
        std::fs::write(groups.ci.join("job/pids.events"), "max 17\n")?;
        std::fs::create_dir_all(groups.engine.join("docker/c1"))?;
        std::fs::write(groups.engine.join("pids.events"), "max 13\n")?;
        std::fs::write(groups.engine.join("docker/c1/pids.events"), "max 5\n")?;
        std::fs::write(
            dir.join("memory.events"),
            "low 0\nhigh 0\nmax 9\noom 1\noom_kill 1\noom_group_kill 0\n",
        )?;
        std::fs::write(groups.ci.join("memory.events"), "oom_kill 7\n")?;
        assert_eq!(
            delegation.limit_counters("s1"),
            LimitCounters {
                task_limit_hits: 13,
                oom_kills: 1
            }
        );
        // 6.13+: the sandbox's own limit, hit by any of its processes.
        std::fs::write(dir.join("pids.events.local"), "max 4\n")?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 4);
        // Without the split everything but the engine runs in the sandbox's group.
        let (_root, delegation, dir) = fake_sandbox("memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(dir.join("pids.events"), "max 4\n")?;
        std::fs::write(groups.engine.join("pids.events"), "max 9\n")?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 4);
        assert_eq!(delegation.limit_counters("../s1"), LimitCounters::default());
        Ok(())
    }

    /// SPEC-272 SHR-003 (SPEC-268 review round 3): on kernels with `pids.events.local` a fork
    /// refused at the common limit is counted in `sandboxes/` only, and must still count; with
    /// `pids_localevents` the forking groups count, as on 6.8, but CI's and the engine's.
    #[test]
    fn a_hit_at_the_common_limit_counts_for_the_sandbox() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        let common = delegation.unit.join(SANDBOXES);
        std::fs::write(dir.join("pids.events.local"), "max 0\n")?;
        std::fs::write(common.join("pids.events.local"), "max 5\n")?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 5);
        // `pids_localevents`: `max` counts the forks refused to each group's own tasks.
        std::fs::write(groups.agents.join("pids.events.local"), "max 3\n")?;
        std::fs::create_dir_all(groups.agents.join("run-t1"))?;
        std::fs::write(groups.agents.join("run-t1/pids.events.local"), "max 2\n")?;
        std::fs::write(groups.ci.join("pids.events.local"), "max 11\n")?;
        std::fs::create_dir_all(groups.engine.join("docker/c1"))?;
        std::fs::write(groups.engine.join("docker/c1/pids.events.local"), "max 13\n")?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 10);
        // On 6.8 (no `.local` file) `sandboxes/` holds no task and is not read.
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(groups.agents.join("pids.events"), "max 3\n")?;
        std::fs::write(delegation.unit.join(SANDBOXES).join("pids.events"), "max 5\n")?;
        assert!(!dir.join("pids.events.local").exists());
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 3);
        Ok(())
    }

    /// Without the split the runs live in the sandbox's group, which cannot pass `pids` down.
    #[test]
    fn without_the_split_the_leaves_get_no_task_limit() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("memory pids\n")?;
        let limits = Limits {
            pids_max: Some(64),
            ..Limits::default()
        };
        let groups = delegation.sandbox_group("s1", &limits)?;
        assert_eq!(std::fs::read_to_string(dir.join("pids.max"))?, "64");
        assert!(!groups.engine.join("pids.max").exists());
        assert!(!dir.join("cgroup.subtree_control").exists());
        // No `pids` controller: the CPU split alone.
        let (_root, delegation, dir) = fake_sandbox("cpu memory\n")?;
        let groups = delegation.sandbox_group("s1", &limits)?;
        assert!(!groups.ci.join("pids.max").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "+cpu"
        );
        Ok(())
    }

    #[test]
    fn processes_in_the_sandbox_group_move_to_agents_first()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        // A holder and a run from before the split.
        std::fs::write(dir.join("cgroup.procs"), "11\n12\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        // A fake file keeps the last write; the kernel moves each process on its write.
        assert_eq!(
            std::fs::read_to_string(groups.agents.join("cgroup.procs"))?,
            "12"
        );
        assert!(!groups.system.join("cgroup.procs").exists());
        assert!(!groups.ci.join("cgroup.procs").exists());
        Ok(())
    }

    #[test]
    fn a_split_that_cannot_be_enabled_still_gives_leaves() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        // The kernel refuses `+cpu` while a process is in the group (EBUSY); a directory in the
        // file's place refuses every write the same way.
        std::fs::create_dir_all(dir.join("cgroup.subtree_control"))?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(groups.ci, dir.join("ci"));
        assert!(groups.ci.is_dir() && groups.agents.is_dir() && groups.system.is_dir());
        assert!(!groups.ci.join("cpu.weight").exists());
        Ok(())
    }

    #[test]
    fn a_stopping_server_undoes_the_split_and_the_next_one_redoes_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        delegation.sandbox_group("s1", &Limits::default())?;
        // A sandbox that was never split is left alone.
        let other = dir.with_file_name("s2");
        std::fs::create_dir_all(&other)?;
        delegation.unsplit_all();
        // An older binary joins the sandbox group itself, which `+cpu` would refuse (EBUSY).
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "-cpu"
        );
        assert!(!other.join("cgroup.subtree_control").exists());
        // A run that starts while the server stops still gets its leaf, but no split again.
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(groups.ci, dir.join("ci"));
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "-cpu"
        );
        // The next server splits again.
        let unit = dir.parent().and_then(Path::parent).ok_or("unit")?;
        prepare(unit)?.sandbox_group("s1", &Limits::default())?;
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "+cpu"
        );
        Ok(())
    }

    #[test]
    fn a_process_moves_only_into_a_cgroup_directory() {
        assert!(move_pid(Path::new("/tmp/x"), 1).is_err());
        assert!(move_pid(Path::new("/sys/fs/cgroup/../etc"), 1).is_err());
    }

    #[test]
    fn without_the_cpu_controller_there_is_no_split() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(
            groups,
            SandboxGroups {
                system: dir.clone(),
                agents: dir.clone(),
                ci: dir.clone(),
                engine: dir.join("engine"),
            }
        );
        assert!(!dir.join("ci").exists());
        // The engine's containers still get a group of their own below the sandbox's (SPEC-187
        // DKR-005), without controllers passed down.
        assert!(dir.join("engine").is_dir());
        assert!(!dir.join("cgroup.subtree_control").exists());
        Ok(())
    }
}
