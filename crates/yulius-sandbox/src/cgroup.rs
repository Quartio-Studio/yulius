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
//!   keeps a quarter however many sandboxes are busy (SPEC-272);
//! - weighs each sandbox group by the CPUs its running CI jobs hold ([`sandbox_weight`]), so on a
//!   loaded host a sandbox's CI gets about what it holds against other sandboxes' agents
//!   (SPEC-362, SPEC-497 HBA-004);
//! - reads what each sandbox's CI and Docker engine use ([`Delegation::ci_use`], SPEC-497
//!   HBA-002).
//!
//! The holder, `slirp4netns` and every run of a sandbox join their leaf before they start
//! anything ([`join`]). Without delegation (the parent of the server's cgroup is not ours),
//! nothing changes and sandboxes run without limits.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Where cgroup v2 is mounted.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";
/// Prefix of a CI job's own group below the sandbox's CI group (SPEC-504 CLB-001).
pub const CI_JOB_GROUP_PREFIX: &str = "ci-job-";
/// Longest CI job id a group is named after.
const MAX_JOB_ID: usize = 64;
/// How long an emptied CI job group may stay before the sweep removes it: a job joins its group
/// seconds after it was made, and the job's own end removes it (SPEC-504 CLB-001).
pub const CI_JOB_GROUP_KEPT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// Controllers enabled for sandboxes (those available are used).
const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];
/// Processes moved out of the unit's root at most (a bound for a pathological unit).
const MAX_MOVED: usize = 4096;
/// The leaf for processes moved out of a service unit, including a tunnel custodian.
pub const LEGACY_LEAF: &str = "legacy";
/// The parent of sandbox groups.
pub(crate) const SANDBOXES: &str = "sandboxes";
/// A sandbox's leaves and their `cpu.weight` (SPEC-053 CPU-201): CI gets four times the agents'
/// share, so up to 80 % of the sandbox under full contention while the holder and the engine
/// idle. The engine runs below its group (SPEC-187 DKR-005), in [`ENGINE_ROOT_LEAF`], the root of
/// its cgroup namespace: containers get groups below that.
const LEAVES: [(&str, u32); 4] = [
    ("system", 100),
    ("agents", 100),
    ("ci", 400),
    ("engine", 100),
];
/// Groups one walk visits at most.
pub(crate) const MAX_GROUPS: usize = 4096;
/// How deep below the unit a group may lie (`sandboxes/<sandbox>/<leaf>/run-<name>` is 4).
pub(crate) const MAX_DEPTH: usize = 8;
/// The engine's group, also without the `cpu` split: it holds the engine's limits.
pub(crate) const ENGINE_LEAF: &str = "engine";
/// The group below [`ENGINE_LEAF`] the engine runs in and sees as its `/sys/fs/cgroup`, the root
/// of its cgroup namespace (SPEC-483 C4). A process of the engine that moves to a group of its
/// own and starts a cgroup namespace there may write the files of every group in its view but
/// its namespace's root, the kernel's only check (`nsdelegate`); the limits are therefore one
/// group up, out of its view. `cpu` and `pids` pass down to it as they reached the engine before
/// (`settle_engine_group`); an engine still running in [`ENGINE_LEAF`] itself from an earlier
/// version keeps starting there until its front starts again.
pub(crate) const ENGINE_ROOT_LEAF: &str = "inside";
/// A sandbox group's `cpu.weight` while none of its CI jobs runs (the kernel's default).
const BASE_WEIGHT: u32 = 100;
/// What running CI jobs add to their sandbox group's weight at least (SPEC-362 CSW-002): what the
/// `ci` leaf weighs in its sandbox, so the sandbox's CI weighs on the host what four sandboxes
/// that only run agents weigh.
const CI_WEIGHT: u32 = 400;
/// What each CPU the sandbox's running CI jobs hold adds to its weight (SPEC-497 HBA-004): the
/// weight of one sandbox that only runs agents. The CI budget admitted those CPUs and keeps an
/// eighth of the host from CI, so a sandbox holding `c` of them gets `(1 + c) / (1 + c + n)` of
/// the host against `n` sandboxes whose agents build: at least its CPUs while `n` is below about
/// `(host - c)(1 + c) / c` (35 others for 30 of 64 CPUs), more when fewer build, and those agents
/// keep a share each. Not per job (SPEC-362 review round 1): four 1-CPU jobs weigh
/// what one 4-CPU job weighs.
const CI_WEIGHT_PER_CPU: u32 = 100;
/// The kernel's largest `cpu.weight`.
const MAX_WEIGHT: u32 = 10_000;

/// The `cpu.weight` of a sandbox group while `jobs` of its CI jobs run, holding `cpus` CPUs of
/// the CI budget (SPEC-362 CSW-002, SPEC-497 HBA-004).
///
/// # Examples
/// ```
/// use yulius_sandbox::cgroup::sandbox_weight;
/// assert_eq!(sandbox_weight(0, 0), 100);
/// assert_eq!(sandbox_weight(1, 2), 500);
/// assert_eq!(sandbox_weight(4, 4), 500);
/// assert_eq!(sandbox_weight(1, 30), 3100);
/// assert_eq!(sandbox_weight(2, 500), 10_000);
/// ```
#[must_use]
pub fn sandbox_weight(jobs: u32, cpus: u32) -> u32 {
    if jobs == 0 {
        BASE_WEIGHT
    } else {
        BASE_WEIGHT
            .saturating_add(CI_WEIGHT_PER_CPU.saturating_mul(cpus))
            .clamp(BASE_WEIGHT + CI_WEIGHT, MAX_WEIGHT)
    }
}

/// What a sandbox's CI jobs and its Docker engine (with its containers) used, read from their
/// groups (SPEC-497 HBA-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CiUse {
    /// CPU time since the groups were made, in microseconds (`cpu.stat` `usage_usec`).
    pub cpu_usec: u64,
    /// Anonymous memory now, in bytes (`memory.stat` `anon`): not the page cache, which the
    /// kernel gives back.
    pub anon_bytes: u64,
    /// Thousandths of the last 10 seconds for which processes of the `ci` group waited for a CPU
    /// (`cpu.pressure` `some avg10`); `None` without the file.
    pub cpu_pressure: Option<u32>,
}

/// The `some avg10` of a `cpu.pressure` file, in thousandths of the last 10 seconds.
fn some_avg10(text: &str) -> Option<u32> {
    let line = text.lines().find(|line| line.starts_with("some "))?;
    let percent: f64 = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse()
        .ok()?;
    if !percent.is_finite() || percent < 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a percentage between 0 and 100, in thousandths"
    )]
    let thousandths = (percent * 10.0).round().min(1000.0) as u32;
    Some(thousandths)
}

/// Writes the default weight to every sandbox group under `sandboxes` that has one: a starting
/// server knows of no CI job, and one that crashed left its weights (SPEC-362 CSW-003, CSW-004).
fn reset_weights(sandboxes: &Path) {
    let Ok(entries) = std::fs::read_dir(sandboxes) else {
        return;
    };
    for entry in entries.flatten().take(MAX_MOVED) {
        let weight = entry.path().join("cpu.weight");
        if weight.exists()
            && let Err(error) = write(&weight, &BASE_WEIGHT.to_string())
        {
            tracing::warn!(group = %entry.path().display(), %error, "cannot reset the sandbox's weight");
        }
    }
}

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
        Self {
            memory_max: Some(yulius_domain::limits::sandbox_memory(total_memory)),
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
    /// CI jobs running per sandbox and the CPUs they hold ([`Delegation::set_ci_jobs`]; shared
    /// by clones).
    ci_jobs: Arc<Mutex<HashMap<String, (u32, u32)>>>,
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

/// Whether this user may place processes in the group `dir` and hand controllers to its children.
fn delegated_to_us(dir: &Path) -> bool {
    [
        dir.join("cgroup.procs"),
        dir.join("cgroup.subtree_control"),
        dir.to_path_buf(),
    ]
    .iter()
    .all(|p| writable(p))
}

/// The sessions unit beside the server's unit `parent` (ADR-0298), when the host has one this
/// user can use: its group is delegated to us, and so is `cgroup.procs` of the slice both units
/// share, which the kernel checks when a process moves from one unit to the other.
fn sessions_unit(fs_root: &Path, parent: &str) -> Option<PathBuf> {
    let (slice, service) = parent.rsplit_once('/')?;
    let unit = fs_root
        .join(slice)
        .join(crate::units::sessions_unit(service)?);
    (delegated_to_us(&unit) && writable(&fs_root.join(slice).join("cgroup.procs"))).then_some(unit)
}

/// The group sandboxes live in, for a server whose cgroup is `own` (a path relative to
/// `fs_root`): the sessions unit beside the server's unit when the host has one (ADR-0298), else
/// the server's own unit when the parent of `own` is ours (`DelegateSubgroup=` placed the server
/// one level down).
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
    if let Some(unit) = sessions_unit(fs_root, parent) {
        return Ok(unit);
    }
    let unit = fs_root.join(parent);
    if delegated_to_us(&unit) {
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
    reset_weights(&sandboxes);
    Ok(Delegation {
        unit: unit.to_path_buf(),
        controllers,
        stopping: Arc::new(Mutex::new(false)),
        ci_jobs: Arc::default(),
    })
}

/// The name of CI job `job`'s own group (SPEC-504 CLB-001): [`CI_JOB_GROUP_PREFIX`] and the id,
/// `None` for an id that is not 1 to 64 ASCII letters, digits, `-` and `_`.
#[must_use]
pub fn ci_job_group_name(job: &str) -> Option<String> {
    let plain = !job.is_empty()
        && job.len() <= MAX_JOB_ID
        && job
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| format!("{CI_JOB_GROUP_PREFIX}{job}"))
}

/// The CI job whose own group is the cgroup `path` (`/system.slice/…/ci/ci-job-<id>`, as
/// `/proc/<pid>/cgroup` gives it): the id, when the group's name is one
/// [`ci_job_group_name`] makes. Any other group is no CI job's.
#[must_use]
pub fn ci_job_of(path: &str) -> Option<String> {
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let job = last.strip_prefix(CI_JOB_GROUP_PREFIX)?;
    ci_job_group_name(job).map(|_| job.to_owned())
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
    /// The Docker engine and, below it, its containers (ADR-0151); always a group of its own,
    /// which holds the engine's limits.
    pub engine: PathBuf,
    /// Where the engine runs, below [`SandboxGroups::engine`]: its cgroup namespace's root
    /// (`inside`).
    pub engine_root: PathBuf,
}

impl SandboxGroups {
    fn single(dir: &Path) -> Self {
        Self {
            system: dir.to_path_buf(),
            agents: dir.to_path_buf(),
            ci: dir.to_path_buf(),
            engine: dir.join(ENGINE_LEAF),
            engine_root: dir.join(ENGINE_LEAF).join(ENGINE_ROOT_LEAF),
        }
    }

    fn leaves(dir: &Path) -> Self {
        let [system, agents, ci, engine] = LEAVES.map(|(leaf, _)| dir.join(leaf));
        Self {
            system,
            agents,
            ci,
            engine_root: engine.join(ENGINE_ROOT_LEAF),
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
    /// Forks and thread creations refused at a task limit the sandbox's sessions are under
    /// ([`Delegation::limit_counters`]).
    pub task_limit_hits: u64,
    /// Processes the OOM killer ended in the sandbox (`memory.events` `oom_kill`).
    pub oom_kills: u64,
}

/// The number after `key` in a cgroup's flat keyed file (`max 3\n`), 0 when absent.
pub(crate) fn keyed(text: &str, key: &str) -> u64 {
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
    /// SHR-001). Returns whether it was written: not without the `pids` controller.
    ///
    /// # Errors
    /// The limit cannot be written.
    pub fn limit_sandboxes(&self, service: Option<u64>) -> io::Result<bool> {
        if !self.controllers.iter().any(|c| c == "pids") {
            return Ok(false);
        }
        let limit = sandboxes_tasks(service).map_or_else(|| "max".to_owned(), |n| n.to_string());
        write(&self.unit.join(SANDBOXES).join("pids.max"), &limit)?;
        Ok(true)
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

    /// `sandbox`'s limit counters (SPEC-268 PID-004, review rounds 1 and 2; SPEC-272 SHR-003;
    /// SPEC-316 LIM-001).
    /// Refused forks are `max` in `pids.events` of every group of the sandbox (at most 4096) but
    /// CI's and the engine's subtrees, whose processes are stopped at their own leaf limits while
    /// sessions keep forking; a paused run's `agents/run-*` group counts. Kernel 6.8 counts a
    /// refused fork there, in the forking task's own group, wherever the limit was. Where the
    /// sandbox has `pids.events.local` (6.13+) the same groups' `.local` files are read instead,
    /// and those of `<unit>/sandboxes` and `<unit>` (SPEC-316): by default `max` there counts
    /// where a limit was hit (the sandbox's own, the common one of all sandboxes or the
    /// service's, both shared), with `pids_localevents` the forking group, as on 6.8. Slices
    /// above the unit are outside the delegation and not read. OOM kills are the sandbox's
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
            [self.unit.join(SANDBOXES).as_path(), self.unit.as_path()]
                .into_iter()
                .map(|group| read(group, events, "max").unwrap_or(0))
                .fold(0, u64::saturating_add)
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
            settle_engine_group(&groups.engine);
            std::fs::create_dir_all(&groups.engine_root)?;
            return Ok(groups);
        }
        let groups = SandboxGroups::leaves(&dir);
        for leaf in [
            &groups.system,
            &groups.agents,
            &groups.ci,
            &groups.engine_root,
        ] {
            std::fs::create_dir_all(leaf)?;
        }
        // Joining a leaf works without the weights too; the next preparation tries again.
        // A failure that persists (a process that cannot be moved) logs at every preparation.
        let stopping = self.stopping.lock().unwrap_or_else(PoisonError::into_inner);
        if *stopping {
            return Ok(groups);
        }
        // Not while the server stops: it takes the engine's controllers back (`unsplit_all`).
        settle_engine_group(&groups.engine);
        if let Err(error) = split_cpu(&dir, &groups) {
            tracing::warn!(sandbox, %error, "CI jobs run without CPU priority for now");
        }
        if let Err(error) = write(
            &dir.join("cpu.weight"),
            &self.weight_of(sandbox).to_string(),
        ) {
            tracing::warn!(sandbox, %error, "the sandbox's CI jobs run without their weight on the host for now");
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

    /// The weight of `sandbox`'s group for its CI jobs running now.
    fn weight_of(&self, sandbox: &str) -> u32 {
        let jobs = self.ci_jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let (jobs, cpus) = jobs.get(sandbox).copied().unwrap_or((0, 0));
        sandbox_weight(jobs, cpus)
    }

    /// What `sandbox`'s CI jobs and Docker engine use now ([`CiUse`]): its `ci` and `engine`
    /// groups' figures added up (hierarchical, so the engine's containers count). `None` without
    /// the `cpu` controller (the leaves are not split then) or when a complete sample cannot be read.
    #[must_use]
    pub fn ci_use(&self, sandbox: &str) -> Option<CiUse> {
        if !self.controllers.iter().any(|c| c == "cpu") {
            return None;
        }
        let groups = self.sandbox_paths(sandbox);
        let mut used = None;
        for group in [&groups.ci, &groups.engine] {
            let cpu = match std::fs::read_to_string(group.join("cpu.stat")) {
                Ok(cpu) => cpu,
                Err(err)
                    if err.kind() == io::ErrorKind::NotFound
                        && group.try_exists().ok() == Some(false) =>
                {
                    continue;
                }
                Err(_) => return None,
            };
            let anon = std::fs::read_to_string(group.join("memory.stat"))
                .map_or(0, |text| keyed(&text, "anon"));
            let sum: &mut CiUse = used.get_or_insert_with(CiUse::default);
            sum.cpu_usec = sum.cpu_usec.saturating_add(keyed(&cpu, "usage_usec"));
            sum.anon_bytes = sum.anon_bytes.saturating_add(anon);
        }
        if let Some(used) = used.as_mut() {
            used.cpu_pressure = std::fs::read_to_string(groups.ci.join("cpu.pressure"))
                .ok()
                .as_deref()
                .and_then(some_avg10);
        }
        used
    }

    /// Makes CI job `job`'s own group below `sandbox`'s CI group and returns it (SPEC-504
    /// CLB-001): the job's processes join it, so the Docker front knows their calls as the job's.
    /// It holds no limit of its own: the CI group's apply to it.
    ///
    /// # Errors
    /// An id [`ci_job_group_name`] refuses, or the group cannot be made (the sandbox's CI group
    /// is not there yet).
    pub fn ci_job_group(&self, sandbox: &str, job: &str) -> io::Result<PathBuf> {
        let name = ci_job_group_name(job).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "not a CI job id for a group")
        })?;
        let group = self.sandbox_paths(sandbox).ci.join(name);
        match std::fs::create_dir(&group) {
            Ok(()) => Ok(group),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // The same job again (a requeued run, a rerun) after its group stayed: made
                // fresh, so the sweep does not take it as an old empty one before the job joins
                // (review round 1). One that still holds a process cannot go, and the sweep
                // keeps it while it does.
                if std::fs::remove_dir(&group).is_ok() {
                    std::fs::create_dir(&group)?;
                }
                Ok(group)
            }
            Err(error) => Err(error),
        }
    }

    /// Removes CI job `job`'s group once the job ended; one that still holds a process stays
    /// (the sweep, [`Delegation::sweep_ci_job_groups`], takes it once it is empty).
    pub fn end_ci_job_group(&self, sandbox: &str, job: &str) {
        if let Some(name) = ci_job_group_name(job) {
            let _ = std::fs::remove_dir(self.sandbox_paths(sandbox).ci.join(name));
        }
    }

    /// Whether CI job `job` of `sandbox` still runs (SPEC-504 CLB-005): `Some(false)` when its
    /// group is gone or holds no process, `Some(true)` while one runs there, `None` when that
    /// cannot be told (its group's `cgroup.events` cannot be read), which callers take as running.
    #[must_use]
    pub fn ci_job_running(&self, sandbox: &str, job: &str) -> Option<bool> {
        let Some(name) = ci_job_group_name(job) else {
            return Some(false);
        };
        let group = self.sandbox_paths(sandbox).ci.join(name);
        match std::fs::read_to_string(group.join("cgroup.events")) {
            Ok(_) => Some(populated(&group)),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !group.exists() => Some(false),
            Err(_) => None,
        }
    }

    /// Removes `sandbox`'s CI job groups that hold no process and were made over
    /// [`CI_JOB_GROUP_KEPT`] ago: those a server that stopped during a job left (SPEC-504
    /// CLB-001). A group made a moment ago, which its job is about to join, stays.
    pub fn sweep_ci_job_groups(&self, sandbox: &str) {
        let ci = self.sandbox_paths(sandbox).ci;
        for group in subgroups(&ci) {
            let ours = group
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| ci_job_of(name).is_some());
            let old = std::fs::metadata(&group)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|made| made.elapsed().ok())
                .is_some_and(|age| age >= CI_JOB_GROUP_KEPT);
            if ours && old && !populated(&group) {
                let _ = std::fs::remove_dir(&group);
            }
        }
    }

    /// CI jobs of `sandbox` running now ([`Delegation::set_ci_jobs`]).
    #[must_use]
    pub fn ci_jobs_of(&self, sandbox: &str) -> u32 {
        self.ci_jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sandbox)
            .map_or(0, |(jobs, _)| *jobs)
    }

    /// Records that `jobs` CI jobs of `sandbox` run now, holding `cpus` CPUs, and writes its
    /// group's `cpu.weight` ([`sandbox_weight`], SPEC-362 CSW-002, SPEC-497 HBA-004). Returns
    /// whether it was written: not without the `cpu` controller, nor once the server stops, nor
    /// before the sandbox's group exists (its first preparation writes the weight then).
    ///
    /// # Errors
    /// The weight cannot be written.
    pub fn set_ci_jobs(&self, sandbox: &str, jobs: u32, cpus: u32) -> io::Result<bool> {
        {
            let mut counts = self.ci_jobs.lock().unwrap_or_else(PoisonError::into_inner);
            if jobs == 0 {
                counts.remove(sandbox);
            } else {
                counts.insert(sandbox.to_owned(), (jobs, cpus));
            }
        }
        if !self.controllers.iter().any(|c| c == "cpu") {
            return Ok(false);
        }
        let stopping = self.stopping.lock().unwrap_or_else(PoisonError::into_inner);
        if *stopping {
            return Ok(false);
        }
        let dir = self
            .unit
            .join(SANDBOXES)
            .join(crate::manager::path_name(sandbox));
        if !dir.is_dir() {
            return Ok(false);
        }
        write(
            &dir.join("cpu.weight"),
            &self.weight_of(sandbox).to_string(),
        )?;
        drop(stopping);
        Ok(true)
    }

    /// Takes the task limits and the CPU back from every sandbox's leaves (`-pids`, `-cpu`) when
    /// the server stops (SPEC-053 CPU-205, SPEC-268 PID-003). An older binary started after this
    /// one (a rollback) joins the sandbox group itself, which the kernel refuses while the group
    /// passes a controller down. Processes stay in their leaves; the next preparation splits
    /// again.
    pub fn unsplit_all(&self) {
        let mut stopping = self.stopping.lock().unwrap_or_else(PoisonError::into_inner);
        *stopping = true;
        // The default weight for whatever runs next (SPEC-362 CSW-004).
        reset_weights(&self.unit.join(SANDBOXES));
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
            // The engine's group passes `cpu` and `pids` to its root (SPEC-483 C4), and the
            // kernel takes a controller from no group while a child still passes it on: the
            // engine's group gives them back first. Where an engine ran containers, its root
            // still passes them to runc's threaded groups and both stay, as before.
            let engine = entry
                .path()
                .join(ENGINE_LEAF)
                .join("cgroup.subtree_control");
            for line in ["-pids", "-cpu"] {
                if engine.exists()
                    && let Err(error) = write(&engine, line)
                {
                    tracing::debug!(group = %entry.path().display(), %error, "the engine keeps {line}");
                }
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

/// The words of the file `path` (none when it cannot be read).
pub(crate) fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Whether a process runs in `group` or below it (`cgroup.events`).
pub(crate) fn populated(group: &Path) -> bool {
    lines(&group.join("cgroup.events"))
        .windows(2)
        .any(|pair| pair[0] == "populated" && pair[1] == "1")
}

/// The groups directly below `group`, sorted, at most [`MAX_GROUPS`].
pub(crate) fn subgroups(group: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(group) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_GROUPS)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Removes the emptied group `dir` and those below it, deepest first. A group that still holds
/// a process stays; so does a fake one in tests (it keeps its files).
pub(crate) fn remove_tree(dir: &Path, depth: usize) {
    if depth < MAX_DEPTH {
        for child in subgroups(dir) {
            remove_tree(&child, depth + 1);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Readies the engine's group `engine` for an engine in its root (SPEC-483 C4), unless something
/// runs in it but below the root: an engine of the old layout, which ran in the group itself.
/// That engine's first container made runc turn the subtree threaded (`engine/docker` threaded),
/// which leaves a domain group below it, [`ENGINE_ROOT_LEAF`], invalid: no container starts
/// there. So the emptied groups beside the root go, and `cpu` and `pids` pass down to the root
/// (one write each; both can be threaded), as the engine's group had them from the sandbox's
/// before: containers keep `--cpus` and `--pids-limit`, which runc passes on in threaded mode.
/// `memory` cannot be threaded and never reached them. A group whose `cgroup.procs` cannot be
/// read counts as busy; an engine that runs there keeps its groups until its front starts again.
fn settle_engine_group(engine: &Path) {
    let root = engine.join(ENGINE_ROOT_LEAF);
    let others: Vec<PathBuf> = subgroups(engine)
        .into_iter()
        .filter(|child| *child != root)
        .collect();
    let busy = match std::fs::read_to_string(engine.join("cgroup.procs")) {
        Ok(procs) => !procs.trim().is_empty(),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    };
    if busy || others.iter().any(|g| populated(g)) {
        return;
    }
    for child in &others {
        remove_tree(child, 0);
    }
    let available = lines(&engine.join("cgroup.controllers"));
    for controller in ["cpu", "pids"] {
        if available.iter().any(|c| c == controller) {
            let _ = std::fs::write(
                engine.join("cgroup.subtree_control"),
                format!("+{controller}"),
            );
        }
    }
}

/// Creates the engine's root `dir` (`engine/inside`) and the engine's group above it when
/// they are gone; returns whether it created either (SPEC-201 DBX-005, #614; SPEC-483 C4).
/// `yulius.service` stops with `KillMode=process`, and systemd then removes every empty group
/// left in the unit, those of an idle engine among them, while its holder and front run on.
/// Nothing above is created: without the sandbox's group, which holds the limits, it fails. An
/// engine's group an old engine left threaded is made plain first (`settle_engine_group`). An
/// engine's group made again under a split parent gets the default `cpu.weight`, 100, which is
/// the engine's (`LEAVES`); its task limit comes back with the sandbox's next preparation.
///
/// # Errors
/// `dir` has a `..`, the sandbox's group is gone, or a group cannot be created.
pub fn restore_leaf(dir: &Path) -> io::Result<bool> {
    if dir.components().any(|c| c.as_os_str() == "..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a cgroup directory",
        ));
    }
    let engine_root = |engine: &Path| {
        dir.file_name() == Some(ENGINE_ROOT_LEAF.as_ref())
            && engine.file_name() == Some(ENGINE_LEAF.as_ref())
    };
    if let Some(engine) = dir.parent().filter(|engine| engine_root(engine)) {
        settle_engine_group(engine);
    }
    let create = |group: &Path| match std::fs::create_dir(group) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    };
    // Only the engine's own group is made with it, never a sandbox's.
    let engine = match dir.parent().filter(|engine| engine_root(engine)) {
        Some(engine) => create(engine)?,
        None => false,
    };
    Ok(create(dir)? || engine)
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
        assert!(delegation.limit_sandboxes(Some(57_763))?);
        assert_eq!(std::fs::read_to_string(&common)?, "43320");
        assert!(delegation.limit_sandboxes(None)?);
        assert_eq!(std::fs::read_to_string(&common)?, "max", "reset, not stale");
        // Without the `pids` controller there is no such file to write.
        let (_root, delegation, _dir) = fake_sandbox("cpu memory\n")?;
        // Review: the server then warns about no common limit (`false`).
        assert!(!delegation.limit_sandboxes(Some(57_763))?);
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

    /// A fake delegated group: the directory and the two files a server must be able to write.
    fn delegated(dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("cgroup.procs"), "")?;
        std::fs::write(dir.join("cgroup.subtree_control"), "")
    }

    #[test]
    fn the_sessions_unit_beside_the_server_is_preferred() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let slice = root.path().join("yulius.slice");
        let server = slice.join("yulius.service");
        let sessions = slice.join("yulius-sandboxes.service");
        delegated(&server)?;
        let own = "/yulius.slice/yulius.service/server";
        // No sessions unit: the server's own unit, as before.
        assert_eq!(detect(root.path(), own)?, server);
        // A sessions unit the server cannot move processes into (the slice's file is missing).
        delegated(&sessions)?;
        assert_eq!(detect(root.path(), own)?, server);
        std::fs::write(slice.join("cgroup.procs"), "")?;
        assert_eq!(detect(root.path(), own)?, sessions);
        // It alone is enough: the server's unit needs no delegation then.
        std::fs::remove_file(server.join("cgroup.subtree_control"))?;
        assert_eq!(detect(root.path(), own)?, sessions);
        // Each missing permission falls back (here to an error: the server's unit is not ours).
        std::fs::remove_file(sessions.join("cgroup.subtree_control"))?;
        assert!(detect(root.path(), own).is_err());
        Ok(())
    }

    #[test]
    fn a_server_outside_a_service_unit_has_no_sessions_unit()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let scope = root.path().join("user.slice/run-1.scope");
        delegated(&scope)?;
        delegated(&root.path().join("user.slice/run-1.scope-sandboxes.service"))?;
        std::fs::write(root.path().join("user.slice/cgroup.procs"), "")?;
        assert_eq!(
            detect(root.path(), "/user.slice/run-1.scope/server")?,
            scope
        );
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
    /// sandbox group is never made up. SPEC-483 C4: the engine's root is made again with the
    /// group above it that holds the engine's limits.
    #[test]
    fn a_pruned_leaf_is_restored_under_its_parent() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let leaf = root.path().join("s1/engine/inside");
        assert!(restore_leaf(&leaf).is_err(), "no sandbox group");
        assert!(!root.path().join("s1").exists());
        std::fs::create_dir(root.path().join("s1"))?;
        assert!(restore_leaf(&leaf)?);
        assert!(leaf.is_dir());
        std::fs::remove_dir(&leaf)?;
        assert!(restore_leaf(&leaf)?, "the root alone");
        assert!(leaf.is_dir());
        // Only the engine's own group is made with its root, no other group's parent.
        assert!(restore_leaf(&root.path().join("s1/agents/inside")).is_err());
        assert!(restore_leaf(&root.path().join("s1/engine2/inside")).is_err());
        assert!(!root.path().join("s1/agents").exists());
        // An old engine's leftovers go first, also on this way (SPEC-483 C4).
        std::fs::remove_dir(&leaf)?;
        let engine = root.path().join("s1/engine");
        std::fs::create_dir(engine.join("docker"))?;
        std::fs::write(engine.join("cgroup.subtree_control"), "cpu\n")?;
        std::fs::write(engine.join("cgroup.controllers"), "cpu\n")?;
        assert!(restore_leaf(&leaf)?);
        assert!(!engine.join("docker").exists());
        assert_eq!(
            std::fs::read_to_string(engine.join("cgroup.subtree_control"))?,
            "+cpu"
        );
        std::fs::write(leaf.join("cgroup.procs"), "")?;
        assert!(!restore_leaf(&leaf)?, "already there");
        assert!(leaf.join("cgroup.procs").exists(), "left as it was");
        // Review round 1 (nit): no `..`, as `join` refuses.
        let around = root.path().join("s1/../s2/engine");
        assert!(restore_leaf(&around).is_err());
        assert!(!root.path().join("s2").exists());
        Ok(())
    }

    /// SPEC-504 CLB-001/002: a CI job's group is named after its id, and only such a group's
    /// processes are that job's.
    #[test]
    fn a_ci_job_group_is_named_after_the_job_and_read_back() {
        assert_eq!(
            ci_job_group_name("0b1c-2d_3").as_deref(),
            Some("ci-job-0b1c-2d_3")
        );
        for refused in ["", "a/b", "..", "a b", "ä", &"x".repeat(65)] {
            assert_eq!(ci_job_group_name(refused), None, "{refused:?}");
        }
        let unit = "/system.slice/yulius.service/sandboxes/s1";
        assert_eq!(
            ci_job_of(&format!("{unit}/ci/ci-job-j1")).as_deref(),
            Some("j1")
        );
        assert_eq!(
            ci_job_of(&format!("{unit}/ci-job-j2/")).as_deref(),
            Some("j2")
        );
        for other in [
            format!("{unit}/ci"),
            format!("{unit}/agents"),
            format!("{unit}/agents/run-ci-job-x"),
            format!("{unit}/ci/ci-job-"),
            format!("{unit}/ci/ci-job-a b"),
            format!("{unit}/engine/inside/docker/ci-job-x/y"),
        ] {
            assert_eq!(ci_job_of(&other), None, "{other}");
        }
    }

    /// SPEC-504 CLB-001/005: a job's group is made below the CI group, read as running while
    /// it holds a process and ended once it is empty or gone; the sweep takes only old, empty
    /// job groups.
    #[test]
    fn a_ci_job_group_lives_as_long_as_its_job() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        delegation.sandbox_group("s1", &Limits::default())?;
        let group = delegation.ci_job_group("s1", "j1")?;
        assert_eq!(group, dir.join("ci").join("ci-job-j1"));
        assert_eq!(delegation.ci_job_group("s1", "j1")?, group, "made once");
        assert!(delegation.ci_job_group("s1", "../x").is_err());
        assert_eq!(
            delegation.ci_job_running("s1", "j1"),
            None,
            "unreadable: taken as running"
        );
        std::fs::write(group.join("cgroup.events"), "populated 1\nfrozen 0\n")?;
        assert_eq!(delegation.ci_job_running("s1", "j1"), Some(true));
        std::fs::write(group.join("cgroup.events"), "populated 0\nfrozen 0\n")?;
        assert_eq!(delegation.ci_job_running("s1", "j1"), Some(false));
        assert_eq!(delegation.ci_job_running("s1", "gone"), Some(false));
        assert_eq!(delegation.ci_job_running("s1", "a/b"), Some(false));
        std::fs::remove_file(group.join("cgroup.events"))?;
        delegation.end_ci_job_group("s1", "j1");
        assert!(!group.exists(), "removed at the job's end");
        // The sweep: an empty job group made a moment ago stays (its job is about to join it),
        // as do other groups; one made long ago goes.
        let fresh = delegation.ci_job_group("s1", "fresh")?;
        let other = dir.join("ci").join("not-a-job");
        std::fs::create_dir(&other)?;
        delegation.sweep_ci_job_groups("s1");
        assert!(fresh.exists() && other.exists());
        let old = std::time::SystemTime::now() - CI_JOB_GROUP_KEPT * 2;
        for dir in [&fresh, &other] {
            std::fs::File::open(dir)?.set_modified(old)?;
        }
        delegation.sweep_ci_job_groups("s1");
        assert!(!fresh.exists(), "an old, empty job group goes");
        assert!(other.exists(), "not a job's");
        Ok(())
    }

    /// Review round 1 (minor): a job id runs again (a requeued run, a rerun for the
    /// infrastructure) after its group could not be removed; its group is made fresh, so the
    /// sweep does not take it, as an old empty one, before the job joins it.
    #[test]
    fn a_job_group_made_again_is_fresh_for_the_sweep() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, _dir) = fake_sandbox("cpu memory pids\n")?;
        delegation.sandbox_group("s1", &Limits::default())?;
        let group = delegation.ci_job_group("s1", "again")?;
        std::fs::File::open(&group)?
            .set_modified(std::time::SystemTime::now() - CI_JOB_GROUP_KEPT * 2)?;
        assert_eq!(delegation.ci_job_group("s1", "again")?, group);
        delegation.sweep_ci_job_groups("s1");
        assert!(group.exists(), "made again: not old");
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
                engine_root: dir.join("engine/inside"),
            }
        );
        assert!(groups.engine_root.is_dir());
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
        // SPEC-483 C4: the engine runs in, and sees as its `/sys/fs/cgroup`, a group below the one
        // that holds its limits, so nothing in the engine can reach them. (Controllers pass down
        // as the group lists them: none in this fake, see
        // `the_engine_root_gets_the_threaded_controllers`.)
        assert_eq!(groups.engine_root, groups.engine.join("inside"));
        assert!(groups.engine_root.is_dir());
        assert!(!groups.engine_root.join("pids.max").exists());
        assert!(!groups.engine.join("cgroup.subtree_control").exists());
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
        std::fs::write(
            groups.engine.join("docker/c1/pids.events.local"),
            "max 13\n",
        )?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 10);
        // On 6.8 (no `.local` file) `sandboxes/` holds no task and is not read.
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(groups.agents.join("pids.events"), "max 3\n")?;
        std::fs::write(
            delegation.unit.join(SANDBOXES).join("pids.events"),
            "max 5\n",
        )?;
        assert!(!dir.join("pids.events.local").exists());
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 3);
        Ok(())
    }

    /// SPEC-316 LIM-001, LIM-002 (#823): on kernels with `pids.events.local` a fork refused at the
    /// service's own limit (`TasksMax`) is counted in the unit's group only, and counts for the
    /// sandbox; a limit above the unit, outside the delegation, does not.
    #[test]
    fn a_hit_at_the_service_limit_counts_for_the_sandbox() -> Result<(), Box<dyn std::error::Error>>
    {
        let (root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(dir.join("pids.events.local"), "max 0\n")?;
        std::fs::write(groups.agents.join("pids.events.local"), "max 0\n")?;
        std::fs::write(
            delegation.unit.join(SANDBOXES).join("pids.events.local"),
            "max 0\n",
        )?;
        std::fs::write(delegation.unit.join("pids.events.local"), "max 6\n")?;
        // The slice above the unit counts every service's forks.
        std::fs::write(root.path().join("pids.events.local"), "max 9\n")?;
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 6);
        // On 6.8 (no `.local` file) the unit holds no task and is not read.
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(groups.agents.join("pids.events"), "max 3\n")?;
        std::fs::write(delegation.unit.join("pids.events"), "max 5\n")?;
        assert!(!dir.join("pids.events.local").exists());
        assert_eq!(delegation.limit_counters("s1").task_limit_hits, 3);
        Ok(())
    }

    /// Without the split the runs live in the sandbox's group, which cannot pass `pids` down.
    /// SPEC-483 C4 (an update): an engine of the old layout ran in `engine` itself, and runc made
    /// its subtree threaded (`engine/docker` threaded, `cpu` and `pids` passed down), which leaves
    /// a domain group below it invalid: no container would start in `inside`. Once nothing runs
    /// in the engine's group but below `inside`, the leftovers go and the controllers are taken
    /// back; an old engine that still runs there is left as it is.
    #[test]
    fn an_idle_engine_group_from_before_is_made_plain_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = |dir: &Path, engine_procs: &str, events: &str| -> io::Result<PathBuf> {
            let engine = dir.join("engine");
            std::fs::create_dir_all(engine.join("docker/c1"))?;
            std::fs::write(engine.join("cgroup.procs"), engine_procs)?;
            std::fs::write(engine.join("cgroup.subtree_control"), "cpu pids\n")?;
            std::fs::write(engine.join("cgroup.controllers"), "cpu pids\n")?;
            if !events.is_empty() {
                std::fs::write(engine.join("docker/cgroup.events"), events)?;
            }
            Ok(engine)
        };
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let engine = old(&dir, "", "")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert!(!engine.join("docker").exists(), "the leftovers are gone");
        // The threaded controllers pass down to the root again, one write each (a fake file
        // keeps the last); see `the_engine_root_gets_the_threaded_controllers`.
        assert_eq!(
            std::fs::read_to_string(engine.join("cgroup.subtree_control"))?,
            "+pids"
        );
        assert!(groups.engine_root.is_dir());
        // A new engine in `inside` is never touched.
        std::fs::write(groups.engine_root.join("cgroup.procs"), "7\n")?;
        std::fs::create_dir(groups.engine_root.join("docker"))?;
        delegation.sandbox_group("s1", &Limits::default())?;
        assert!(groups.engine_root.join("docker").is_dir());

        // The old engine still runs in the group itself, or a container of it below.
        for (procs, events) in [("41\n", ""), ("", "populated 1\nfrozen 0\n")] {
            let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
            let engine = old(&dir, procs, events)?;
            delegation.sandbox_group("s1", &Limits::default())?;
            assert!(engine.join("docker/c1").is_dir(), "{procs:?} {events:?}");
            assert_eq!(
                std::fs::read_to_string(engine.join("cgroup.subtree_control"))?,
                "cpu pids\n"
            );
        }
        Ok(())
    }

    /// SPEC-483 slice 39, review round 1 (F1): the engine's root gets `cpu` and `pids` from the
    /// engine's group, as the engine's group had them from the sandbox's before, so containers
    /// keep `--cpus` and `--pids-limit` (runc passes them on in threaded mode). `memory` never
    /// was and is not: it cannot be threaded. An unreadable `cgroup.procs` counts as busy.
    #[test]
    fn the_engine_root_gets_the_threaded_controllers() -> Result<(), Box<dyn std::error::Error>> {
        for (available, last) in [("cpu memory pids\n", "+pids"), ("cpu memory\n", "+cpu")] {
            let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
            let engine = dir.join("engine");
            std::fs::create_dir_all(&engine)?;
            std::fs::write(engine.join("cgroup.controllers"), available)?;
            delegation.sandbox_group("s1", &Limits::default())?;
            assert_eq!(
                std::fs::read_to_string(engine.join("cgroup.subtree_control"))?,
                last,
                "{available:?}"
            );
        }
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let engine = dir.join("engine");
        std::fs::create_dir_all(engine.join("cgroup.procs"))?;
        std::fs::write(engine.join("cgroup.controllers"), "cpu pids\n")?;
        delegation.sandbox_group("s1", &Limits::default())?;
        assert!(
            !engine.join("cgroup.subtree_control").exists(),
            "an unreadable `cgroup.procs` is a busy group"
        );
        Ok(())
    }

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
        // The engine runs below its group here too (SPEC-483 C4).
        assert_eq!(groups.engine_root, dir.join("engine/inside"));
        assert!(groups.engine_root.is_dir());
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
        std::fs::create_dir_all(dir.join("engine"))?;
        std::fs::write(dir.join("engine/cgroup.controllers"), "cpu pids\n")?;
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
        // SPEC-483 slice 39, review round 2: the engine's group passes `cpu` and `pids` to its
        // root, and the kernel takes a controller from no group while a child still passes it
        // on, so the engine's group gives them back first (seen on the guest).
        assert_eq!(
            std::fs::read_to_string(dir.join("engine/cgroup.subtree_control"))?,
            "-cpu"
        );
        assert!(!other.join("cgroup.subtree_control").exists());
        // A run that starts while the server stops still gets its leaf, but no split again, and
        // the engine's group passes nothing down again.
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(groups.ci, dir.join("ci"));
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "-cpu"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("engine/cgroup.subtree_control"))?,
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
                engine_root: dir.join("engine/inside"),
            }
        );
        assert!(!dir.join("ci").exists());
        // The engine's containers still get a group of their own below the sandbox's (SPEC-187
        // DKR-005), without controllers passed down.
        assert!(dir.join("engine").is_dir());
        assert!(!dir.join("cgroup.subtree_control").exists());
        Ok(())
    }

    /// SPEC-362 CSW-002, CSW-003, CSW-004: a sandbox's own group weighs 500 while any of its CI jobs
    /// runs, however many (review round 1), and 100 otherwise; preparing its groups keeps that, a
    /// starting server writes 100 to every sandbox (a crash left the old weight) and a stopping
    /// one too.
    #[test]
    fn a_sandbox_weighs_more_while_its_ci_jobs_run() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(sandbox_weight(0, 0), 100);
        assert_eq!(sandbox_weight(1, 1), 500);
        assert_eq!(sandbox_weight(4, 4), 500, "not by the number of jobs");
        assert_eq!(sandbox_weight(u32::MAX, 4), 500);
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let weight = || std::fs::read_to_string(dir.join("cpu.weight"));
        delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(weight()?, "100", "no job runs");
        assert!(delegation.set_ci_jobs("s1", 1, 1)?);
        assert_eq!(weight()?, "500");
        assert!(delegation.set_ci_jobs("s1", 2, 2)?);
        assert_eq!(weight()?, "500");
        // Every run prepares the groups again; that keeps the weight of the jobs running now.
        delegation.sandbox_group("s1", &Limits::default())?;
        assert_eq!(weight()?, "500");
        assert!(delegation.set_ci_jobs("s1", 1, 1)?);
        assert_eq!(weight()?, "500", "one job still runs");
        assert!(delegation.set_ci_jobs("s1", 0, 0)?);
        assert_eq!(weight()?, "100", "the jobs ended");
        // A new server knows of no job: its start writes the default again, before any run of
        // the sandbox prepares its groups (after a crash, nothing else would).
        delegation.set_ci_jobs("s1", 1, 1)?;
        let unit = dir.parent().and_then(Path::parent).ok_or("unit")?;
        let next = prepare(unit)?;
        assert_eq!(weight()?, "100", "after a restart");
        next.sandbox_group("s1", &Limits::default())?;
        assert_eq!(weight()?, "100");
        // A sandbox whose group is not there yet (its first run prepares it) is no error.
        assert!(!next.set_ci_jobs("s9", 1, 1)?);
        assert!(!dir.with_file_name("s9").exists());
        next.set_ci_jobs("s9", 0, 0)?;
        // A stopping server leaves the default for whatever starts next, and writes no more.
        delegation.set_ci_jobs("s1", 2, 2)?;
        delegation.unsplit_all();
        assert_eq!(weight()?, "100", "after the stop");
        assert!(!delegation.set_ci_jobs("s1", 3, 3)?);
        assert_eq!(weight()?, "100");
        Ok(())
    }

    /// The share of the CPU under `root` that cgroup v2 gives the busy group `group` when the
    /// groups in `busy` all want more than they get: at each level, its `cpu.weight` (100 when
    /// unset) over the weights of its siblings that hold a busy group.
    fn cpu_share(root: &Path, busy: &[PathBuf], group: &Path) -> f64 {
        let weight = |dir: &Path| {
            std::fs::read_to_string(dir.join("cpu.weight"))
                .ok()
                .and_then(|w| w.trim().parse::<f64>().ok())
                .unwrap_or(100.0)
        };
        let mut share = 1.0;
        let mut node = group;
        while node != root {
            let Some(parent) = node.parent() else {
                return 0.0;
            };
            let siblings: f64 = std::fs::read_dir(parent)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && busy.iter().any(|b| b.starts_with(path)))
                .map(|path| weight(&path))
                .sum();
            share *= weight(node) / siblings;
            node = parent;
        }
        share
    }

    /// SPEC-362 CSW-006: one sandbox runs a CI job while its agents and those of two other
    /// sandboxes build. With the weights Yulius writes the CI job gets 4/7 of the host and all
    /// local builds 3/7; with every sandbox at 100 (before) CI got 4/15 and the builds 11/15.
    #[test]
    fn under_load_ci_gets_more_of_the_host_than_local_builds()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let sandboxes = dir.parent().ok_or("sandboxes")?.to_path_buf();
        let mut agents = Vec::new();
        let mut ci = PathBuf::new();
        for sandbox in ["s1", "s2", "s3"] {
            let groups = delegation.sandbox_group(sandbox, &Limits::default())?;
            agents.push(groups.agents);
            if sandbox == "s1" {
                ci = groups.ci;
            }
        }
        let busy: Vec<PathBuf> = agents.iter().cloned().chain([ci.clone()]).collect();
        let builds = |busy: &[PathBuf]| -> f64 {
            agents
                .iter()
                .map(|leaf| cpu_share(&sandboxes, busy, leaf))
                .sum()
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Before: the sandboxes weigh the same, so CI gets a third of four fifths.
        let (ci_before, builds_before) = (cpu_share(&sandboxes, &busy, &ci), builds(&busy));
        assert!(close(ci_before, 4.0 / 15.0), "{ci_before}");
        assert!(ci_before < builds_before, "{ci_before} vs {builds_before}");
        // While the job runs.
        delegation.set_ci_jobs("s1", 1, 1)?;
        let (ci_now, builds_now) = (cpu_share(&sandboxes, &busy, &ci), builds(&busy));
        assert!(close(ci_now, 4.0 / 7.0), "{ci_now}");
        assert!(close(builds_now, 3.0 / 7.0), "{builds_now}");
        assert!(ci_now > builds_now, "{ci_now} vs {builds_now}");
        // More jobs do not weigh more (review round 1): each other sandbox's agents keep 1/7.
        delegation.set_ci_jobs("s1", 4, 4)?;
        for leaf in &agents[1..] {
            let share = cpu_share(&sandboxes, &busy, leaf);
            assert!(close(share, 1.0 / 7.0), "{share}");
        }
        // Idle CPU still goes to whoever wants it: with no CI work the agents share everything.
        let only_agents = agents.clone();
        assert!(close(builds(&only_agents), 1.0));
        Ok(())
    }

    /// SPEC-497 HBA-004: a sandbox whose CI jobs hold 30 of the host's 64 CPUs gets at least
    /// those 30 while its own agents and those of four other sandboxes build. With the flat 500
    /// of SPEC-362 it got 28.
    #[test]
    fn ci_gets_about_the_cpus_its_jobs_hold() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, dir) = fake_sandbox("cpu memory pids\n")?;
        let sandboxes = dir.parent().ok_or("sandboxes")?.to_path_buf();
        let mut busy = Vec::new();
        let mut ci = PathBuf::new();
        for sandbox in ["s1", "s2", "s3", "s4", "s5"] {
            let groups = delegation.sandbox_group(sandbox, &Limits::default())?;
            busy.push(groups.agents);
            if sandbox == "s1" {
                ci = groups.ci;
            }
        }
        busy.push(ci.clone());
        delegation.set_ci_jobs("s1", 1, 30)?;
        let cpus = 64.0 * cpu_share(&sandboxes, &busy, &ci);
        assert!(cpus >= 30.0, "{cpus}");
        // Each other sandbox's agents keep a share.
        let other = 64.0 * cpu_share(&sandboxes, &busy, &busy[1]);
        assert!(other > 1.5, "{other}");
        Ok(())
    }

    /// SPEC-497 HBA-002: CI's use is the `ci` and `engine` groups' together; none without the
    /// `cpu` split.
    #[test]
    fn ci_use_adds_the_ci_and_engine_groups() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, delegation, _dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(
            groups.ci.join("cpu.stat"),
            "usage_usec 1500\nuser_usec 1000\n",
        )?;
        std::fs::write(groups.ci.join("memory.stat"), "anon 4096\nfile 999999\n")?;
        std::fs::write(groups.engine.join("cpu.stat"), "usage_usec 500\n")?;
        std::fs::write(groups.engine.join("memory.stat"), "anon 1024\n")?;
        std::fs::write(groups.agents.join("cpu.stat"), "usage_usec 77777\n")?;
        assert_eq!(
            delegation.ci_use("s1"),
            Some(CiUse {
                cpu_usec: 2000,
                anon_bytes: 5120,
                cpu_pressure: None
            })
        );
        // The `ci` group's pressure (SPEC-497 HBA-002), not the engine's.
        std::fs::write(
            groups.ci.join("cpu.pressure"),
            "some avg10=24.49 avg60=11.59 avg300=4.29 total=2608214434\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        )?;
        std::fs::write(
            groups.engine.join("cpu.pressure"),
            "some avg10=90.00 avg60=0.00 avg300=0.00 total=1\n",
        )?;
        assert_eq!(
            delegation.ci_use("s1").and_then(|used| used.cpu_pressure),
            Some(245)
        );
        assert_eq!(delegation.ci_use("nobody"), None);
        let (_root, single, _dir) = fake_sandbox("memory pids\n")?;
        single.sandbox_group("s1", &Limits::default())?;
        assert_eq!(single.ci_use("s1"), None);
        Ok(())
    }

    #[test]
    fn ci_use_requires_cpu_time_from_each_existing_group() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, delegation, _dir) = fake_sandbox("cpu memory pids\n")?;
        let groups = delegation.sandbox_group("s1", &Limits::default())?;
        std::fs::write(groups.ci.join("cpu.stat"), "usage_usec 1500\n")?;
        // A directory in place of the stat file fails to read even when tests run as root.
        std::fs::create_dir(groups.engine.join("cpu.stat"))?;
        assert_eq!(delegation.ci_use("s1"), None);
        std::fs::remove_dir_all(&groups.engine)?;
        assert_eq!(
            delegation.ci_use("s1").map(|used| used.cpu_usec),
            Some(1500)
        );
        Ok(())
    }

    /// SPEC-497 HBA-002 (review of the CI admission fix): `some avg10` in thousandths; none
    /// without a `some` line or with a value that is no percentage; above 100 % counts as 100 %.
    #[test]
    fn cpu_pressure_reads_some_avg10() {
        let file = |some: &str| format!("{some}\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n");
        assert_eq!(
            some_avg10(&file("some avg10=24.50 avg60=1.00 avg300=0.00 total=5")),
            Some(245)
        );
        assert_eq!(some_avg10(&file("some avg10=0.00 avg60=0.00")), Some(0));
        assert_eq!(
            some_avg10("full avg10=50.00 avg60=0.00\n"),
            None,
            "no some line"
        );
        assert_eq!(some_avg10(&file("some avg10=nan avg60=0.00")), None);
        assert_eq!(some_avg10(&file("some avg10=-1.00 avg60=0.00")), None);
        assert_eq!(some_avg10(&file("some avg60=1.00")), None, "no avg10");
        assert_eq!(
            some_avg10(&file("some avg10=250.00 avg60=0.00")),
            Some(1000)
        );
    }

    /// SPEC-362 CSW-005: without the `cpu` controller nothing is weighted.
    #[test]
    fn without_the_cpu_controller_ci_jobs_weigh_nothing() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, delegation, dir) = fake_sandbox("memory pids\n")?;
        delegation.sandbox_group("s1", &Limits::default())?;
        assert!(!delegation.set_ci_jobs("s1", 1, 1)?);
        assert!(!dir.join("cpu.weight").exists());
        Ok(())
    }
}
