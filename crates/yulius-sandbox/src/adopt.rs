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
            mirror_or_report(old, sessions, below, owner, engine_wait, done);
        }
    }
    Ok(())
}

/// [`mirror`], with a group that cannot be made or filled reported instead of ending the whole
/// move: the other sandboxes still move, since every session left behind keeps the server from
/// starting on systemd 258.
fn mirror_or_report(
    old: &Path,
    sessions: &Path,
    relative: &Path,
    owner: (u32, u32),
    engine_wait: Duration,
    done: &mut Adopted,
) {
    if let Err(error) = mirror(old, sessions, relative, owner, engine_wait, done) {
        done.left.push(format!(
            "{}: not moved: {error}",
            old.join(relative).display()
        ));
    }
}

/// Rebuilds the groups of the server unit `old` inside the sessions unit `sessions` and moves
/// their processes: everything but the server's own subgroup, which stays when `keep_server`
/// (the unit keeps its place) and else moves to `legacy/` so that the old unit empties and
/// systemd lets go of it. Emptied groups are removed and `old` passes no controller down any
/// more, which is what lets systemd 258 start the unit. Engines are ended, waiting up to
/// `engine_wait` for each.
///
/// # Errors
/// The sessions unit cannot be read or written. Groups that keep processes, or that could not
/// be made in the sessions unit, are not an error: they are named in [`Adopted::left`] and the
/// others still move.
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
            mirror_or_report(old, sessions, relative, owner, engine_wait, &mut done);
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

/// Waits up to `wait` until systemd no longer names the emptied group `old` (below `root`) as
/// its unit's group, then removes what is left of it. `shown` is what systemd names now
/// (`ControlGroup`; `None` when it cannot be asked, which is no reason to fail a start).
///
/// A stopped unit keeps the place of its group while one process is left there, whatever its
/// `Slice=` says, and lets go once the group is empty. It sweeps the emptied groups itself then,
/// and where that sweep crosses [`adopt`]'s own removals it gives the group up without removing
/// it: that the group is still there says nothing, only systemd's answer does. Whether it let go.
#[must_use]
pub fn let_go(old: &Path, root: &Path, shown: &dyn Fn() -> Option<String>, wait: Duration) -> bool {
    let name = old.strip_prefix(root).map_or_else(
        |_| old.display().to_string(),
        |relative| format!("/{}", relative.display()),
    );
    let deadline = Instant::now() + wait;
    while shown().is_some_and(|group| group.trim() == name) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(ENGINE_POLL);
    }
    // Only an empty group goes: one that holds anything is refused by the kernel.
    let _ = std::fs::remove_dir(old);
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

    /// What systemd names as the unit's group: the old one for the first `held` questions.
    fn systemd(
        held: u32,
    ) -> (
        std::rc::Rc<std::cell::Cell<u32>>,
        impl Fn() -> Option<String>,
    ) {
        let asked = std::rc::Rc::new(std::cell::Cell::new(0_u32));
        let count = std::rc::Rc::clone(&asked);
        (asked, move || {
            count.set(count.get() + 1);
            Some(if count.get() <= held {
                "/system.slice/yulius.service\n".to_owned()
            } else {
                String::new()
            })
        })
    }

    #[test]
    fn the_emptied_unit_is_let_go_once_systemd_names_it_no_more() -> TestResult {
        let root = tempfile::tempdir()?;
        let old = root.path().join("system.slice/yulius.service");
        std::fs::create_dir_all(&old)?;
        // systemd gave the group up without removing it (its sweep crossed ours): the leftover
        // goes, and that it was there is no failure.
        let (asked, shown) = systemd(2);
        assert!(let_go(&old, root.path(), &shown, Duration::from_secs(5)));
        assert_eq!(asked.get(), 3, "asked until systemd named it no more");
        assert!(!old.exists(), "the empty leftover is removed");
        // Removed by systemd already: nothing to do.
        assert!(let_go(&old, root.path(), &systemd(0).1, Duration::ZERO));
        Ok(())
    }

    #[test]
    fn a_unit_systemd_still_holds_is_not_let_go_and_the_wait_is_bounded() -> TestResult {
        let root = tempfile::tempdir()?;
        let old = root.path().join("system.slice/yulius.service");
        std::fs::create_dir_all(&old)?;
        let (_, shown) = systemd(u32::MAX);
        assert!(!let_go(&old, root.path(), &shown, Duration::from_millis(1)));
        assert!(old.exists(), "a group systemd holds is left alone");
        Ok(())
    }

    #[test]
    fn a_group_that_holds_something_stays_and_an_unanswered_question_is_no_failure() -> TestResult {
        let root = tempfile::tempdir()?;
        let old = root.path().join("system.slice/yulius.service");
        std::fs::create_dir_all(old.join("sandboxes"))?;
        assert!(let_go(&old, root.path(), &|| None, Duration::ZERO));
        assert!(old.join("sandboxes").exists());
        // Another group of the same unit is not the one asked about.
        assert!(let_go(
            &old,
            root.path(),
            &|| Some("/yulius.slice/yulius.service".to_owned()),
            Duration::ZERO
        ));
        Ok(())
    }

    /// One sandbox whose group cannot be made in the sessions unit must not cost the others
    /// their move: every session left behind keeps the server from starting on systemd 258.
    #[test]
    fn a_sandbox_that_cannot_be_moved_does_not_stop_the_others() -> TestResult {
        let root = tempfile::tempdir()?;
        let (old, sessions) = old_layout(root.path())?;
        group(
            &old.join("sandboxes/s2/agents"),
            &[("cgroup.procs", "51\n52\n")],
        )?;
        // Something that is no group stands where s1's group would go.
        group(&sessions.join("sandboxes"), &[("cgroup.procs", "")])?;
        std::fs::write(sessions.join("sandboxes/s1"), "in the way")?;
        let done = adopt(&old, &sessions, false, WAIT)?;
        assert_eq!(
            std::fs::read_to_string(sessions.join("sandboxes/s2/agents/cgroup.procs"))?,
            "52",
            "s2 moved (a fake cgroup.procs keeps the last write)"
        );
        assert_eq!(done.processes, 2, "{done:?}");
        assert!(
            done.left
                .iter()
                .any(|line| line.contains("sandboxes/s1") && line.contains("not moved")),
            "{:?}",
            done.left
        );
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
