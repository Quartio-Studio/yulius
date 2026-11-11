//! Moves the sessions of the old layout out of the server's unit (ADR-0298, SPEC-458 LAY-004).
//!
//! Until ADR-0298 sandboxes lived in the server's own delegated unit. systemd 258 and later
//! start a unit by cloning their executor into the unit's own group, which the kernel refuses
//! while that group passes controllers to children that still hold processes: the unit cannot
//! start again while one session survives. [`adopt`] runs as root before the server starts and
//! rebuilds the old tree inside the sessions unit: the same groups, limits and processes.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cgroup::{ENGINE_LEAF, LEGACY_LEAF, SANDBOXES};

/// The subgroup systemd places the server in (`DelegateSubgroup=server`).
pub const SERVER_LEAF: &str = "server";
/// Groups one call visits at most.
const MAX_GROUPS: usize = 4096;
/// How deep below the unit a group may lie (`sandboxes/<sandbox>/<leaf>/run-<name>` is 4).
const MAX_DEPTH: usize = 8;
/// What a group's settings are: copied to a group this call creates. `cgroup.freeze` keeps a
/// paused run paused: its processes arrive in a frozen group.
const SETTINGS: [&str; 6] = [
    "cpu.max",
    "cpu.weight",
    "memory.high",
    "memory.max",
    "pids.max",
    "cgroup.freeze",
];
/// How often an ended engine's group is looked at while it empties.
const ENGINE_POLL: Duration = Duration::from_millis(50);

/// What one [`adopt`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Adopted {
    /// Groups mirrored into the sessions unit.
    pub groups: usize,
    /// Processes moved.
    pub processes: usize,
    /// Docker engines ended (they start again on demand).
    pub engines: usize,
    /// Groups that still hold processes, with the reason.
    pub left: Vec<String>,
}

impl Adopted {
    /// Whether there was nothing to do.
    #[must_use]
    pub fn nothing(&self) -> bool {
        *self == Self::default()
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn populated(group: &Path) -> bool {
    lines(&group.join("cgroup.events"))
        .windows(2)
        .any(|pair| pair[0] == "populated" && pair[1] == "1")
}

fn subgroups(group: &Path) -> Vec<PathBuf> {
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

/// Gives the group `dir` and its files to `owner`: a group root creates is root's, and the
/// server must write its limits and place processes in it.
fn give(dir: &Path, owner: (u32, u32)) -> io::Result<()> {
    let (uid, gid) = owner;
    std::os::unix::fs::chown(dir, Some(uid), Some(gid))?;
    for entry in std::fs::read_dir(dir)?.flatten().take(MAX_GROUPS) {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            std::os::unix::fs::chown(entry.path(), Some(uid), Some(gid))?;
        }
    }
    Ok(())
}

/// Passes down from `to` the controllers `from` passes down.
fn pass_controllers(from: &Path, to: &Path) -> io::Result<()> {
    let wanted: Vec<String> = lines(&from.join("cgroup.subtree_control"))
        .iter()
        .map(|c| format!("+{c}"))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    std::fs::write(to.join("cgroup.subtree_control"), wanted.join(" "))
}

/// Moves the processes of `from` to `to`; one that exited meanwhile is not an error.
fn move_all(from: &Path, to: &Path, done: &mut Adopted) {
    let procs = to.join("cgroup.procs");
    for pid in lines(&from.join("cgroup.procs")).iter().take(MAX_GROUPS) {
        match std::fs::write(&procs, pid) {
            Ok(()) => done.processes += 1,
            Err(error) if error.raw_os_error() == Some(nix::libc::ESRCH) => {}
            Err(error) => done
                .left
                .push(format!("{}: process {pid}: {error}", from.display())),
        }
    }
}

/// Removes the emptied group `dir` and those below it, deepest first. A group that still holds
/// a process stays; so does a fake one in tests (it keeps its files).
fn remove_tree(dir: &Path, depth: usize) {
    if depth < MAX_DEPTH {
        for child in subgroups(dir) {
            remove_tree(&child, depth + 1);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Ends the Docker engine in `group` (SPEC-458 LAY-005): it runs in a cgroup namespace rooted
/// there and could not manage its containers from another place. The front starts it again on
/// demand, as after a reboot.
fn end_engine(group: &Path, wait: Duration, done: &mut Adopted) {
    if !populated(group) {
        remove_tree(group, 0);
        return;
    }
    done.engines += 1;
    if let Err(error) = std::fs::write(group.join("cgroup.kill"), "1") {
        done.left.push(format!(
            "{}: cannot end the engine: {error}",
            group.display()
        ));
        return;
    }
    let deadline = Instant::now() + wait;
    while populated(group) {
        if Instant::now() >= deadline {
            done.left
                .push(format!("{}: the engine did not end", group.display()));
            return;
        }
        std::thread::sleep(ENGINE_POLL);
    }
    remove_tree(group, 0);
}

fn is_engine(relative: &Path) -> bool {
    let parts: Vec<_> = relative.iter().collect();
    parts.len() == 3 && parts[0] == SANDBOXES && parts[2] == ENGINE_LEAF
}

/// Mirrors `old/relative` into `sessions/relative` and moves its processes, then its subgroups.
fn mirror(
    old: &Path,
    sessions: &Path,
    relative: &Path,
    owner: (u32, u32),
    engine_wait: Duration,
    done: &mut Adopted,
) -> io::Result<()> {
    let from = old.join(relative);
    if is_engine(relative) {
        end_engine(&from, engine_wait, done);
        return Ok(());
    }
    if done.groups >= MAX_GROUPS || relative.iter().count() > MAX_DEPTH {
        done.left
            .push(format!("{}: too many groups, not moved", from.display()));
        return Ok(());
    }
    done.groups += 1;
    let to = sessions.join(relative);
    let created = match std::fs::create_dir(&to) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    give(&to, owner)?;
    if created {
        for setting in SETTINGS {
            // A setting the group has no controller for does not exist; one the kernel refuses
            // must not cost the session its move.
            if let Ok(value) = std::fs::read_to_string(from.join(setting)) {
                let _ = std::fs::write(to.join(setting), value.trim());
            }
        }
    }
    pass_controllers(&from, &to)?;
    move_all(&from, &to, done);
    for child in subgroups(&from) {
        if let Ok(below) = child.strip_prefix(old) {
            mirror(old, sessions, below, owner, engine_wait, done)?;
        }
    }
    Ok(())
}

/// Rebuilds the groups of the server unit `old` inside the sessions unit `sessions` and moves
/// their processes: everything but the server's own subgroup, which stays when `keep_server`
/// (the unit keeps its place) and else moves to `legacy/` so that the old unit empties and
/// systemd lets go of it. Emptied groups are removed and `old` passes no controller down any
/// more, which is what lets systemd 258 start the unit. Engines are ended, waiting up to
/// `engine_wait` for each.
///
/// # Errors
/// The sessions unit cannot be read or written. Groups that keep processes are not an error:
/// they are named in [`Adopted::left`].
pub fn adopt(
    old: &Path,
    sessions: &Path,
    keep_server: bool,
    engine_wait: Duration,
) -> io::Result<Adopted> {
    let mut done = Adopted::default();
    if !old.is_dir() || old == sessions {
        return Ok(done);
    }
    let children: Vec<PathBuf> = subgroups(old)
        .into_iter()
        .filter(|dir| dir.file_name().is_some_and(|name| name != SERVER_LEAF))
        .collect();
    let server = old.join(SERVER_LEAF);
    let stray_server = !keep_server && !lines(&server.join("cgroup.procs")).is_empty();
    if children.is_empty() && !stray_server {
        return Ok(done);
    }
    let meta = std::fs::metadata(sessions)?;
    let owner = (meta.uid(), meta.gid());
    pass_controllers(old, sessions)?;
    for child in &children {
        if let Ok(relative) = child.strip_prefix(old) {
            mirror(old, sessions, relative, owner, engine_wait, &mut done)?;
        }
    }
    if stray_server {
        let legacy = sessions.join(LEGACY_LEAF);
        std::fs::create_dir_all(&legacy)?;
        give(&legacy, owner)?;
        move_all(&server, &legacy, &mut done);
    }
    for child in &children {
        remove_tree(child, 0);
    }
    // One write each: a controller a remaining group still uses must not keep the others.
    for controller in lines(&old.join("cgroup.subtree_control")) {
        let _ = std::fs::write(old.join("cgroup.subtree_control"), format!("-{controller}"));
    }
    for child in &children {
        if child.is_dir() && populated(child) {
            done.left
                .push(format!("{}: still holds processes", child.display()));
        }
    }
    Ok(done)
}

/// Waits up to `wait` for systemd to remove the emptied group `old`: a stopped unit keeps the
/// place of its group while one process is left there, whatever its `Slice=` says, and lets go
/// of it when it removes the group. Whether it is gone.
#[must_use]
pub fn wait_gone(old: &Path, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while old.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(ENGINE_POLL);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const WAIT: Duration = Duration::from_millis(200);

    /// A fake group: its directory with the given files.
    fn group(dir: &Path, files: &[(&str, &str)]) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, text) in files {
            std::fs::write(dir.join(name), text)?;
        }
        Ok(())
    }

    fn text(path: PathBuf) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The old layout: one sandbox with the split leaves, a paused run and an engine.
    fn old_layout(root: &Path) -> io::Result<(PathBuf, PathBuf)> {
        let old = root.join("system.slice/yulius.service");
        let sessions = root.join("yulius.slice/yulius-sandboxes.service");
        group(&old, &[("cgroup.subtree_control", "cpu memory pids\n")])?;
        group(&old.join("server"), &[("cgroup.procs", "")])?;
        group(
            &old.join("sandboxes"),
            &[("cgroup.subtree_control", "cpu memory pids\n")],
        )?;
        group(
            &old.join("sandboxes/s1"),
            &[
                ("cgroup.subtree_control", "cpu\n"),
                ("memory.max", "1073741824\n"),
                ("pids.max", "8192\n"),
                ("cpu.max", "200000 100000\n"),
            ],
        )?;
        group(
            &old.join("sandboxes/s1/agents"),
            &[("cgroup.procs", "41\n42\n"), ("cpu.weight", "100\n")],
        )?;
        group(
            &old.join("sandboxes/s1/agents/run-a1"),
            &[("cgroup.procs", "43\n"), ("cgroup.freeze", "1\n")],
        )?;
        group(
            &old.join("sandboxes/s1/system"),
            &[("cgroup.procs", "40\n")],
        )?;
        group(
            &old.join("sandboxes/s1/engine"),
            &[("cgroup.events", "populated 0\n")],
        )?;
        group(&sessions, &[("cgroup.subtree_control", "")])?;
        group(&sessions.join("keeper"), &[("cgroup.procs", "7\n")])?;
        Ok((old, sessions))
    }

    #[test]
    fn the_old_tree_is_rebuilt_in_the_sessions_unit() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        let done = adopt(&old, &sessions, false, WAIT)?;
        assert_eq!(done.left, Vec::<String>::new());
        assert_eq!((done.groups, done.processes, done.engines), (5, 4, 0));
        assert_eq!(
            text(sessions.join("cgroup.subtree_control")),
            "+cpu +memory +pids"
        );
        let sandbox = sessions.join("sandboxes/s1");
        assert_eq!(text(sandbox.join("cgroup.subtree_control")), "+cpu");
        assert_eq!(text(sandbox.join("memory.max")), "1073741824");
        assert_eq!(text(sandbox.join("pids.max")), "8192");
        assert_eq!(text(sandbox.join("cpu.max")), "200000 100000");
        assert_eq!(text(sandbox.join("agents/cpu.weight")), "100");
        // The fake file keeps the last process written.
        assert_eq!(text(sandbox.join("agents/cgroup.procs")), "42");
        assert_eq!(text(sandbox.join("system/cgroup.procs")), "40");
        // A paused run arrives in a frozen group.
        assert_eq!(text(sandbox.join("agents/run-a1/cgroup.freeze")), "1");
        assert_eq!(text(sandbox.join("agents/run-a1/cgroup.procs")), "43");
        assert!(!sandbox.join("engine").exists(), "an engine is not moved");
        assert!(!sessions.join("server").exists());
        // The old unit passes nothing down any more (the fake file keeps the last write).
        assert_eq!(text(old.join("cgroup.subtree_control")), "-pids");
        Ok(())
    }

    #[test]
    fn nothing_to_adopt_changes_nothing() -> TestResult {
        let root = tempfile::tempdir()?;
        let sessions = root.path().join("yulius.slice/yulius-sandboxes.service");
        group(&sessions, &[("cgroup.subtree_control", "")])?;
        // No old unit at all.
        let missing = root.path().join("system.slice/yulius.service");
        assert!(adopt(&missing, &sessions, false, WAIT)?.nothing());
        // A unit with only its server, which keeps its place.
        let unit = root.path().join("yulius.slice/yulius.service");
        group(&unit.join("server"), &[("cgroup.procs", "9\n")])?;
        assert!(adopt(&unit, &sessions, true, WAIT)?.nothing());
        assert!(adopt(&sessions, &sessions, false, WAIT)?.nothing());
        assert_eq!(text(sessions.join("cgroup.subtree_control")), "");
        Ok(())
    }

    #[test]
    fn what_the_server_left_behind_moves_when_the_unit_changes_its_place() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        std::fs::write(old.join("server/cgroup.procs"), "9\n")?;
        let moved = adopt(&old, &sessions, false, WAIT)?;
        assert_eq!(moved.processes, 5);
        assert_eq!(text(sessions.join("legacy/cgroup.procs")), "9");
        // In place, the server's subgroup is the unit's own business.
        let other = tempfile::tempdir()?;
        let (old, sessions) = old_layout(other.path())?;
        std::fs::write(old.join("server/cgroup.procs"), "9\n")?;
        assert_eq!(adopt(&old, &sessions, true, WAIT)?.processes, 4);
        assert!(!sessions.join("legacy").exists());
        Ok(())
    }

    #[test]
    fn a_second_run_keeps_the_limits_the_server_set_meanwhile() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        adopt(&old, &sessions, false, WAIT)?;
        std::fs::write(sessions.join("sandboxes/s1/pids.max"), "100")?;
        let again = adopt(&old, &sessions, false, WAIT)?;
        assert_eq!(again.left, Vec::<String>::new());
        assert_eq!(text(sessions.join("sandboxes/s1/pids.max")), "100");
        Ok(())
    }

    #[test]
    fn a_running_engine_is_ended_and_one_that_stays_is_reported() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        let engine = old.join("sandboxes/s1/engine");
        // The fake group never empties: the kill is written and the wait ends at its bound.
        group(
            &engine,
            &[("cgroup.events", "populated 1\n"), ("cgroup.kill", "")],
        )?;
        let done = adopt(&old, &sessions, false, Duration::ZERO)?;
        assert_eq!(done.engines, 1);
        assert_eq!(text(engine.join("cgroup.kill")), "1");
        assert!(
            done.left
                .iter()
                .any(|line| line.contains("engine did not end")),
            "{:?}",
            done.left
        );
        assert!(!sessions.join("sandboxes/s1/engine").exists());
        Ok(())
    }

    #[test]
    fn a_tree_deeper_than_the_bound_is_reported_not_followed() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        let mut deep = old.join("sandboxes/s1/agents");
        for level in 0..MAX_DEPTH {
            deep = deep.join(format!("d{level}"));
        }
        group(&deep, &[("cgroup.procs", "77\n")])?;
        let done = adopt(&old, &sessions, false, WAIT)?;
        assert!(
            done.left
                .iter()
                .any(|line| line.contains("too many groups")),
            "{:?}",
            done.left
        );
        Ok(())
    }

    #[test]
    fn the_wait_for_an_emptied_unit_is_bounded() -> TestResult {
        let root = tempfile::tempdir()?;
        assert!(wait_gone(&root.path().join("gone"), Duration::ZERO));
        assert!(!wait_gone(root.path(), Duration::from_millis(1)));
        Ok(())
    }

    #[test]
    fn a_missing_sessions_unit_is_an_error() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, _sessions) = old_layout(root.path())?;
        assert!(adopt(&old, &root.path().join("absent"), false, WAIT).is_err());
        Ok(())
    }
}
