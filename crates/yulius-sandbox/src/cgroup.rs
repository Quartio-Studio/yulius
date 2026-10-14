//! Per-sandbox cgroup v2 limits through systemd delegation (SPEC-002 ADR-22 §5).
//!
//! With `Delegate=yes` and `DelegateSubgroup=server` on `yulius.service`, systemd starts the
//! server in `<unit>/server` and hands `<unit>` to the service user. The server then:
//! - moves processes left in `<unit>` from before delegation (runs, browsers) to `<unit>/legacy`;
//! - enables the `cpu`, `memory` and `pids` controllers for `<unit>` and `<unit>/sandboxes`;
//! - gives each sandbox `<unit>/sandboxes/<sandbox>` with its limits, split by CPU weight into
//!   `system/` (the holder), `agents/` (runs) and `ci/` (CI jobs) when `cpu` is available
//!   (SPEC-053).
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
/// share, so up to 80 % of the sandbox under full contention while the holder idles.
const LEAVES: [(&str, u32); 3] = [("system", 100), ("agents", 100), ("ci", 400)];

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
    /// Defaults for a host: the memory but a reserve for the host and Yulius (an eighth, at
    /// least 4 GiB; the sandbox gets at least 2 GiB), no CPU quota, 8192 tasks. Half the memory
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
    pub fn defaults(total_memory: u64) -> Self {
        let reserve = (total_memory / 8).max(4 << 30);
        Self {
            memory_max: Some(total_memory.saturating_sub(reserve).max(2 << 30)),
            cpus: None,
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

/// The groups a sandbox's processes join (SPEC-053 CPU-201). Without the `cpu` controller all
/// three are the sandbox's own group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxGroups {
    /// The holder and `slirp4netns`.
    pub system: PathBuf,
    /// Agent runs, shells, browsers and the Cockpit.
    pub agents: PathBuf,
    /// CI jobs.
    pub ci: PathBuf,
}

impl SandboxGroups {
    fn single(dir: &Path) -> Self {
        Self {
            system: dir.to_path_buf(),
            agents: dir.to_path_buf(),
            ci: dir.to_path_buf(),
        }
    }

    fn leaves(dir: &Path) -> Self {
        let [system, agents, ci] = LEAVES.map(|(leaf, _)| dir.join(leaf));
        Self { system, agents, ci }
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

impl Delegation {
    /// The groups of `sandbox` (created when missing) with `limits` applied.
    ///
    /// # Errors
    /// The group cannot be created or a limit cannot be written.
    pub fn sandbox_group(&self, sandbox: &str, limits: &Limits) -> io::Result<SandboxGroups> {
        let dir = self.unit.join(SANDBOXES).join(sandbox);
        std::fs::create_dir_all(&dir)?;
        for (file, value) in limits.files() {
            let controller = file.split('.').next().unwrap_or_default();
            if self.controllers.iter().any(|c| c == controller) {
                write(&dir.join(file), &value)?;
            }
        }
        if !self.controllers.iter().any(|c| c == "cpu") {
            return Ok(SandboxGroups::single(&dir));
        }
        let groups = SandboxGroups::leaves(&dir);
        for leaf in [&groups.system, &groups.agents, &groups.ci] {
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
        drop(stopping);
        Ok(groups)
    }

    /// Takes the CPU back from every sandbox's leaves (`-cpu`) when the server stops (SPEC-053
    /// CPU-205). An older binary started after this one (a rollback) joins the sandbox group
    /// itself, which the kernel refuses while the group passes a controller down. Processes stay
    /// in their leaves; the next preparation splits again.
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
                    .any(|c| c.trim_start_matches('+') == "cpu")
            });
            if split && let Err(error) = write(&control, "-cpu") {
                tracing::warn!(group = %entry.path().display(), %error, "cannot undo the CPU split");
            }
        }
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
        let limits = Limits::defaults(48 << 30);
        assert_eq!(limits.memory_max, Some(42 << 30), "all but an eighth");
        assert_eq!(
            Limits::defaults(16 << 30).memory_max,
            Some(12 << 30),
            "a 4 GiB reserve at least"
        );
        assert_eq!(limits.cpus, None, "shared by weight, not capped (SPEC-082)");
        assert_eq!(limits.files()[1], ("cpu.max", "max 100000".to_owned()));
        assert_eq!(limits.pids_max, Some(8192));
        let small = Limits::defaults(2 << 30);
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
            }
        );
        // Only the CPU goes down to the leaves; the limits stay on the sandbox.
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.subtree_control"))?,
            "+cpu"
        );
        assert_eq!(std::fs::read_to_string(dir.join("cpu.max"))?, "max 100000");
        for (leaf, weight) in [("system", "100"), ("agents", "100"), ("ci", "400")] {
            assert_eq!(
                std::fs::read_to_string(dir.join(leaf).join("cpu.weight"))?,
                weight,
                "{leaf}"
            );
        }
        // Preparing again (every run does) changes nothing.
        assert_eq!(delegation.sandbox_group("s1", &Limits::default())?, groups);
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
            }
        );
        assert!(!dir.join("ci").exists());
        assert!(!dir.join("cgroup.subtree_control").exists());
        Ok(())
    }
}
