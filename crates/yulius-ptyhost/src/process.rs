//! Process identity and liveness probes used by the host and by reconciliation (DATA-002).
//!
//! A process id alone is not an identity: ids are reused. A recorded process is considered the
//! same process only when it exists, is owned by us (signal 0 succeeds), is not a zombie, started
//! no later than the recorded start instant (plus clock granularity slack) and keeps its
//! group/session leadership.

use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::{Pid, getpgid, getsid};

/// Slack for start-time comparisons: covers second-granularity clocks and rounding.
const START_SLACK_MS: i64 = 2_000;

pub(crate) fn pid(raw: u32) -> Option<Pid> {
    i32::try_from(raw)
        .ok()
        .filter(|p| *p > 1)
        .map(Pid::from_raw)
}

/// The process exists, is signalable by us and is not a zombie.
pub(crate) fn alive(pid: Pid) -> bool {
    kill(pid, None).is_ok() && !is_zombie(pid)
}

/// `pid` exists, started no later than `recorded_ms` + slack and still leads its own process
/// group (`group_leader`) or session (`!group_leader`).
pub(crate) fn matches_record(pid: Pid, recorded_ms: i64, group_leader: bool) -> bool {
    if !alive(pid) {
        return false;
    }
    let Some(started) = start_time_ms(pid) else {
        return false;
    };
    if started > recorded_ms.saturating_add(START_SLACK_MS) {
        return false;
    }
    let leader = if group_leader {
        getpgid(Some(pid))
    } else {
        getsid(Some(pid))
    };
    leader == Ok(pid)
}

/// Sends `signal` to the process group `pgid`; a vanished group is not an error.
pub(crate) fn signal_group(pgid: Pid, signal: Signal) -> Result<(), Errno> {
    match killpg(pgid, signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(errno) => Err(errno),
    }
}

/// Polls until none of `pids` is [`alive`], at most `timeout`. Returns the first survivor.
pub(crate) fn wait_gone(pids: &[Pid], timeout: Duration) -> Option<Pid> {
    let deadline = Instant::now() + timeout;
    loop {
        let survivor = pids.iter().copied().find(|p| alive(*p));
        match survivor {
            None => return None,
            Some(pid) if Instant::now() >= deadline => return Some(pid),
            Some(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Pid;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::{SysconfVar, sysconf};

    /// Fields after the `(comm)` of `/proc/<pid>/stat`, starting at field 3 (state).
    fn stat_fields(pid: i32) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = text.get(text.rfind(')')? + 1..)?;
        Some(after.split_whitespace().map(str::to_owned).collect())
    }

    pub(crate) fn is_zombie(pid: Pid) -> bool {
        stat_fields(pid.as_raw())
            .and_then(|f| f.first().cloned())
            .is_some_and(|state| state == "Z" || state == "X")
    }

    pub(crate) fn start_time_ms(pid: Pid) -> Option<i64> {
        let fields = stat_fields(pid.as_raw())?;
        let ticks: i64 = fields.get(19)?.parse().ok()?;
        let hz = sysconf(SysconfVar::CLK_TCK).ok()??;
        if hz <= 0 {
            return None;
        }
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let btime: i64 = stat
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()?;
        Some(btime * 1000 + ticks * 1000 / hz)
    }

    /// SIGKILLs every process whose session id is `sid` (except `except`), scanning `/proc`.
    pub(crate) fn kill_session(sid: Pid, except: Pid) {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten().take(1 << 20) {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            if pid == except.as_raw() || pid <= 1 {
                continue;
            }
            let session = stat_fields(pid)
                .and_then(|f| f.get(3).cloned())
                .and_then(|s| s.parse::<i32>().ok());
            if session == Some(sid.as_raw()) {
                let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Pid;
    use std::process::{Command, Stdio};

    fn ps(pid: Pid, column: &str) -> Option<String> {
        let output = Command::new("/bin/ps")
            .args(["-o", column, "-p", &pid.as_raw().to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    pub(crate) fn is_zombie(pid: Pid) -> bool {
        ps(pid, "stat=").is_some_and(|stat| stat.starts_with('Z'))
    }

    /// Parses `ps` elapsed time `[[dd-]hh:]mm:ss` into seconds.
    pub(crate) fn parse_etime(text: &str) -> Option<i64> {
        let (days, clock) = match text.split_once('-') {
            Some((days, clock)) => (days.parse::<i64>().ok()?, clock),
            None => (0, text),
        };
        let mut seconds = 0i64;
        let parts: Vec<&str> = clock.split(':').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        for part in parts {
            seconds = seconds * 60 + part.parse::<i64>().ok()?;
        }
        Some(days * 86_400 + seconds)
    }

    pub(crate) fn start_time_ms(pid: Pid) -> Option<i64> {
        let elapsed = parse_etime(&ps(pid, "etime=")?)?;
        Some(crate::files::now_ms() - elapsed * 1000)
    }

    /// Session sweeping needs `/proc`; process groups are the cleanup unit elsewhere.
    pub(crate) fn kill_session(_sid: Pid, _except: Pid) {}
}

pub(crate) use imp::{is_zombie, kill_session, start_time_ms};

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn own_process_matches_only_with_a_plausible_start_instant() -> TestResult {
        let me = Pid::from_raw(i32::try_from(std::process::id())?);
        assert!(alive(me));
        let started = start_time_ms(me).ok_or("no start time")?;
        let now = crate::files::now_ms();
        assert!(started <= now + START_SLACK_MS && started > now - 3_600_000);
        // A record written before this process existed cannot describe it (reused pid).
        assert!(!matches_record(me, started - 60_000, true));
        assert!(!matches_record(me, started - 60_000, false));
        Ok(())
    }

    #[test]
    fn reaped_process_is_not_alive_and_zombie_is_detected() -> TestResult {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .spawn()?;
        let pid = Pid::from_raw(i32::try_from(child.id())?);
        // Before reaping the exited child is a zombie: it must not count as alive.
        assert!(wait_gone(&[pid], Duration::from_secs(5)).is_none());
        child.wait()?;
        assert!(!alive(pid));
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn etime_parsing_covers_all_formats() {
        assert_eq!(imp::parse_etime("00:05"), Some(5));
        assert_eq!(imp::parse_etime("01:02:03"), Some(3723));
        assert_eq!(imp::parse_etime("2-00:00:01"), Some(172_801));
        assert_eq!(imp::parse_etime("x"), None);
    }
}
