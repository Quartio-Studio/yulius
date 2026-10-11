//! Starting `bwrap` in a new user namespace whose id map covers a subordinate range
//! (SPEC-002 ADR-19 §1).
//!
//! An unprivileged `bwrap` can only map the caller's own id. With `--userns-block-fd` it leaves
//! the map to us: it reports the namespace's process on `--info-fd`, waits, and we write the
//! map with the setuid helpers `newuidmap`/`newgidmap` (which check `/etc/subuid` and
//! `/etc/subgid`) before releasing it.
//!
//! Call these from a single-purpose, short-lived process: the descriptors handed to `bwrap`
//! lose `FD_CLOEXEC` for the duration of the spawn, so a concurrent spawn elsewhere in the
//! process could inherit them.

use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use yulius_contracts::sandbox::IdMap;

use crate::subid::map_args;

/// Time `bwrap` has to report its namespace process.
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest `--info-fd` report read.
const MAX_INFO_BYTES: u64 = 4096;

/// The programs and id maps of a mapped namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapper {
    /// `bwrap`.
    pub bwrap: PathBuf,
    /// `newuidmap`.
    pub newuidmap: PathBuf,
    /// `newgidmap`.
    pub newgidmap: PathBuf,
    /// User ids.
    pub uid: IdMap,
    /// Group ids.
    pub gid: IdMap,
}

/// The `child-pid` of a `bwrap --info-fd` report.
#[must_use]
pub fn info_child_pid(text: &str) -> Option<u32> {
    let rest = text.split("\"child-pid\"").nth(1)?;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|pid| *pid > 1)
}

#[cfg(target_os = "linux")]
fn keep_across_exec(fd: &OwnedFd) -> io::Result<()> {
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty()))
        .map(drop)
        .map_err(io::Error::from)
}

#[cfg(target_os = "linux")]
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).map_err(io::Error::from)
}

/// Clears `FD_CLOEXEC` on `fd` so a spawned child keeps it (see the module docs).
///
/// # Errors
/// The flag cannot be changed, or the platform is not Linux.
pub fn inheritable(fd: &OwnedFd) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        keep_across_exec(fd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        Err(io::Error::other("namespaces are Linux-only"))
    }
}

/// A pipe whose ends close on exec (Linux).
///
/// # Errors
/// The pipe cannot be created, or the platform is not Linux.
pub fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    {
        pipe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::other("namespaces are Linux-only"))
    }
}

fn read_info(reader: OwnedFd) -> io::Result<u32> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(reader);
        let mut text = Vec::new();
        let mut byte = [0u8; 1];
        // Read until the closing brace: other processes may keep the write end open.
        while (text.len() as u64) < MAX_INFO_BYTES {
            match file.read(&mut byte) {
                Ok(1) => {
                    text.push(byte[0]);
                    if byte[0] == b'}' {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = sender.send(String::from_utf8_lossy(&text).into_owned());
    });
    let text = receiver
        .recv_timeout(INFO_TIMEOUT)
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "bwrap did not report"))?;
    info_child_pid(&text).ok_or_else(|| {
        io::Error::other(format!(
            "bwrap did not start its namespace ({})",
            text.trim()
        ))
    })
}

fn write_map(program: &PathBuf, pid: u32, map: &IdMap) -> io::Result<()> {
    let output = Command::new(program)
        .arg(pid.to_string())
        .args(map_args(map))
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

impl Mapper {
    /// Spawns `bwrap --unshare-user --uid 0 --gid 0 <args>` in its own process group with the id
    /// map written, and returns it with the pid of its namespace process. `keep` are
    /// descriptors (already [`inheritable`]) the namespace process must receive.
    ///
    /// # Errors
    /// `bwrap` cannot start, does not report, or a map cannot be written; the child is killed.
    pub fn spawn(&self, args: &[OsString], stderr: Stdio) -> io::Result<(Child, u32)> {
        let (info_read, info_write) = cloexec_pipe()?;
        let (block_read, block_write) = cloexec_pipe()?;
        inheritable(&info_write)?;
        inheritable(&block_read)?;
        let mut command = Command::new(&self.bwrap);
        command
            .arg("--unshare-user")
            .arg("--info-fd")
            .arg(info_write.as_raw_fd().to_string())
            .arg("--userns-block-fd")
            .arg(block_read.as_raw_fd().to_string())
            .args(["--uid", "0", "--gid", "0"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr);
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        drop(info_write);
        drop(block_read);
        let result = (|| {
            let pid = read_info(info_read)?;
            write_map(&self.newuidmap, pid, &self.uid)?;
            write_map(&self.newgidmap, pid, &self.gid)?;
            let mut release = std::fs::File::from(block_write);
            release.write_all(b"1")?;
            Ok(pid)
        })();
        match result {
            Ok(pid) => Ok((child, pid)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    /// Runs `bwrap … <args>` to completion (bounded by `limit`) and returns its status as text.
    ///
    /// # Errors
    /// Start failure, a timeout (the child is killed) or a non-zero exit with its stderr.
    pub fn run(&self, args: &[OsString], limit: Duration) -> io::Result<()> {
        let log = tempfile_like()?;
        let (mut child, _) = self.spawn(args, Stdio::from(log.try_clone()?))?;
        let deadline = std::time::Instant::now() + limit;
        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                let mut text = String::new();
                let mut file = log;
                let _ = std::io::Seek::rewind(&mut file);
                let _ = (&mut file).take(4096).read_to_string(&mut text);
                return Err(io::Error::other(format!(
                    "namespace command failed ({status}): {}",
                    text.trim()
                )));
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "namespace command timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// An anonymous file for a child's stderr.
fn tempfile_like() -> io::Result<std::fs::File> {
    let dir = std::env::temp_dir();
    for attempt in 0..16u32 {
        let path = dir.join(format!(".yulius-userns-{}-{attempt}", std::process::id()));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("no temporary file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_info_report_names_the_namespace_process() {
        let report = "{\n    \"child-pid\": 1706698,\n    \"mnt-namespace\": 4026533354\n}";
        assert_eq!(info_child_pid(report), Some(1_706_698));
        assert_eq!(info_child_pid("{\"child-pid\":12}"), Some(12));
        assert_eq!(info_child_pid("{\"child-pid\": 1}"), None);
        assert_eq!(
            info_child_pid("bwrap: setting up uid map: Permission denied"),
            None
        );
    }
}
