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
//! task reads as waiting and its caches may go under disk pressure. The probe reads the process table once: on Linux it walks
//! `/proc/<pid>/task/<tid>/children` from the agent (falling back to a scan of `/proc`), elsewhere
//! it runs `ps` with a timeout. Every read is bounded ([`MAX_PROCESSES`], [`MAX_DEPTH`],
//! [`MAX_CMDLINE_BYTES`], [`PS_TIMEOUT`]); `None` means the table could not be read.

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

/// Counts the background shells below `root` in `table` (the root itself never counts): wrapper
/// shells with no wrapper shell above them, at most [`MAX_PROCESSES`] processes and
/// [`MAX_DEPTH`] levels deep. A shell whose only processes are `yulius wait` does not count.
#[must_use]
pub fn count_shells(table: &[ProcessEntry], root: u32) -> u32 {
    let mut children: HashMap<u32, Vec<&ProcessEntry>> = HashMap::new();
    for entry in table {
        if entry.pid != entry.ppid {
            children.entry(entry.ppid).or_default().push(entry);
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
                None if child.shell => {
                    shells.insert(child.pid, (false, false));
                    Some(child.pid)
                }
                None => None,
            };
            queue.push_back((child.pid, shell, depth + 1));
        }
    }
    let counted = shells
        .values()
        .filter(|(waits, other)| !(*waits && !*other))
        .count();
    u32::try_from(counted).unwrap_or(u32::MAX)
}

/// Background shells left under the agent process `root`; `None` when the process table cannot
/// be read (the process is gone, `/proc` is hidden, `ps` failed or timed out).
#[must_use]
pub fn background_shells(root: u32) -> Option<u32> {
    if root <= 1 {
        return None;
    }
    let table = imp::table_under(root)?;
    Some(count_shells(&table, root))
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{HashSet, VecDeque};
    use std::io::Read;

    use super::{
        MAX_CMDLINE_BYTES, MAX_DEPTH, MAX_PROCESSES, ProcessEntry, is_agent_shell, is_yulius_wait,
    };

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

    fn parent(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.get(stat.rfind(')')? + 1..)?;
        after.split_whitespace().nth(1)?.parse().ok()
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
                table.push(ProcessEntry {
                    pid: kid,
                    ppid: pid,
                    shell: is_agent_shell(&line),
                    waits: is_yulius_wait(&line),
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
        }
    }

    fn wait(pid: u32, ppid: u32) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid,
            shell: false,
            waits: true,
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
        assert_eq!(
            rows,
            vec![entry(1, 0, false), entry(42, 1, true), entry(43, 42, false)]
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
}
