//! Restart reconciliation of run directories (SPEC-002 §7.1, DATA-002, AC-NFR-2).
//!
//! Reconciliation only observes and, when asked, terminates; it never respawns anything.

use std::path::Path;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use yulius_contracts::ptyhost::{ExitRecord, ProcessUsage, STATUS_FILE, StatusRecord};

use crate::files::{read_exit, read_exit_file, read_status};
use crate::process::{self, kill_session, matches_record, signal_group, wait_gone};

/// How long [`terminate_recorded`] waits for killed processes to disappear.
const TERMINATE_WAIT: Duration = Duration::from_secs(2);

/// What a run directory says about its run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDirState {
    /// No readable `exit.json` or `status.json`: the host never reported a running child.
    Missing,
    /// The run ended; `exit.json` holds its outcome.
    Exited(ExitRecord),
    /// The host reported a running child but no outcome yet.
    Recorded {
        /// The status record.
        status: StatusRecord,
        /// The recorded host process still exists with a matching identity.
        host_alive: bool,
        /// The recorded child process still exists with a matching identity.
        child_alive: bool,
        /// The kernel boot id equals the recorded one; otherwise both are certainly gone.
        same_boot: bool,
    },
}

/// The current kernel boot identifier: Linux `/proc/sys/kernel/random/boot_id`, macOS
/// `kern.boottime`; `None` when unavailable (then nothing is ever considered alive).
#[must_use]
pub fn current_boot_id() -> Option<String> {
    boot_id_impl()
}

#[cfg(target_os = "linux")]
fn boot_id_impl() -> Option<String> {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

#[cfg(target_os = "macos")]
fn boot_id_impl() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.boottime"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_boottime(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn boot_id_impl() -> Option<String> {
    None
}

/// Parses `{ sec = 1759820000, usec = 123456 } ...` into `boottime:<sec>.<usec>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_boottime(text: &str) -> Option<String> {
    let field = |name: &str| -> Option<u64> {
        let start = text.find(name)? + name.len();
        let digits: String = text
            .get(start..)?
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    Some(format!(
        "boottime:{}.{:06}",
        field("sec =")?,
        field("usec =")?
    ))
}

/// Classifies a run directory for restart reconciliation.
///
/// `exit.json` wins over `status.json`. Liveness requires the same boot id and a process
/// identity matching the record (exists, owned by us, not a zombie, started no later than the
/// record, still leading its process group (host) or session (child)), so a reused pid is not
/// mistaken for the run.
#[must_use]
pub fn inspect_run_dir(run_dir: &Path) -> RunDirState {
    if let Ok(Some(exit)) = read_exit(run_dir) {
        return RunDirState::Exited(exit);
    }
    let Ok(Some(status)) = read_status(&run_dir.join(STATUS_FILE)) else {
        return RunDirState::Missing;
    };
    let same_boot = current_boot_id().is_some_and(|id| id == status.boot_id);
    let host_alive = same_boot && host_matches(&status).is_some();
    let child_alive = same_boot && child_matches(&status).is_some();
    RunDirState::Recorded {
        status,
        host_alive,
        child_alive,
        same_boot,
    }
}

/// What the run in `run_dir` used, as its host recorded in `exit.json` once it reaped the child
/// (SPEC-154 USE-001); `None` while it runs, when it never ran or from an older host.
#[must_use]
pub fn exit_usage(run_dir: &Path) -> Option<ProcessUsage> {
    read_exit_file(run_dir).ok().flatten()?.usage
}

fn host_matches(status: &StatusRecord) -> Option<Pid> {
    process::pid(status.ptyhost_pid).filter(|pid| matches_record(*pid, status.started_at_ms, true))
}

fn child_matches(status: &StatusRecord) -> Option<Pid> {
    process::pid(status.child_pid).filter(|pid| matches_record(*pid, status.started_at_ms, false))
}

/// Failure to terminate a recorded run.
#[derive(Debug, thiserror::Error)]
pub enum TerminateError {
    /// A signal could not be delivered.
    #[error("cannot signal process {pid}: {errno}")]
    Signal {
        /// Target process (group) id.
        pid: u32,
        /// Cause.
        errno: Errno,
    },
    /// A killed process was still alive after the bounded wait.
    #[error("process {pid} still alive after SIGKILL")]
    StillAlive {
        /// The survivor.
        pid: u32,
    },
}

/// Kills a recorded run: the child's process group (and, on Linux, its session) first, then the
/// host. Only processes whose identity matches the record (see [`inspect_run_dir`]) are
/// signalled; a record from another boot needs nothing. Waits up to 2 s for both to disappear.
///
/// # Errors
/// Fails when a signal cannot be delivered or a process survives the bounded wait.
pub fn terminate_recorded(status: &StatusRecord) -> Result<(), TerminateError> {
    if current_boot_id().is_none_or(|id| id != status.boot_id) {
        return Ok(());
    }
    let mut targets = Vec::new();
    let host = host_matches(status);
    if let Some(child) = child_matches(status) {
        signal_group(child, Signal::SIGKILL).map_err(|errno| TerminateError::Signal {
            pid: status.child_pid,
            errno,
        })?;
        kill_session(child, host.unwrap_or(child));
        targets.push(child);
    }
    if let Some(host) = host {
        signal_group(host, Signal::SIGKILL).map_err(|errno| TerminateError::Signal {
            pid: status.ptyhost_pid,
            errno,
        })?;
        targets.push(host);
    }
    match wait_gone(&targets, TERMINATE_WAIT) {
        Some(survivor) => Err(TerminateError::StillAlive {
            pid: u32::try_from(survivor.as_raw()).unwrap_or(0),
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boottime_output_is_parsed() {
        assert_eq!(
            parse_boottime("{ sec = 1791197076, usec = 533771 } Mon Oct  5 12:44:36 2026\n"),
            Some("boottime:1791197076.533771".into())
        );
        assert_eq!(parse_boottime("garbage"), None);
    }

    #[test]
    fn boot_id_is_stable_within_a_boot() {
        assert_eq!(current_boot_id(), current_boot_id());
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(current_boot_id().is_some());
        }
    }
}
