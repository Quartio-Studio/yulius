//! Chrome's profile locks (SPEC-593, ADR-0432): `SingletonLock` is a symbolic link to
//! `<hostname>-<pid>`, beside `SingletonCookie` and `SingletonSocket`. Chrome refuses a profile
//! locked by another host, so a profile carried from one (an instance move, a restore) never
//! started. Before a start, the entries are removed when no running browser can hold them.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use tracing::info;

use crate::error::BrowserError;

/// Chrome's process-singleton entries of a profile, the lock first.
pub(crate) const SINGLETON_ENTRIES: [&str; 3] =
    ["SingletonLock", "SingletonCookie", "SingletonSocket"];

/// What a profile's `SingletonLock` says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lock {
    /// No lock.
    Absent,
    /// Not a `<host>-<pid>` link (Chrome removes such a lock itself).
    Invalid,
    /// Held by `pid` on `host`.
    Held { host: Vec<u8>, pid: u32 },
}

/// Splits a lock target at its last `-`, as Chrome does.
fn parse_lock(target: &OsStr) -> Lock {
    let bytes = target.as_bytes();
    let Some(at) = bytes.iter().rposition(|&b| b == b'-') else {
        return Lock::Invalid;
    };
    let (host, pid) = (&bytes[..at], &bytes[at + 1..]);
    match std::str::from_utf8(pid)
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    {
        Some(pid) if pid > 0 && !host.is_empty() => Lock::Held {
            host: host.to_vec(),
            pid,
        },
        _ => Lock::Invalid,
    }
}

async fn read_lock(profile_dir: &Path) -> Result<Lock, BrowserError> {
    let path = profile_dir.join(SINGLETON_ENTRIES[0]);
    match tokio::fs::read_link(&path).await {
        Ok(target) => Ok(parse_lock(target.as_os_str())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Lock::Absent),
        // Not a link (EINVAL): a file or directory where the lock belongs.
        Err(error) if error.raw_os_error() == Some(Errno::EINVAL as i32) => Ok(Lock::Invalid),
        Err(error) => Err(BrowserError::Profile(io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", path.display()),
        ))),
    }
}

/// Whether `pid` names a running process (one of another user counts).
pub(crate) fn process_running(pid: u32) -> bool {
    match i32::try_from(pid) {
        Ok(raw) if raw > 0 => matches!(kill(Pid::from_raw(raw), None), Ok(()) | Err(Errno::EPERM)),
        _ => false,
    }
}

/// This host's name as Chrome writes it into its lock (`gethostname`).
pub(crate) fn host_name() -> Result<Vec<u8>, BrowserError> {
    nix::unistd::gethostname()
        .map(|name| name.as_bytes().to_vec())
        .map_err(|errno| {
            BrowserError::Profile(io::Error::new(
                io::Error::from(errno).kind(),
                format!(
                    "could not read this host's name to check the browser profile's lock: {errno}"
                ),
            ))
        })
}

/// Whether the profile's lock still names `pid` of this host (after a failed start: Chrome
/// replaces a lock whose pid is no Chrome with its own).
pub(crate) async fn still_held(profile_dir: &Path, this_host: &[u8], pid: u32) -> bool {
    matches!(read_lock(profile_dir).await, Ok(Lock::Held { host, pid: held }) if host == this_host && held == pid)
}

/// Removes the profile's Chrome lock entries unless a running process of this host holds the
/// lock; returns that process's pid when the lock is kept (Chrome then decides).
///
/// # Errors
/// [`BrowserError::Profile`] when the lock cannot be read or a stale entry cannot be removed.
pub(crate) async fn clear_stale_locks(
    profile_dir: &Path,
    this_host: &[u8],
    running: impl Fn(u32) -> bool,
) -> Result<Option<u32>, BrowserError> {
    let lock = read_lock(profile_dir).await?;
    let why = match &lock {
        Lock::Held { host, pid } if host.as_slice() == this_host => {
            if running(*pid) {
                return Ok(Some(*pid));
            }
            format!("left by process {pid} of this host, which is not running")
        }
        Lock::Held { host, pid } => format!(
            "locked by process {pid} on another host ({})",
            String::from_utf8_lossy(host)
        ),
        Lock::Invalid => "not a Chrome lock of the form <host>-<pid>".to_owned(),
        Lock::Absent => "without a lock".to_owned(),
    };
    let mut removed = Vec::new();
    for name in SINGLETON_ENTRIES {
        let path = profile_dir.join(name);
        // Removes the link itself: its target (a socket in the old host's /tmp) is gone.
        match tokio::fs::remove_file(&path).await {
            Ok(()) => removed.push(name),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(BrowserError::Profile(io::Error::new(
                    error.kind(),
                    format!(
                        "the browser profile {} is {why}, and its stale {name} could not be removed: {error}",
                        profile_dir.display()
                    ),
                )));
            }
        }
    }
    if !removed.is_empty() {
        info!(profile = %profile_dir.display(), entries = ?removed, reason = %why, "stale Chrome profile locks removed");
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const HOST: &[u8] = b"new-host";

    fn link(dir: &Path, name: &str, target: &str) -> TestResult {
        std::os::unix::fs::symlink(target, dir.join(name))?;
        Ok(())
    }

    fn entries(dir: &Path) -> Vec<&'static str> {
        SINGLETON_ENTRIES
            .into_iter()
            .filter(|name| std::fs::symlink_metadata(dir.join(name)).is_ok())
            .collect()
    }

    fn all_three(dir: &Path, lock: &str) -> TestResult {
        link(dir, "SingletonLock", lock)?;
        link(
            dir,
            "SingletonSocket",
            "/tmp/.org.chromium.Chromium.gone/SingletonSocket",
        )?;
        link(dir, "SingletonCookie", "8412907261")?;
        Ok(())
    }

    #[test]
    fn a_lock_splits_at_its_last_dash_like_chrome() {
        let held = |host: &str, pid| Lock::Held {
            host: host.as_bytes().to_vec(),
            pid,
        };
        assert_eq!(
            parse_lock(OsStr::new("old-host-4242")),
            held("old-host", 4242)
        );
        assert_eq!(parse_lock(OsStr::new("h-1")), held("h", 1));
        assert_eq!(parse_lock(OsStr::new("host--1")), held("host-", 1));
        for invalid in ["", "nohyphen", "host-", "-42", "host-0", "host-x1"] {
            assert_eq!(parse_lock(OsStr::new(invalid)), Lock::Invalid, "{invalid}");
        }
    }

    /// SPEC-593 AC-1: another host's lock goes, with its dangling socket and cookie links.
    #[tokio::test]
    async fn another_hosts_lock_and_its_dangling_links_are_removed() -> TestResult {
        let dir = tempfile::tempdir()?;
        all_three(dir.path(), "old-host-4242")?;
        let kept = clear_stale_locks(dir.path(), HOST, |_| true).await?;
        assert_eq!(
            kept, None,
            "a pid of another host says nothing about this one"
        );
        assert!(entries(dir.path()).is_empty());
        Ok(())
    }

    /// SPEC-593 AC-2: this host's lock of a process that is not running goes.
    #[tokio::test]
    async fn this_hosts_lock_of_a_stopped_process_is_removed() -> TestResult {
        let dir = tempfile::tempdir()?;
        all_three(dir.path(), "new-host-4242")?;
        assert_eq!(clear_stale_locks(dir.path(), HOST, |_| false).await?, None);
        assert!(entries(dir.path()).is_empty());
        Ok(())
    }

    /// SPEC-593 AC-3: this host's lock of a running process stays, with its links.
    #[tokio::test]
    async fn this_hosts_lock_of_a_running_process_is_kept() -> TestResult {
        let dir = tempfile::tempdir()?;
        all_three(dir.path(), "new-host-4242")?;
        let kept = clear_stale_locks(dir.path(), HOST, |pid| pid == 4242).await?;
        assert_eq!(kept, Some(4242));
        assert_eq!(entries(dir.path()), SINGLETON_ENTRIES.to_vec());
        Ok(())
    }

    /// SPEC-593 AC-2: a lock that is no `<host>-<pid>` link, and links left without a lock.
    #[tokio::test]
    async fn invalid_locks_and_orphaned_links_are_removed() -> TestResult {
        let dir = tempfile::tempdir()?;
        all_three(dir.path(), "garbage")?;
        assert_eq!(clear_stale_locks(dir.path(), HOST, |_| true).await?, None);
        assert!(entries(dir.path()).is_empty());

        std::fs::write(dir.path().join("SingletonLock"), b"not a link")?;
        assert_eq!(clear_stale_locks(dir.path(), HOST, |_| true).await?, None);
        assert!(entries(dir.path()).is_empty());

        link(dir.path(), "SingletonSocket", "/tmp/gone/SingletonSocket")?;
        link(dir.path(), "SingletonCookie", "1")?;
        assert_eq!(clear_stale_locks(dir.path(), HOST, |_| true).await?, None);
        assert!(entries(dir.path()).is_empty());
        // Nothing to do is not an error, and nothing else in the profile is touched.
        std::fs::write(dir.path().join("Local State"), b"{}")?;
        assert_eq!(clear_stale_locks(dir.path(), HOST, |_| true).await?, None);
        assert!(dir.path().join("Local State").exists());
        Ok(())
    }

    /// After a failed start: does the lock still name the process it named before?
    #[tokio::test]
    async fn the_lock_is_still_held_only_by_the_same_process_of_this_host() -> TestResult {
        let dir = tempfile::tempdir()?;
        assert!(!still_held(dir.path(), HOST, 4242).await);
        link(dir.path(), "SingletonLock", "new-host-4242")?;
        assert!(still_held(dir.path(), HOST, 4242).await);
        assert!(
            !still_held(dir.path(), HOST, 4243).await,
            "Chrome took it over"
        );
        assert!(!still_held(dir.path(), b"old-host", 4242).await);
        Ok(())
    }

    #[test]
    fn this_process_is_running_and_pid_zero_is_not() -> TestResult {
        assert!(process_running(std::process::id()));
        let mut child = std::process::Command::new("true").spawn()?;
        let reaped = child.id();
        child.wait()?;
        assert!(!process_running(reaped), "a reaped child is not running");
        assert!(!process_running(0));
        assert!(!process_running(u32::MAX));
        assert!(!host_name()?.is_empty());
        Ok(())
    }
}
