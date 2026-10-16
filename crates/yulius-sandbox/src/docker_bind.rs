//! Runs that are already running get the sandbox's Docker directory (ADR-0158, SPEC-196
//! DKL-004).
//!
//! The server finds the mount namespaces of a sandbox's runs ([`run_namespaces`]) and keeps those
//! whose mounts ([`plan`]) lack `/run/yulius/docker` and are not a production CD run's. Then
//! `yulius sandbox-host bind-docker --spec <state>/docker.json --pid …` ([`bind`]), a
//! single-threaded helper, joins the sandbox's user namespace (the server owns it, so the helper
//! has every capability there) and, for each run:
//!
//! 1. enters the holder's mount namespace, where `<state>/docker` is visible, and clones it as a
//!    detached tree (`open_tree(OPEN_TREE_CLONE)`): a plain bind cannot cross mount namespaces;
//! 2. enters the run's mount namespace and moves the tree to `/run/yulius/docker`
//!    (`move_mount`), creating the directory in the run's `/run` tmpfs, and makes it read-only;
//! 3. links `/run/docker.sock` to it when nothing is at that path (a SPEC-187 run's file bind
//!    stays).
//!
//! The run sees only that directory (the bind's root), its environment is unchanged
//! (`/var/run/docker.sock` is `docker`'s default), and nothing restarts. The helper is not in
//! the run's PID namespace, so the run cannot see or trace it.

use std::path::{Path, PathBuf};

use yulius_contracts::sandbox::{DOCKER_DIR_IN_RUNS, DockerFrontSpec};

/// Mount namespaces looked at per pass.
const MAX_PROCESSES: usize = 1 << 20;

/// What a run's mount namespace needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// The Docker directory is to be moved in.
    Bind,
    /// It has it (a run started since ADR-0158, or bound before).
    AlreadyBound,
    /// A production CD run (an empty `/var` over the host's system): never.
    HostSystem,
    /// No `/run` of its own: not a joined run.
    NotARun,
}

/// The plan for a mount namespace from its `/proc/<pid>/mountinfo`.
#[must_use]
pub fn plan(mountinfo: &str) -> Plan {
    let mut var_type = None;
    let mut has_run = false;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let (Some(point), Some(dash)) = (fields.get(4), fields.iter().position(|f| *f == "-"))
        else {
            continue;
        };
        let kind = fields.get(dash + 1).copied().unwrap_or_default();
        match *point {
            DOCKER_DIR_IN_RUNS => return Plan::AlreadyBound,
            "/var" => var_type = Some(kind),
            "/run" => has_run = true,
            _ => {}
        }
    }
    if var_type == Some("tmpfs") {
        Plan::HostSystem
    } else if has_run {
        Plan::Bind
    } else {
        Plan::NotARun
    }
}

/// One process per mount namespace among those in the network namespace `net_ns`, other than
/// `exclude` (the holder, its front), outside the namespaces `exclude_mnt` (the holder's) and
/// outside `engine_cgroup` (the engine and its containers): `(pid, mount namespace inode)`.
#[must_use]
pub fn run_namespaces(
    net_ns: u64,
    exclude: &[u32],
    exclude_mnt: &[u64],
    engine_cgroup: Option<&Path>,
) -> Vec<(u32, u64)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut found: Vec<(u32, u64)> = Vec::new();
    for pid in entries
        .flatten()
        .take(MAX_PROCESSES)
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
    {
        if exclude.contains(&pid) || crate::holder::ns_inode(pid, "net") != Some(net_ns) {
            continue;
        }
        let Some(mnt) = crate::holder::ns_inode(pid, "mnt") else {
            continue;
        };
        if exclude_mnt.contains(&mnt) || found.iter().any(|(_, m)| *m == mnt) {
            continue;
        }
        if engine_cgroup.is_some_and(|engine| cgroup_of(pid).is_some_and(|c| c.starts_with(engine)))
        {
            continue;
        }
        found.push((pid, mnt));
    }
    found
}

/// The cgroup v2 directory of `pid` on the host.
fn cgroup_of(pid: u32) -> Option<PathBuf> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let own = crate::cgroup::own_path(&text)?;
    Some(Path::new(crate::cgroup::CGROUP_ROOT).join(own.trim_start_matches('/')))
}

/// What happened to one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The directory was moved in.
    Bound,
    /// Nothing to do ([`Plan`] other than `Bind`).
    Skipped(Plan),
    /// It failed.
    Failed(String),
}

impl Outcome {
    /// The helper's output line for `pid`.
    #[must_use]
    pub fn line(&self, pid: u32) -> String {
        match self {
            Self::Bound => format!("bound {pid}"),
            Self::Skipped(plan) => format!("skipped {pid} {plan:?}"),
            Self::Failed(error) => format!("failed {pid} {error}"),
        }
    }

    /// Parses [`Outcome::line`]: the pid and whether that run is done with (bound or skipped).
    #[must_use]
    pub fn parse(line: &str) -> Option<(u32, bool)> {
        let mut words = line.split_whitespace();
        let done = match words.next()? {
            "bound" | "skipped" => true,
            "failed" => false,
            _ => return None,
        };
        Some((words.next()?.parse().ok()?, done))
    }
}

/// Body of `yulius sandbox-host bind-docker`: see the module docs. Must run single-threaded.
///
/// # Errors
/// The sandbox's namespaces cannot be joined.
#[cfg(target_os = "linux")]
pub fn bind(spec: &DockerFrontSpec, pids: &[u32]) -> Result<Vec<(u32, Outcome)>, String> {
    use nix::sched::{CloneFlags, setns};
    let paths = crate::holder::StatePaths::new(PathBuf::from(&spec.state_dir));
    let source = paths.docker_dir();
    let open = |kind| {
        crate::enter::holder_ns(
            spec.holder_pid,
            kind,
            match kind {
                "user" => spec.user_ns,
                _ => spec.mnt_ns,
            },
        )
        .map_err(|e| format!("the holder's {kind} namespace: {e}"))
    };
    let (user, holder_mnt) = (open("user")?, open("mnt")?);
    // Everything about the runs is read through the host's /proc, before any setns.
    let targets: Vec<(u32, Result<std::fs::File, Outcome>)> = pids
        .iter()
        .map(|&pid| {
            let target = match std::fs::read_to_string(format!("/proc/{pid}/mountinfo")) {
                Err(e) => Err(Outcome::Failed(format!("mountinfo: {e}"))),
                Ok(text) => match plan(&text) {
                    Plan::Bind => std::fs::File::open(format!("/proc/{pid}/ns/mnt"))
                        .map_err(|e| Outcome::Failed(format!("mount namespace: {e}"))),
                    other => Err(Outcome::Skipped(other)),
                },
            };
            (pid, target)
        })
        .collect();
    setns(&user, CloneFlags::CLONE_NEWUSER).map_err(|e| format!("setns user: {e}"))?;
    Ok(targets
        .into_iter()
        .map(|(pid, target)| {
            let outcome = match target {
                Err(outcome) => outcome,
                Ok(mnt) => bind_one(&holder_mnt, &mnt, &source)
                    .map_or_else(Outcome::Failed, |()| Outcome::Bound),
            };
            (pid, outcome)
        })
        .collect())
}

#[cfg(target_os = "linux")]
fn bind_one(
    holder_mnt: &std::fs::File,
    run_mnt: &std::fs::File,
    source: &Path,
) -> Result<(), String> {
    use nix::sched::{CloneFlags, setns};
    use rustix::mount::{MoveMountFlags, OpenTreeFlags, move_mount, open_tree};
    use yulius_contracts::sandbox::{DOCKER_SOCKET_LINK, ENGINE_SOCKET_IN_RUNS};
    setns(holder_mnt, CloneFlags::CLONE_NEWNS).map_err(|e| format!("setns holder: {e}"))?;
    let tree = open_tree(
        rustix::fs::CWD,
        source,
        OpenTreeFlags::OPEN_TREE_CLONE | OpenTreeFlags::OPEN_TREE_CLOEXEC,
    )
    .map_err(|e| format!("open_tree {}: {e}", source.display()))?;
    setns(run_mnt, CloneFlags::CLONE_NEWNS).map_err(|e| format!("setns run: {e}"))?;
    std::fs::create_dir_all(DOCKER_DIR_IN_RUNS)
        .map_err(|e| format!("mkdir {DOCKER_DIR_IN_RUNS}: {e}"))?;
    move_mount(
        &tree,
        "",
        rustix::fs::CWD,
        DOCKER_DIR_IN_RUNS,
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
    )
    .map_err(|e| format!("move_mount: {e}"))?;
    read_only(Path::new(DOCKER_DIR_IN_RUNS));
    match std::fs::symlink_metadata(ENGINE_SOCKET_IN_RUNS) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(DOCKER_SOCKET_LINK, ENGINE_SOCKET_IN_RUNS)
                .map_err(|e| format!("link {ENGINE_SOCKET_IN_RUNS}: {e}"))
        }
        _ => Ok(()),
    }
}

/// Makes the bind at `path` read-only, as in runs started with it (the directory holds only the
/// socket). Best effort: a bind keeps its source's locked flags (atime, nosuid, nodev), so they
/// are repeated; if the kernel still refuses, the bind stays writable but works.
#[cfg(target_os = "linux")]
fn read_only(path: &Path) {
    use nix::mount::MsFlags;
    use nix::sys::statvfs::FsFlags;
    let mut flags = MsFlags::MS_REMOUNT | MsFlags::MS_BIND | MsFlags::MS_RDONLY;
    if let Ok(stat) = nix::sys::statvfs::statvfs(path) {
        for (have, set) in [
            (FsFlags::ST_NOSUID, MsFlags::MS_NOSUID),
            (FsFlags::ST_NODEV, MsFlags::MS_NODEV),
            (FsFlags::ST_NOEXEC, MsFlags::MS_NOEXEC),
            (FsFlags::ST_NOATIME, MsFlags::MS_NOATIME),
            (FsFlags::ST_NODIRATIME, MsFlags::MS_NODIRATIME),
            (FsFlags::ST_RELATIME, MsFlags::MS_RELATIME),
        ] {
            if stat.flags().contains(have) {
                flags |= set;
            }
        }
    }
    let _ = nix::mount::mount(None::<&str>, path, None::<&str>, flags, None::<&str>);
}

/// Body of `yulius sandbox-host bind-docker` (Linux only).
///
/// # Errors
/// Always here.
#[cfg(not(target_os = "linux"))]
pub fn bind(_spec: &DockerFrontSpec, _pids: &[u32]) -> Result<Vec<(u32, Outcome)>, String> {
    Err("sandbox namespaces are Linux-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "\
101 90 0:52 / / rw,relatime - tmpfs tmpfs rw
102 101 0:53 / /proc rw - proc proc rw
110 101 0:60 / /run rw,nosuid - tmpfs tmpfs rw,mode=755
111 101 0:41 /usr /usr rw,relatime - overlay overlay rw,lowerdir=/d/system/base-1
112 101 0:41 /var /var rw,relatime - overlay overlay rw,lowerdir=/d/system/base-1
113 101 0:61 / /var/lib/yulius rw - tmpfs tmpfs rw
";

    /// SPEC-196 DKL-004: a run started before ADR-0158 is bound; one that has the directory, a
    /// production CD run (its `/var` an empty tmpfs) and a namespace without `/run` are not.
    #[test]
    fn only_runs_without_the_directory_are_bound() {
        assert_eq!(plan(RUN), Plan::Bind);
        let bound = format!(
            "{RUN}120 110 8:1 /var/lib/yulius/runtime/sandboxes/s1/docker /run/yulius/docker rw - ext4 /dev/sda1 rw\n"
        );
        assert_eq!(plan(&bound), Plan::AlreadyBound);
        let cd = RUN.replace(
            "112 101 0:41 /var /var rw,relatime - overlay overlay",
            "112 101 0:70 / /var rw - tmpfs tmpfs",
        );
        assert_eq!(plan(&cd), Plan::HostSystem);
        let holder = "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:5 / /proc rw - proc proc rw\n";
        assert_eq!(plan(holder), Plan::NotARun);
        assert_eq!(plan(""), Plan::NotARun);
        assert_eq!(plan("garbage line\n"), Plan::NotARun);
    }

    #[test]
    fn outcomes_round_trip_through_the_helpers_output() {
        for (outcome, done) in [
            (Outcome::Bound, true),
            (Outcome::Skipped(Plan::HostSystem), true),
            (Outcome::Failed("move_mount: EPERM".into()), false),
        ] {
            assert_eq!(Outcome::parse(&outcome.line(42)), Some((42, done)));
        }
        assert_eq!(Outcome::parse("noise"), None);
    }

    /// The test process's own namespace is excluded like the holder's; nothing else in its
    /// network namespace shares it, and the scan stays bounded.
    #[test]
    fn the_scan_skips_excluded_namespaces() {
        let me = std::process::id();
        let (Some(net), Some(mnt)) = (
            crate::holder::ns_inode(me, "net"),
            crate::holder::ns_inode(me, "mnt"),
        ) else {
            return;
        };
        let found = run_namespaces(net, &[me], &[mnt], None);
        assert!(
            found.iter().all(|(pid, m)| *pid != me && *m != mnt),
            "{found:?}"
        );
    }
}
