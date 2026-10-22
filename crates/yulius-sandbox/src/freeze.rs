//! Freezing an agent run's process tree with the cgroup v2 freezer while Yulius updates
//! (SPEC-160 OPS-010..OPS-013, ADR-0131).
//!
//! A run's ptyhost joins its sandbox's `agents/` group before it forks, so the agent and every
//! process it starts live there, next to people's shells, browsers and the ptyhosts. To freeze
//! one run, [`freeze`] moves the agent's tree (the recorded child and its descendants found in
//! the child's group) into the run's own group `run-<name>` under that group and writes `1` to
//! its `cgroup.freeze`. The kernel stops those processes with no signal, so nothing in them can
//! tell; the ptyhost keeps serving its socket. [`thaw`] writes `0`; [`thaw_all`] thaws every run
//! group left frozen under the delegated unit (a server that stopped between freeze and thaw).
//!
//! Only groups under `<unit>/sandboxes/` are touched: a run elsewhere (no delegation, a run on
//! the host) cannot be frozen and the caller pauses it another way. The processes stay in the
//! run group after a thaw, under the same sandbox limits; the next freeze reuses it.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Prefix of a run's group.
pub const RUN_GROUP_PREFIX: &str = "run-";
/// The parent of sandbox groups in the delegated unit.
const SANDBOXES: &str = "sandboxes";
/// Passes that move processes forked while the tree was being moved.
const MAX_PASSES: usize = 8;
/// Longest parent chain followed from a process to the run's child.
const MAX_DEPTH: usize = 256;
/// Groups scanned by [`thaw_all`] at most.
const MAX_GROUPS: usize = 4096;
/// Longest run name.
const MAX_NAME: usize = 100;

/// What the kernel's process table says about a process (`/proc` on Linux).
pub trait ProcessTable {
    /// The process's cgroup v2 path (`/system.slice/…`), relative to the cgroup root.
    fn cgroup(&self, pid: u32) -> Option<String>;
    /// The process's parent.
    fn parent(&self, pid: u32) -> Option<u32>;
}

/// The real process table.
#[derive(Debug, Clone, Copy, Default)]
pub struct Procfs;

impl ProcessTable for Procfs {
    fn cgroup(&self, pid: u32) -> Option<String> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        crate::cgroup::own_path(&text)
    }

    fn parent(&self, pid: u32) -> Option<u32> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Fields after `(comm)`: state, ppid, …
        let after = text.get(text.rfind(')')? + 1..)?;
        after.split_whitespace().nth(1)?.parse().ok()
    }
}

/// Where the cgroup tree is and which part of it is ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// Where cgroup v2 is mounted ([`crate::cgroup::CGROUP_ROOT`]).
    pub root: PathBuf,
    /// The delegated unit (under `root`).
    pub unit: PathBuf,
}

/// Why a run cannot be frozen; the caller pauses it another way.
#[derive(Debug, thiserror::Error)]
pub enum FreezeError {
    /// The name is not a run name (letters, digits, `-` and `_`).
    #[error("not a run name")]
    BadName,
    /// The run's child is gone or its group cannot be read.
    #[error("the run's process is gone")]
    Gone,
    /// The child's group is not under the delegated unit's sandboxes.
    #[error("the run is not in a sandbox group of this server")]
    NotDelegated,
    /// The run's child did not move into its group (it keeps running).
    #[error("the run's process did not move into its group")]
    NotMoved,
    /// A cgroup file cannot be written.
    #[error("cgroup: {0}")]
    Io(#[from] io::Error),
}

/// A frozen (or freezing) run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    /// The run's group.
    pub group: PathBuf,
    /// The kernel reports the group frozen (`frozen 1` in `cgroup.events`).
    pub settled: bool,
    /// The processes this call moved into the group.
    pub moved: Vec<u32>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The group the child of run `name` is in, and the run's group: the child's own group when it is
/// already the run's, else `run-<name>` under it.
fn groups(
    tree: &Tree,
    procs: &dyn ProcessTable,
    child: u32,
    name: &str,
) -> Result<(PathBuf, PathBuf), FreezeError> {
    if !valid_name(name) {
        return Err(FreezeError::BadName);
    }
    let own = procs.cgroup(child).ok_or(FreezeError::Gone)?;
    let relative = own.trim().trim_start_matches('/');
    if relative
        .split('/')
        .any(|c| c == ".." || c == "." || c.is_empty())
    {
        return Err(FreezeError::NotDelegated);
    }
    let current = tree.root.join(relative);
    let sandboxes = tree.unit.join(SANDBOXES);
    if !current.starts_with(&sandboxes) || current == sandboxes {
        return Err(FreezeError::NotDelegated);
    }
    let leaf = format!("{RUN_GROUP_PREFIX}{name}");
    if current.file_name().is_some_and(|n| n == leaf.as_str()) {
        let parent = current
            .parent()
            .map(Path::to_path_buf)
            .ok_or(FreezeError::NotDelegated)?;
        Ok((parent, current))
    } else {
        let group = current.join(&leaf);
        Ok((current, group))
    }
}

/// Moves `pid` into `group` (appended, as `echo pid >> cgroup.procs`: the kernel moves each).
fn move_into(group: &Path, pid: u32) -> io::Result<()> {
    // `create` is a no-op on cgroupfs, where the kernel made the file with the group.
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(group.join("cgroup.procs"))?
        .write_all(format!("{pid}\n").as_bytes())
}

/// Checks that freezing `name` could work, without freezing or creating anything.
///
/// # Errors
/// As [`freeze`], before it writes.
pub fn check(
    tree: &Tree,
    procs: &dyn ProcessTable,
    child: u32,
    name: &str,
) -> Result<(), FreezeError> {
    groups(tree, procs, child, name).map(|_| ())
}

fn settled(group: &Path) -> bool {
    std::fs::read_to_string(group.join("cgroup.events"))
        .is_ok_and(|text| text.lines().any(|l| l.trim() == "frozen 1"))
}

fn members(group: &Path) -> Vec<u32> {
    std::fs::read_to_string(group.join("cgroup.procs"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Whether `pid` is `child` or one of its descendants.
fn descends(procs: &dyn ProcessTable, pid: u32, child: u32, seen: &mut HashMap<u32, bool>) -> bool {
    let mut chain = Vec::new();
    let mut at = pid;
    let found = loop {
        if at == child {
            break true;
        }
        if let Some(known) = seen.get(&at) {
            break *known;
        }
        if at <= 1 || chain.len() >= MAX_DEPTH {
            break false;
        }
        chain.push(at);
        match procs.parent(at) {
            Some(parent) => at = parent,
            None => break false,
        }
    };
    for pid in chain {
        seen.insert(pid, found);
    }
    found
}

/// Freezes the process tree of the run `name` whose recorded child is `child` (see the module
/// docs). Idempotent: a frozen run is checked again and stragglers are moved.
///
/// # Errors
/// [`FreezeError`]: the run cannot be frozen; nothing is frozen unless the error is
/// [`NotMoved`] or [`Io`] after the group was frozen, which a [`thaw`] undoes.
///
/// [`NotMoved`]: FreezeError::NotMoved
/// [`Io`]: FreezeError::Io
pub fn freeze(
    tree: &Tree,
    procs: &dyn ProcessTable,
    child: u32,
    name: &str,
) -> Result<Frozen, FreezeError> {
    let (parent, group) = groups(tree, procs, child, name)?;
    std::fs::create_dir_all(&group)?;
    // Frozen first: each process moved in stops as it lands, and what it forks stays there.
    std::fs::write(group.join("cgroup.freeze"), "1")?;
    let mut seen = HashMap::new();
    let mut moved = Vec::new();
    let mut done = HashSet::new();
    for pass in 0..MAX_PASSES {
        let found: Vec<u32> = members(&parent)
            .into_iter()
            .filter(|pid| !done.contains(pid) && descends(procs, *pid, child, &mut seen))
            .collect();
        if found.is_empty() {
            break;
        }
        if pass + 1 == MAX_PASSES {
            tracing::debug!(run = name, "the run still forked on the last pass");
        }
        for pid in found {
            // A process that exited meanwhile is not an error.
            let _ = move_into(&group, pid);
            done.insert(pid);
            moved.push(pid);
        }
    }
    // The agent itself must be in: otherwise the run keeps working while counted as paused.
    if !members(&group).contains(&child) {
        return Err(FreezeError::NotMoved);
    }
    Ok(Frozen {
        settled: settled(&group),
        group,
        moved,
    })
}

/// Thaws the run `name` whose recorded child is `child`. `Ok(false)` when it has no run group.
///
/// # Errors
/// As [`freeze`].
pub fn thaw(
    tree: &Tree,
    procs: &dyn ProcessTable,
    child: u32,
    name: &str,
) -> Result<bool, FreezeError> {
    let (_, group) = groups(tree, procs, child, name)?;
    if !group.is_dir() {
        return Ok(false);
    }
    std::fs::write(group.join("cgroup.freeze"), "0")?;
    Ok(true)
}

/// Run groups under the unit's sandboxes: `sandboxes/<sandbox>/run-*` and
/// `sandboxes/<sandbox>/<leaf>/run-*`.
fn run_groups(unit: &Path) -> Vec<PathBuf> {
    let is_run = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(RUN_GROUP_PREFIX))
    };
    let dirs = |p: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .map(|entries| {
                entries
                    .flatten()
                    .take(MAX_GROUPS)
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut found = Vec::new();
    for sandbox in dirs(&unit.join(SANDBOXES)) {
        for child in dirs(&sandbox) {
            if is_run(&child) {
                found.push(child);
            } else {
                found.extend(dirs(&child).into_iter().filter(|p| is_run(p)));
            }
            if found.len() >= MAX_GROUPS {
                return found;
            }
        }
    }
    found
}

/// Whether `thaw_all` removes the run group `group`: not while it has processes, nor once forks
/// were refused to them (`max` in `pids.events` or `pids.events.local`). The sandbox's limit
/// counters add up its groups' counts, so removing such a group during another run would make
/// them fall and hide a rise (SPEC-272 SHR-004); systemd removes it when the service stops.
fn removable(group: &Path) -> bool {
    let met_a_limit = ["pids.events", "pids.events.local"].iter().any(|file| {
        std::fs::read_to_string(group.join(file))
            .is_ok_and(|text| crate::cgroup::keyed(&text, "max") > 0)
    });
    members(group).is_empty() && !met_a_limit
}

/// Thaws every frozen run group under the unit's sandboxes but those named in `keep` (the runs
/// a person paused, SPEC-191) and removes the empty ones but those whose processes met a task
/// limit (SPEC-272 SHR-004); returns how many were thawed.
pub fn thaw_all(unit: &Path, keep: &HashSet<String>) -> usize {
    let mut thawed = 0;
    for group in run_groups(unit) {
        if group
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| keep.contains(n))
        {
            continue;
        }
        let freeze = group.join("cgroup.freeze");
        let frozen = std::fs::read_to_string(&freeze).is_ok_and(|t| t.trim() == "1");
        if frozen {
            match std::fs::write(&freeze, "0") {
                Ok(()) => thawed += 1,
                Err(error) => {
                    tracing::warn!(group = %group.display(), %error, "cannot thaw a run group");
                }
            }
        }
        // The kernel refuses to remove a group that still has processes.
        if removable(&group) {
            let _ = std::fs::remove_dir(&group);
        }
    }
    thawed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process table of (pid, parent, cgroup).
    struct Table(Vec<(u32, u32, &'static str)>);

    impl ProcessTable for Table {
        fn cgroup(&self, pid: u32) -> Option<String> {
            self.0
                .iter()
                .find(|(p, _, _)| *p == pid)
                .map(|(_, _, c)| (*c).to_owned())
        }

        fn parent(&self, pid: u32) -> Option<u32> {
            self.0
                .iter()
                .find(|(p, _, _)| *p == pid)
                .map(|(_, p, _)| *p)
        }
    }

    const AGENTS: &str = "/u/sandboxes/s1/agents";

    fn tree() -> Result<(tempfile::TempDir, Tree, PathBuf), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let agents = root.path().join(AGENTS.trim_start_matches('/'));
        std::fs::create_dir_all(&agents)?;
        // The ptyhost 99, its agent 100, the agent's tools 101 and a background shell 102
        // reparented under 101, and a person's shell 200.
        std::fs::write(agents.join("cgroup.procs"), "99\n100\n101\n102\n200\n")?;
        let tree = Tree {
            root: root.path().to_path_buf(),
            unit: root.path().join("u"),
        };
        Ok((root, tree, agents))
    }

    fn table() -> Table {
        Table(vec![
            (99, 1, AGENTS),
            (100, 99, AGENTS),
            (101, 100, AGENTS),
            (102, 101, AGENTS),
            (200, 1, AGENTS),
        ])
    }

    #[test]
    fn the_agent_tree_moves_into_its_frozen_group_and_its_host_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_root, tree, agents) = tree()?;
        let group = agents.join("run-a1");
        let frozen = freeze(&tree, &table(), 100, "a1")?;
        assert_eq!(frozen.group, group);
        assert_eq!(std::fs::read_to_string(group.join("cgroup.freeze"))?, "1");
        assert_eq!(
            frozen.moved,
            vec![100, 101, 102],
            "never the ptyhost or a shell"
        );
        assert_eq!(
            std::fs::read_to_string(group.join("cgroup.procs"))?,
            "100\n101\n102\n"
        );
        assert!(check(&tree, &table(), 100, "a1").is_ok());
        assert!(!frozen.settled, "frozen only once the kernel says so");
        std::fs::write(group.join("cgroup.events"), "populated 1\nfrozen 1\n")?;
        let again = freeze(&tree, &table(), 100, "a1")?;
        assert!(again.settled);
        assert_eq!(again.group, group);
        Ok(())
    }

    #[test]
    fn a_run_already_in_its_group_reuses_it() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, tree, agents) = tree()?;
        let group = agents.join("run-a1");
        std::fs::create_dir_all(&group)?;
        std::fs::write(group.join("cgroup.procs"), "100\n101\n")?;
        let inside = "/u/sandboxes/s1/agents/run-a1";
        let procs = Table(vec![(99, 1, AGENTS), (100, 99, inside), (101, 100, inside)]);
        let frozen = freeze(&tree, &procs, 100, "a1")?;
        assert_eq!(frozen.group, group);
        assert!(!group.join("run-a1").exists(), "no group inside the group");
        assert_eq!(std::fs::read_to_string(group.join("cgroup.freeze"))?, "1");
        assert!(thaw(&tree, &procs, 100, "a1")?);
        assert_eq!(std::fs::read_to_string(group.join("cgroup.freeze"))?, "0");
        Ok(())
    }

    #[test]
    fn runs_outside_the_sandboxes_or_gone_are_not_frozen() -> Result<(), Box<dyn std::error::Error>>
    {
        let (root, tree, _agents) = tree()?;
        let outside = Table(vec![(100, 99, "/u/server")]);
        assert!(matches!(
            freeze(&tree, &outside, 100, "a1"),
            Err(FreezeError::NotDelegated)
        ));
        let sneaky = Table(vec![(100, 99, "/u/sandboxes/../server")]);
        assert!(matches!(
            freeze(&tree, &sneaky, 100, "a1"),
            Err(FreezeError::NotDelegated)
        ));
        let bare = Table(vec![(100, 99, "/u/sandboxes")]);
        assert!(matches!(
            freeze(&tree, &bare, 100, "a1"),
            Err(FreezeError::NotDelegated)
        ));
        assert!(matches!(
            freeze(&tree, &table(), 7, "a1"),
            Err(FreezeError::Gone)
        ));
        for name in ["", "../x", "a/b", "a b"] {
            assert!(matches!(
                freeze(&tree, &table(), 100, name),
                Err(FreezeError::BadName)
            ));
        }
        assert!(matches!(
            check(&tree, &outside, 100, "a1"),
            Err(FreezeError::NotDelegated)
        ));
        assert!(!root.path().join("u/server").exists(), "nothing created");
        assert!(!thaw(&tree, &table(), 100, "a1")?, "never frozen");
        Ok(())
    }

    #[test]
    fn a_child_that_did_not_move_is_not_counted_as_frozen() -> Result<(), Box<dyn std::error::Error>>
    {
        let (_root, tree, agents) = tree()?;
        // The parent group no longer lists the child (it cannot be moved from where it is).
        std::fs::write(agents.join("cgroup.procs"), "99\n200\n")?;
        assert!(matches!(
            freeze(&tree, &table(), 100, "a1"),
            Err(FreezeError::NotMoved)
        ));
        Ok(())
    }

    #[test]
    fn thaw_all_thaws_frozen_run_groups_and_removes_empty_ones()
    -> Result<(), Box<dyn std::error::Error>> {
        let (root, tree, agents) = tree()?;
        let frozen = agents.join("run-a1");
        let empty = agents.join("run-a2");
        let single = root.path().join("u/sandboxes/s2/run-a3");
        for (group, freeze, procs) in [
            (&frozen, "1", "100\n"),
            (&empty, "0", ""),
            (&single, "1", "300\n"),
        ] {
            std::fs::create_dir_all(group)?;
            std::fs::write(group.join("cgroup.freeze"), freeze)?;
            std::fs::write(group.join("cgroup.procs"), procs)?;
        }
        // Not run groups: left alone.
        std::fs::write(agents.join("cgroup.freeze"), "1")?;
        assert_eq!(thaw_all(&tree.unit, &HashSet::new()), 2);
        assert_eq!(std::fs::read_to_string(frozen.join("cgroup.freeze"))?, "0");
        assert_eq!(std::fs::read_to_string(single.join("cgroup.freeze"))?, "0");
        assert_eq!(std::fs::read_to_string(agents.join("cgroup.freeze"))?, "1");
        // A fake empty group still holds its files; the kernel's would be removed.
        assert!(empty.is_dir() || !empty.exists());
        assert_eq!(
            thaw_all(&tree.unit, &HashSet::new()),
            0,
            "nothing left frozen"
        );
        Ok(())
    }

    /// SPEC-272 SHR-004 (SPEC-268 review round 3): an empty run group whose processes met a task
    /// limit stays, so a sandbox's limit counters never fall during another run. The decision is
    /// tested, not `thaw_all`'s removal: a fake group keeps its files, so `remove_dir` always
    /// fails on it.
    #[test]
    fn an_empty_run_group_that_met_a_limit_is_kept() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, _tree, agents) = tree()?;
        let group = agents.join("run-a1");
        std::fs::create_dir_all(&group)?;
        std::fs::write(group.join("cgroup.procs"), "")?;
        assert!(removable(&group), "empty, no counters");
        std::fs::write(group.join("pids.events"), "max 0\n")?;
        assert!(removable(&group), "no hit");
        std::fs::write(group.join("pids.events"), "max 2\n")?;
        assert!(!removable(&group), "6.8: forks refused to its processes");
        std::fs::write(group.join("pids.events"), "max 0\n")?;
        std::fs::write(group.join("pids.events.local"), "max 1\n")?;
        assert!(!removable(&group), "pids_localevents");
        std::fs::write(group.join("pids.events.local"), "max 0\n")?;
        std::fs::write(group.join("cgroup.procs"), "100\n")?;
        assert!(!removable(&group), "a process left");
        Ok(())
    }

    /// SPEC-191 PER-003: the groups of runs a person paused stay frozen.
    #[test]
    fn thaw_all_spares_the_groups_it_is_told_to_keep() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, tree, agents) = tree()?;
        for name in ["run-a1", "run-a2"] {
            let group = agents.join(name);
            std::fs::create_dir_all(&group)?;
            std::fs::write(group.join("cgroup.freeze"), "1")?;
            std::fs::write(group.join("cgroup.procs"), "100\n")?;
        }
        let keep = HashSet::from(["run-a2".to_owned()]);
        assert_eq!(thaw_all(&tree.unit, &keep), 1);
        assert_eq!(
            std::fs::read_to_string(agents.join("run-a1/cgroup.freeze"))?,
            "0"
        );
        assert_eq!(
            std::fs::read_to_string(agents.join("run-a2/cgroup.freeze"))?,
            "1"
        );
        Ok(())
    }

    #[test]
    fn parents_are_read_from_proc_stat() {
        let procs = Procfs;
        let me = std::process::id();
        assert_eq!(procs.parent(me), Some(std::os::unix::process::parent_id()));
    }
}
