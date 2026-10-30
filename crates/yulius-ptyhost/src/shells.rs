//! Background shells an agent left running (SPEC-001 TSK-014, SPEC-002 §6.1).
//!
//! Claude Code 2.1.292 runs every Bash tool command as
//! `<shell> -c [-l] "<snapshot> && … && eval '<command>' && pwd -P >| <cwd file>"`, as a child of
//! its own process. A foreground command ends before the turn does; one started with
//! `run_in_background` keeps its shell alive after the turn ("· 1 shell" in Claude Code's footer)
//! until it exits, Claude Code stops it at its background timeout, or the session ends.
//!
//! A *background shell* is therefore a process under the agent whose command line carries that
//! wrapper ([`SHELL_SIGNATURE`]) and which has no such shell above it (a sub-shell of a counted
//! shell is the same shell). Hooks, the status line and MCP servers run without the wrapper and
//! are never counted. A shell that only runs `yulius wait` is not counted either: the agent
//! waits for Yulius (checks, a merge, a message) and builds nothing (SPEC-050 DSK-102), so its
//! caches may go under disk pressure. Such shells are counted apart ([`ShellCount::waits`]): the
//! agent goes on by itself when Yulius has news, so its task is not the person's turn (SPEC-085
//! STT-002).
//!
//! In the sandbox the probe's root is `bwrap`, and the agent runs in a PID namespace whose init
//! adopts orphans. A foreground command's backgrounded subshell (`(server &)`) keeps the wrapper's
//! command line and outlives it. Once adopted, it is no shell of the agent's: Claude Code does not
//! track it and never wakes the agent for it. A wrapper shell whose parent is a namespace init
//! ([`ProcessEntry::ns_init`]) is skipped with everything under it (SPEC-130 DMD-001; task #108
//! read Working for good behind such a preview server). Those orphans are listed apart as
//! [`Stray`]s ([`strays`], SPEC-353 LAP-003): everything the sandbox's init adopted other than the
//! agent (its earliest-started child), with its subtree and start times, so that the server can
//! end an idle run's leftovers ([`reap_strays`]) without hitting a reused pid. The probe reads the
//! process table once:
//! on Linux it walks `/proc/<pid>/task/<tid>/children` from the agent (falling back to a scan of
//! `/proc`), elsewhere it runs `ps` with a timeout. Every read is bounded ([`MAX_PROCESSES`],
//! [`MAX_DEPTH`], [`MAX_CMDLINE_BYTES`], [`PS_TIMEOUT`]); `None` means the table could not be read.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

/// The tail of the wrapper Claude Code puts around every Bash tool command.
pub const SHELL_SIGNATURE: &[u8] = b"pwd -P >| ";

/// Most processes examined under one agent.
pub const MAX_PROCESSES: usize = 4096;

/// Deepest process examined below the agent.
pub const MAX_DEPTH: usize = 64;

/// Bytes of a command line read (Linux caps one argument at 128 KiB, and the wrapper is one).
pub const MAX_CMDLINE_BYTES: u64 = 256 * 1024;

/// How long `ps` may take where there is no `/proc`.
pub const PS_TIMEOUT: Duration = Duration::from_secs(2);

/// One row of a process table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub ppid: u32,
    /// The command line carries Claude Code's shell wrapper.
    pub shell: bool,
    /// The process is `yulius wait` ([`is_yulius_wait`]).
    pub waits: bool,
    /// The process is the init of a PID namespace (the sandbox's `bwrap`): it adopts the orphans
    /// of the processes in that namespace.
    pub ns_init: bool,
    /// When it started, in clock ticks after boot (`/proc/<pid>/stat`); 0 when unknown. With the
    /// pid it identifies the process (pids are reused).
    pub started: u64,
}

/// Whether a command line (raw `/proc` bytes or `ps` text) is a Claude Code Bash tool shell.
#[must_use]
pub fn is_agent_shell(command_line: &[u8]) -> bool {
    command_line
        .windows(SHELL_SIGNATURE.len())
        .any(|window| window == SHELL_SIGNATURE)
}

/// Whether a command line (raw `/proc` bytes or `ps` text) is `yulius wait`: the agent's command
/// that blocks until Yulius has news for it (the shim execs the server binary with `wait`).
#[must_use]
pub fn is_yulius_wait(command_line: &[u8]) -> bool {
    let mut args = command_line
        .split(|b| *b == 0 || *b == b' ')
        .filter(|arg| !arg.is_empty());
    let Some(exe) = args.next() else {
        return false;
    };
    let name = exe.rsplit(|b| *b == b'/').next().unwrap_or(exe);
    name == b"yulius" && args.next() == Some(b"wait".as_slice())
}

/// Whether a `/proc/<pid>/status` text describes the init of a PID namespace: the last pid of its
/// `NSpid` line (the pid in the innermost namespace) is 1. Without that line (an old kernel, an
/// unreadable status) it is not one.
#[must_use]
pub fn is_ns_init(status: &str) -> bool {
    status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .and_then(|pids| pids.split_whitespace().next_back())
        == Some("1")
}

/// What one probe found below an agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellCount {
    /// Background shells that run something ([`count_shells`]).
    pub shells: u32,
    /// Background shells whose only processes are `yulius wait` (DSK-102): the agent waits for
    /// Yulius and is woken when the wait ends.
    pub waits: u32,
}

/// Counts the background shells below `root` in `table` (the root itself never counts): wrapper
/// shells with no wrapper shell above them, at most [`MAX_PROCESSES`] processes and
/// [`MAX_DEPTH`] levels deep. A shell whose only processes are `yulius wait` does not count, nor
/// an orphan a namespace init adopted, nor anything under it.
#[must_use]
pub fn count_shells(table: &[ProcessEntry], root: u32) -> u32 {
    count_background(table, root).shells
}

/// [`count_shells`], with the shells that only run `yulius wait` counted apart.
#[must_use]
pub fn count_background(table: &[ProcessEntry], root: u32) -> ShellCount {
    let mut children: HashMap<u32, Vec<&ProcessEntry>> = HashMap::new();
    let mut inits = HashSet::new();
    for entry in table {
        if entry.pid != entry.ppid {
            children.entry(entry.ppid).or_default().push(entry);
        }
        if entry.ns_init {
            inits.insert(entry.pid);
        }
    }
    // Per counted shell: whether a `yulius wait` and whether anything else runs below it.
    let mut shells: HashMap<u32, (bool, bool)> = HashMap::new();
    let mut seen = HashSet::from([root]);
    let mut queue = VecDeque::from([(root, None::<u32>, 0usize)]);
    while let Some((pid, shell, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        for child in children.get(&pid).into_iter().flatten() {
            if seen.len() > MAX_PROCESSES || !seen.insert(child.pid) {
                continue;
            }
            let shell = match shell {
                Some(top) => {
                    if !child.shell
                        && let Some((waits, other)) = shells.get_mut(&top)
                    {
                        *waits |= child.waits;
                        *other |= !child.waits;
                    }
                    Some(top)
                }
                // An orphan the sandbox's init adopted: not the agent's (DMD-001).
                None if child.shell && inits.contains(&pid) => continue,
                None if child.shell => {
                    shells.insert(child.pid, (false, false));
                    Some(child.pid)
                }
                None => None,
            };
            queue.push_back((child.pid, shell, depth + 1));
        }
    }
    let waits = shells
        .values()
        .filter(|(waits, other)| *waits && !*other)
        .count();
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    ShellCount {
        shells: count(shells.len().saturating_sub(waits)),
        waits: count(waits),
    }
}

/// Background shells left under the agent process `root`; `None` when the process table cannot
/// be read (the process is gone, `/proc` is hidden, `ps` failed or timed out).
#[must_use]
pub fn background_shells(root: u32) -> Option<u32> {
    background_probe(root).map(|count| count.shells)
}

/// [`background_shells`], with the shells that only run `yulius wait` counted apart.
#[must_use]
pub fn background_probe(root: u32) -> Option<ShellCount> {
    if root <= 1 {
        return None;
    }
    let table = imp::table_under(root)?;
    Some(count_background(&table, root))
}

/// A stray: a process the run's sandbox init adopted, other than the agent, with everything below
/// it (SPEC-353 LAP-003). No shell of the agent owns it, so Claude Code neither tracks nor stops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    /// `(pid, started)` of the adopted process first, then of the processes below it.
    pub processes: Vec<(u32, u64)>,
}

/// The strays below `root` in `table` (LAP-003): the children of the run's sandbox init (a
/// namespace init directly below `root`, the outer `bwrap`) other than the agent, its
/// earliest-started child, each with its subtree. A namespace deeper down (one a command of the
/// agent made) is the agent's own, and an unsandboxed run (the root is the agent) has none. An
/// init with a child whose start time is unknown (it ended mid-probe) gives none this time: the
/// agent cannot be told apart. Bounded by [`MAX_PROCESSES`] and [`MAX_DEPTH`].
#[must_use]
pub fn strays(table: &[ProcessEntry], root: u32) -> Vec<Stray> {
    let mut children: HashMap<u32, Vec<&ProcessEntry>> = HashMap::new();
    for entry in table {
        if entry.pid != entry.ppid {
            children.entry(entry.ppid).or_default().push(entry);
        }
    }
    let kids = |pid: u32| children.get(&pid).map_or(&[][..], Vec::as_slice);
    let mut found = Vec::new();
    let mut listed = 0usize;
    for init in kids(root).iter().filter(|e| e.ns_init) {
        let adopted = kids(init.pid);
        if adopted.iter().any(|e| e.started == 0) {
            continue;
        }
        let Some(agent) = adopted.iter().min_by_key(|e| (e.started, e.pid)) else {
            continue;
        };
        for stray in adopted.iter().filter(|e| e.pid != agent.pid) {
            let mut processes = Vec::new();
            let mut below = VecDeque::from([(*stray, 0usize)]);
            let mut inside = HashSet::from([stray.pid]);
            while let Some((entry, depth)) = below.pop_front() {
                if listed >= MAX_PROCESSES {
                    break;
                }
                listed += 1;
                processes.push((entry.pid, entry.started));
                if depth + 1 >= MAX_DEPTH {
                    continue;
                }
                for child in kids(entry.pid) {
                    if inside.insert(child.pid) {
                        below.push_back((child, depth + 1));
                    }
                }
            }
            if !processes.is_empty() {
                found.push(Stray { processes });
            }
        }
    }
    found
}

/// What one probe found below an agent: its background shells and its strays.
#[must_use]
pub fn background_report(root: u32) -> Option<(ShellCount, Vec<Stray>)> {
    if root <= 1 {
        return None;
    }
    let table = imp::table_under(root)?;
    Some((count_background(&table, root), strays(&table, root)))
}

/// Kills the processes of `strays` (`SIGKILL`), each only while its pid still has the recorded
/// start time (LAP-004), so a pid another process took since the probe is never signalled. A
/// process with no known start time (0) is skipped. Returns how many were signalled.
#[must_use]
pub fn reap_strays(strays: &[Stray]) -> usize {
    strays
        .iter()
        .flat_map(|stray| &stray.processes)
        .filter(|(pid, started)| *started != 0 && imp::kill_if_started(*pid, *started))
        .count()
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{HashSet, VecDeque};
    use std::io::Read;

    use super::{
        MAX_CMDLINE_BYTES, MAX_DEPTH, MAX_PROCESSES, ProcessEntry, is_agent_shell, is_ns_init,
        is_yulius_wait,
    };

    /// Bytes of a `/proc/<pid>/status` read (a few KiB in practice).
    const MAX_STATUS_BYTES: u64 = 64 * 1024;

    /// Whether `pid` is the init of a PID namespace, from its `/proc` status ([`is_ns_init`]).
    pub(super) fn ns_init(pid: u32) -> bool {
        let mut text = String::new();
        if let Ok(file) = std::fs::File::open(format!("/proc/{pid}/status")) {
            let _ = file.take(MAX_STATUS_BYTES).read_to_string(&mut text);
        }
        is_ns_init(&text)
    }

    fn command_line(pid: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        if let Ok(file) = std::fs::File::open(format!("/proc/{pid}/cmdline")) {
            let _ = file.take(MAX_CMDLINE_BYTES).read_to_end(&mut bytes);
        }
        bytes
    }

    /// Children of every thread of `pid`, or `None` when the kernel does not list them.
    fn children(pid: u32) -> Option<Vec<u32>> {
        let mut out = Vec::new();
        let mut listed = false;
        for task in std::fs::read_dir(format!("/proc/{pid}/task"))
            .ok()?
            .flatten()
        {
            let path = task.path().join("children");
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            listed = true;
            out.extend(
                text.split_whitespace()
                    .filter_map(|p| p.parse::<u32>().ok()),
            );
            if out.len() > MAX_PROCESSES {
                break;
            }
        }
        listed.then_some(out)
    }

    /// Field `n` of `/proc/<pid>/stat` after the command name (0: the state).
    fn stat_field(pid: u32, n: usize) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.get(stat.rfind(')')? + 1..)?;
        after.split_whitespace().nth(n)?.parse().ok()
    }

    fn parent(pid: u32) -> Option<u32> {
        stat_field(pid, 1).and_then(|ppid| u32::try_from(ppid).ok())
    }

    /// When `pid` started, in clock ticks after boot.
    pub(super) fn started(pid: u32) -> Option<u64> {
        stat_field(pid, 19)
    }

    /// `SIGKILL`s `pid` if it still started at `started`; whether it was signalled. The check and
    /// the signal are two steps: only a process that ended, was reaped and had its pid handed out
    /// again between them (a wrap of the pid space within that instant) could be hit instead.
    pub(super) fn kill_if_started(pid: u32, started: u64) -> bool {
        use nix::sys::signal::{Signal, kill};
        let Some(raw) = i32::try_from(pid).ok().filter(|p| *p > 1) else {
            return false;
        };
        self::started(pid) == Some(started)
            && kill(nix::unistd::Pid::from_raw(raw), Signal::SIGKILL).is_ok()
    }

    /// The processes below `root`, walked through the kernel's child lists.
    fn walk(root: u32) -> Option<Vec<ProcessEntry>> {
        let mut table = Vec::new();
        let mut seen = HashSet::from([root]);
        let mut queue = VecDeque::from([(root, 0usize)]);
        while let Some((pid, depth)) = queue.pop_front() {
            if depth >= MAX_DEPTH {
                continue;
            }
            let Some(kids) = children(pid) else {
                if pid == root {
                    return None;
                }
                continue;
            };
            for kid in kids {
                if seen.len() > MAX_PROCESSES || !seen.insert(kid) {
                    continue;
                }
                let line = command_line(kid);
                let shell = is_agent_shell(&line);
                table.push(ProcessEntry {
                    pid: kid,
                    ppid: pid,
                    shell,
                    waits: is_yulius_wait(&line),
                    // A wrapper shell is never a namespace's init: its status is not read.
                    ns_init: !shell && ns_init(kid),
                    started: started(kid).unwrap_or(0),
                });
                queue.push_back((kid, depth + 1));
            }
        }
        Some(table)
    }

    /// Every process's parent from a scan of `/proc`; command lines only for the subtree.
    fn scan(root: u32) -> Option<Vec<ProcessEntry>> {
        let mut parents = Vec::new();
        for entry in std::fs::read_dir("/proc").ok()?.flatten().take(1 << 16) {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(ppid) = parent(pid) {
                parents.push((pid, ppid));
            }
        }
        let kinds = |pid: u32| {
            let line = command_line(pid);
            (is_agent_shell(&line), is_yulius_wait(&line))
        };
        let mut below = HashSet::from([root]);
        let mut table = Vec::new();
        // Parents may be listed after their children: repeat until nothing is added (bounded).
        for _ in 0..MAX_DEPTH {
            let before = table.len();
            for &(pid, ppid) in &parents {
                if below.contains(&ppid) && !below.contains(&pid) && table.len() < MAX_PROCESSES {
                    below.insert(pid);
                    let (shell, waits) = kinds(pid);
                    table.push(ProcessEntry {
                        pid,
                        ppid,
                        shell,
                        waits,
                        ns_init: !shell && ns_init(pid),
                        started: started(pid).unwrap_or(0),
                    });
                }
            }
            if table.len() == before {
                break;
            }
        }
        Some(table)
    }

    pub(super) fn table_under(root: u32) -> Option<Vec<ProcessEntry>> {
        if !std::path::Path::new(&format!("/proc/{root}")).exists() {
            return None;
        }
        walk(root).or_else(|| scan(root))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    use super::{PS_TIMEOUT, ProcessEntry, is_agent_shell, is_yulius_wait};

    /// Largest `ps` output read.
    const MAX_PS_BYTES: u64 = 16 * 1024 * 1024;

    /// Without `/proc` no start time is known, so nothing is reaped.
    pub(super) fn kill_if_started(_pid: u32, _started: u64) -> bool {
        false
    }

    /// Parses `ps -axww -o pid=,ppid=,command=` output.
    pub(super) fn parse_ps(text: &[u8]) -> Vec<ProcessEntry> {
        text.split(|&b| b == b'\n')
            .filter_map(|line| {
                let line = std::str::from_utf8(line).ok()?.trim_start();
                let (pid, rest) = line.split_once(char::is_whitespace)?;
                let rest = rest.trim_start();
                let (ppid, command) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                Some(ProcessEntry {
                    pid: pid.parse().ok()?,
                    ppid: ppid.parse().ok()?,
                    shell: is_agent_shell(command.as_bytes()),
                    waits: is_yulius_wait(command.as_bytes()),
                    ns_init: false,
                    started: 0,
                })
            })
            .collect()
    }

    pub(super) fn table_under(root: u32) -> Option<Vec<ProcessEntry>> {
        let mut child = Command::new("/bin/ps")
            .args(["-axww", "-o", "pid=,ppid=,command="])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        // Read on a thread so a full pipe cannot stall `ps` past the deadline.
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(MAX_PS_BYTES).read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + PS_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let bytes = reader.join().ok()?;
        status.filter(std::process::ExitStatus::success)?;
        let table = parse_ps(&bytes);
        // The agent itself must still be listed: a vanished process has no shells to report.
        table.iter().any(|e| e.pid == root).then_some(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    fn entry(pid: u32, ppid: u32, shell: bool) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid,
            shell,
            waits: false,
            ns_init: false,
            started: u64::from(pid),
        }
    }

    /// The init of a PID namespace (the sandbox's inner `bwrap`).
    fn ns_init(pid: u32, ppid: u32) -> ProcessEntry {
        ProcessEntry {
            ns_init: true,
            ..entry(pid, ppid, false)
        }
    }

    fn wait(pid: u32, ppid: u32) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid,
            shell: false,
            waits: true,
            ns_init: false,
            started: u64::from(pid),
        }
    }

    /// DSK-102: a shell that only waits for Yulius is not a running background shell; one that
    /// also builds is.
    #[test]
    fn a_shell_that_only_runs_yulius_wait_does_not_count() {
        assert!(is_yulius_wait(
            b"/usr/local/bin/yulius\0wait\0--minutes\x0030\0"
        ));
        assert!(is_yulius_wait(b"/usr/local/bin/yulius wait"));
        assert!(!is_yulius_wait(b"/usr/local/bin/yulius\0serve\0"));
        assert!(!is_yulius_wait(b"/bin/sh\0-c\0yulius wait\0"));
        let table = [
            entry(10, 1, false), // the agent
            entry(11, 10, true), // `yulius wait` in the background
            wait(12, 11),
            entry(13, 10, true), // a gate and then a wait in one shell
            entry(14, 13, false),
            wait(15, 13),
            entry(16, 10, true), // a shell that has not started its command yet
        ];
        assert_eq!(count_shells(&table, 10), 2);
        assert_eq!(count_shells(&table[..3], 10), 0);
        // SPEC-085 STT-002: the waiting shell is counted apart.
        assert_eq!(
            count_background(&table, 10),
            ShellCount {
                shells: 2,
                waits: 1
            }
        );
        assert_eq!(
            count_background(&table[..3], 10),
            ShellCount {
                shells: 0,
                waits: 1
            }
        );
    }

    #[test]
    fn the_wrapper_is_recognised_in_raw_and_ps_command_lines() {
        assert!(is_agent_shell(
            b"/bin/bash\0-c\0-l\0source /h/.claude/shell-snapshots/s.sh && eval 'npm run dev' < /dev/null && pwd -P >| /tmp/claude-1a2b-cwd\0"
        ));
        assert!(is_agent_shell(
            b"/bin/sh -c sleep 30 && pwd -P >| /tmp/claude-cwd"
        ));
        assert!(!is_agent_shell(
            b"/usr/local/bin/yulius\0hook\0claude\0Stop\0"
        ));
        assert!(!is_agent_shell(b"npm run dev"));
        assert!(!is_agent_shell(b""));
    }

    /// Only top-level wrapper shells below the root count: not the root, not their sub-shells,
    /// not helpers, not other processes' shells.
    #[test]
    fn counting_follows_the_agent_subtree() {
        let table = vec![
            entry(10, 1, false),  // the agent
            entry(11, 10, true),  // background shell
            entry(12, 11, false), // its server
            entry(13, 12, true),  // a shell the server started: inside the counted one
            entry(14, 10, false), // an MCP server
            entry(15, 10, true),  // a second background shell
            entry(20, 1, true),   // another task's shell
            entry(21, 20, true),
        ];
        assert_eq!(count_shells(&table, 10), 2);
        assert_eq!(count_shells(&table, 14), 0);
        assert_eq!(count_shells(&table, 99), 0);
        // The root's own command line is never counted; a shell below it is.
        assert_eq!(count_shells(&table, 11), 1);
        // A self-parented entry (a racy read) does not loop.
        assert_eq!(count_shells(&[entry(5, 5, true), entry(6, 5, true)], 5), 1);
    }

    /// #473 (task #108 on 2026-10-14: Done, read Working for 10 minutes until it was closed): in
    /// the sandbox the probe's root is `bwrap`. A foreground command's backgrounded subshell
    /// (`(cd web && python3 -m http.server 20100 &)`) keeps the wrapper's command line and outlives
    /// it; the namespace's init adopts it. It is no shell of the agent's: neither it nor anything
    /// under it counts, while the agent's own shells still do.
    #[test]
    fn an_orphan_the_sandbox_adopted_is_no_background_shell() {
        let table = vec![
            ns_init(11, 10),      // the sandbox's init, below the probe's root (the outer bwrap)
            entry(12, 11, false), // the agent
            entry(13, 12, true),  // its background shell (Claude Code's scrub-exec wrapper)
            entry(14, 13, true),  // the wrapper's bash: inside the counted one
            entry(20, 11, true),  // the orphaned subshell, adopted by the init
            entry(21, 20, false), // the server it started
            entry(22, 20, true),  // a subshell of the orphan
            entry(23, 11, true),  // an orphaned shell running `yulius wait`
            wait(24, 23),
        ];
        assert_eq!(
            count_background(&table, 10),
            ShellCount {
                shells: 1,
                waits: 0
            }
        );
        assert_eq!(count_shells(&table[..4], 10), 1, "the agent's shell alone");
        assert_eq!(count_shells(&table[..2], 10), 0, "nothing running");
        // Unsandboxed, the root is the agent and its shells are its children.
        assert_eq!(count_shells(&[entry(31, 30, true)], 30), 1);
    }

    /// DMD-001: the namespace init is read from the innermost `NSpid` pid.
    #[test]
    fn a_namespace_init_is_read_from_its_status() {
        let status = |nspid: &str| format!("Name:\tbwrap\nTgid:\t4242\n{nspid}\nPPid:\t4241\n");
        assert!(is_ns_init(&status("NSpid:\t4242\t1")), "the sandbox's init");
        assert!(is_ns_init(&status("NSpid:\t1")), "the host's init");
        assert!(
            !is_ns_init(&status("NSpid:\t4243\t2")),
            "the agent in the sandbox"
        );
        assert!(!is_ns_init(&status("NSpid:\t4243")), "a host process");
        assert!(
            !is_ns_init(&status("NSpid:\t11\t4")),
            "pid 11 on the host is not pid 1"
        );
        assert!(!is_ns_init(""), "no status");
        assert!(!is_ns_init("Name:\tx\nPid:\t1\n"), "no NSpid line");
    }

    /// DMD-001 on a real `/proc`: pid 1 of this namespace is its init, this process is not.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_namespace_init_is_found_in_proc() {
        assert!(!imp::ns_init(std::process::id()), "the test process");
        // Pid 1's status may be hidden (`hidepid`); when it is readable, it is the init.
        if std::fs::read_to_string("/proc/1/status").is_ok_and(|s| s.contains("NSpid:")) {
            assert!(imp::ns_init(1), "this namespace's init");
        }
    }

    #[test]
    fn counting_is_bounded_in_depth_and_size() -> Result<(), std::num::TryFromIntError> {
        let mut chain = vec![entry(1000, 1, false)];
        for pid in 1001..1001 + 200 {
            chain.push(entry(pid, pid - 1, false));
        }
        chain.push(entry(5000, 1200, true));
        assert_eq!(count_shells(&chain, 1000), 0, "past MAX_DEPTH");
        let mut wide = vec![entry(2, 1, false)];
        let bound = u32::try_from(MAX_PROCESSES)?;
        for pid in 10..10 + bound + 50 {
            wide.push(entry(pid, 2, true));
        }
        assert!(count_shells(&wide, 2) <= bound + 1);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn ps_output_parses() {
        let rows = imp::parse_ps(
            b"  1     0 /sbin/launchd\n 42     1 /bin/sh -c sleep 3 && pwd -P >| /tmp/c\nbad\n 43    42\n",
        );
        // `ps` gives no start time.
        let unknown = |e: ProcessEntry| ProcessEntry { started: 0, ..e };
        assert_eq!(
            rows,
            vec![
                unknown(entry(1, 0, false)),
                unknown(entry(42, 1, true)),
                unknown(entry(43, 42, false))
            ]
        );
    }

    /// A real process tree: a wrapper shell under a parent counts while it runs, then not.
    #[test]
    fn a_running_wrapper_shell_is_found_until_it_exits() -> Result<(), Box<dyn std::error::Error>> {
        // The parent runs the wrapper shell in the foreground (`; true` keeps it from exec'ing).
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "/bin/sh -c 'sleep 2 && pwd -P >| /dev/null' ; true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let root = parent.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while Instant::now() < deadline {
            seen = background_shells(root);
            if seen == Some(1) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(seen, Some(1), "the running wrapper shell");
        parent.wait()?;
        assert_eq!(background_shells(root), None, "the root is gone");
        assert_eq!(background_shells(0), None);
        Ok(())
    }

    /// The pids of each stray, in order.
    fn pids(found: &[Stray]) -> Vec<Vec<u32>> {
        found
            .iter()
            .map(|stray| stray.processes.iter().map(|(pid, _)| *pid).collect())
            .collect()
    }

    /// SPEC-353 LAP-003, the quartio-v2 sandbox on 2026-10-30: about 60 detached test jobs
    /// (`sh -c "if command -v bash …"` running `bash /tmp/.tmpXXXXXX/.jobs/job-<hex>.sh`, each
    /// polling `sleep 0.05`) whose test had ended, adopted by the sandbox's init, beside the agent
    /// and its own background shell. Every job is a stray with its tree; the agent, its shells and
    /// a live turn's commands never are.
    #[test]
    fn jobs_the_sandbox_init_adopted_are_strays() {
        let mut table = vec![
            ns_init(11, 10),      // the sandbox's init, below the probe's root (the outer bwrap)
            entry(12, 11, false), // the agent
            entry(13, 12, true),  // its background shell
            entry(14, 13, false), // what the shell runs
        ];
        for job in 0..60 {
            let sh = 100 + 3 * job;
            table.push(entry(sh, 11, false)); // sh -c "if command -v bash …"
            table.push(entry(sh + 1, sh, false)); // bash …/.jobs/job-<hex>.sh
            table.push(entry(sh + 2, sh + 1, false)); // sleep 0.05
        }
        let found = strays(&table, 10);
        assert_eq!(found.len(), 60, "{found:?}");
        assert_eq!(found[0].processes, vec![(100, 100), (101, 101), (102, 102)]);
        assert!(
            found
                .iter()
                .flat_map(|stray| &stray.processes)
                .all(|(pid, _)| *pid >= 100),
            "the agent and its shells are no strays"
        );
        // They are no background shells either (DMD-001): the agent's own shell alone counts.
        assert_eq!(count_shells(&table, 10), 1);
    }

    /// LAP-003: the agent is the init's earliest-started child, whatever the pids (they wrap);
    /// a namespace inside the agent (its own `bwrap` or `unshare`) adopts the agent's processes,
    /// which stay the agent's; without a sandbox init there are no strays.
    #[test]
    fn the_agent_is_the_init_s_first_child_and_inner_namespaces_are_its_own() {
        let table = vec![
            ns_init(11, 10),
            ProcessEntry {
                started: 50,
                ..entry(900, 11, false) // the agent, at a high pid
            },
            ProcessEntry {
                started: 400,
                ..entry(20, 11, true) // an orphaned preview subshell (DMD-001), at a low pid
            },
            ProcessEntry {
                started: 401,
                ..entry(21, 20, false) // the server it started
            },
            ProcessEntry {
                started: 60,
                ..entry(901, 900, true)
            },
            ProcessEntry {
                started: 61,
                ..ns_init(902, 901) // a namespace the agent's command made
            },
            ProcessEntry {
                started: 62,
                ..entry(903, 902, false)
            },
            ProcessEntry {
                started: 63,
                ..entry(904, 902, false) // an orphan inside it: the agent's still
            },
        ];
        assert_eq!(pids(&strays(&table, 10)), vec![vec![20, 21]]);
        // Unsandboxed, the root is the agent: nothing is adopted below it.
        assert!(strays(&[entry(31, 30, true), entry(32, 31, false)], 30).is_empty());
        // Nothing but the agent below the init.
        assert!(strays(&table[..2], 10).is_empty());
    }

    /// LAP-004 on real processes: a stray is killed only while its pid has the recorded start.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stray_is_reaped_only_while_its_start_time_matches()
    -> Result<(), Box<dyn std::error::Error>> {
        /// Kills the process when the test ends, passed or failed: a test leaves nothing behind.
        struct Reaped(std::process::Child);
        impl Drop for Reaped {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Reaped(
            Command::new("/bin/sleep")
                .arg("600")
                .stdin(Stdio::null())
                .spawn()?,
        );
        let pid = child.0.id();
        let started = imp::started(pid).ok_or("no start time")?;
        let reused = Stray {
            processes: vec![(pid, started + 1)],
        };
        assert_eq!(
            reap_strays(&[reused]),
            0,
            "another process now has that pid"
        );
        assert!(child.0.try_wait()?.is_none(), "not signalled");
        let stray = Stray {
            processes: vec![(pid, started), (u32::MAX - 1, 1)],
        };
        assert_eq!(reap_strays(&[stray]), 1);
        let status = child.0.wait()?;
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(status.signal(), Some(9), "{status:?}");
        Ok(())
    }

    /// LAP-003 (review round 1): unsandboxed, the probe's root is the agent; a namespace a command
    /// of the agent made (`unshare --pid --fork`) adopts the agent's own processes, which are no
    /// strays. Only an init directly below the root is the run's sandbox.
    #[test]
    fn a_namespace_below_an_unsandboxed_agent_has_no_strays() {
        let table = vec![
            entry(31, 30, true),  // the agent's background shell
            ns_init(32, 31),      // unshare --pid --fork
            entry(33, 32, false), // its first child
            entry(34, 32, false), // a second one, adopted by that init
        ];
        assert!(strays(&table, 30).is_empty(), "{:?}", strays(&table, 30));
    }

    /// LAP-003 (review round 1): a child of the sandbox's init whose start time could not be read
    /// (it ended mid-probe) makes the agent unknown: that init gives no strays this time.
    #[test]
    fn an_unknown_start_time_gives_no_strays() {
        let table = vec![
            ns_init(11, 10),
            entry(12, 11, false), // the agent
            entry(13, 12, true),
            ProcessEntry {
                started: 0,
                ..entry(20, 11, false)
            },
            entry(21, 11, false),
        ];
        assert!(strays(&table, 10).is_empty(), "{:?}", strays(&table, 10));
    }

    /// LAP-003 on a real sandbox (review round 1): under `bwrap --unshare-pid`, a process whose
    /// parent ended is adopted by the namespace's init and found as a stray, with its real start
    /// time; the agent is not. Skipped where bwrap cannot create namespaces (unprivileged
    /// sandboxes).
    #[cfg(target_os = "linux")]
    #[test]
    fn an_orphan_in_a_real_sandbox_is_a_stray() -> Result<(), Box<dyn std::error::Error>> {
        let args = [
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
        ];
        let probe = Command::new("bwrap")
            .args(args)
            .args(["--", "/bin/true"])
            .stderr(Stdio::null())
            .status();
        if !probe.is_ok_and(|status| status.success()) {
            eprintln!("SKIPPED an_orphan_in_a_real_sandbox_is_a_stray: bwrap cannot run here");
            return Ok(());
        }
        /// Ends the sandbox (and with its init everything in it) when the test ends.
        struct Sandbox(std::process::Child);
        impl Drop for Sandbox {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        // The agent backgrounds a `sleep` from a subshell that exits at once: the init adopts it.
        let sandbox = Sandbox(
            Command::new("bwrap")
                .args(args)
                .args(["--", "/bin/sh", "-c", "(sleep 600 &); exec sleep 601"])
                .stdin(Stdio::null())
                .spawn()?,
        );
        let root = sandbox.0.id();
        let deadline = Instant::now() + Duration::from_secs(10);
        let found = loop {
            let found = background_report(root)
                .map(|(_, strays)| strays)
                .unwrap_or_default();
            if found.len() == 1 || Instant::now() >= deadline {
                break found;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(found.len(), 1, "{found:?}");
        let (pid, started) = found[0].processes[0];
        assert!(started > 0);
        let line = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))?;
        assert!(line.starts_with("sleep\0600"), "{line:?}");
        assert_eq!(reap_strays(&found), 1);
        Ok(())
    }
}
