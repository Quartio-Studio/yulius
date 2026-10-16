//! Entering a sandbox: what a contained run does before its `bwrap` (SPEC-002 ADR-22 §2).
//!
//! The ptyhost calls [`join`] first, while it is still single-threaded:
//! - it moves into the sandbox's cgroup;
//! - it opens the holder's user, network and mount namespaces, checks that each still has the
//!   recorded inode (a pid the holder no longer owns is refused);
//! - it `setns`es into them, user namespace first.
//!
//! Its PTY child is then `yulius sandbox-exec -- bwrap …` ([`exec_bwrap`]), which hands `bwrap`
//! the seccomp program on a pipe (`--seccomp FD`) and replaces itself with it, so the run's pid
//! stays `bwrap`'s.

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::Path;

use yulius_contracts::ptyhost::SandboxJoin;

use crate::userns::{cloexec_pipe, inheritable};

/// Joins the sandbox described by `join` (see the module docs).
///
/// # Errors
/// The cgroup cannot be joined, a namespace is gone or changed, or `setns` is refused.
pub fn join(join: &SandboxJoin) -> io::Result<()> {
    if let Some(cgroup) = &join.cgroup {
        crate::cgroup::join(Path::new(cgroup))?;
    }
    join_namespaces(join)
}

#[cfg(target_os = "linux")]
fn join_namespaces(join: &SandboxJoin) -> io::Result<()> {
    join_holder(join.holder_pid, join.user_ns, join.net_ns, join.mnt_ns)
}

/// `/proc/<pid>/ns/<kind>` of a holder, refused unless it still has the recorded inode (a pid
/// the holder no longer owns).
///
/// # Errors
/// It is gone or changed.
pub fn holder_ns(pid: u32, kind: &str, expected: u64) -> io::Result<std::fs::File> {
    use std::os::unix::fs::MetadataExt as _;
    let file = std::fs::File::open(format!("/proc/{pid}/ns/{kind}"))?;
    if file.metadata()?.ino() == expected {
        Ok(file)
    } else {
        Err(io::Error::other(format!(
            "the sandbox's {kind} namespace changed (holder restarted)"
        )))
    }
}

/// Joins the user, network and mount namespaces of the holder `pid` (checked against the
/// recorded inodes), user namespace first. The caller must be single-threaded.
///
/// # Errors
/// A namespace is gone or changed, or `setns` is refused.
#[cfg(target_os = "linux")]
pub fn join_holder(pid: u32, user_ns: u64, net_ns: u64, mnt_ns: u64) -> io::Result<()> {
    use nix::sched::{CloneFlags, setns};
    let user = holder_ns(pid, "user", user_ns)?;
    let net = holder_ns(pid, "net", net_ns)?;
    let mnt = holder_ns(pid, "mnt", mnt_ns)?;
    setns(&user, CloneFlags::CLONE_NEWUSER).map_err(io::Error::from)?;
    setns(&net, CloneFlags::CLONE_NEWNET).map_err(io::Error::from)?;
    setns(&mnt, CloneFlags::CLONE_NEWNS).map_err(io::Error::from)?;
    std::env::set_current_dir("/")
}

/// Joins a holder's namespaces (Linux only).
///
/// # Errors
/// Always here.
#[cfg(not(target_os = "linux"))]
pub fn join_holder(_pid: u32, _user_ns: u64, _net_ns: u64, _mnt_ns: u64) -> io::Result<()> {
    Err(io::Error::other("sandbox namespaces are Linux-only"))
}

#[cfg(not(target_os = "linux"))]
fn join_namespaces(_join: &SandboxJoin) -> io::Result<()> {
    Err(io::Error::other("sandbox namespaces are Linux-only"))
}

/// Body of `yulius sandbox-exec -- <bwrap> <arguments>`: execs `bwrap` with the run seccomp
/// filter (where the architecture has one). Returns only on failure.
#[must_use]
pub fn exec_bwrap(argv: &[OsString]) -> io::Error {
    exec_bwrap_with(argv, crate::seccomp::Profile::Run)
}

/// As [`exec_bwrap`] with `profile`'s filter (`sandbox-exec --engine`: the engine's, ADR-0151).
#[must_use]
pub fn exec_bwrap_with(argv: &[OsString], profile: crate::seccomp::Profile) -> io::Error {
    use std::os::unix::process::CommandExt as _;

    let Some((program, rest)) = argv.split_first() else {
        return io::Error::new(io::ErrorKind::InvalidInput, "nothing to run");
    };
    let mut command = std::process::Command::new(program);
    // Kept open until exec replaces this process.
    let mut filter = None;
    if let Some(bytes) = crate::seccomp::program_bytes_for(profile) {
        let pipe = cloexec_pipe().and_then(|(read, write)| {
            let mut writer = std::fs::File::from(write);
            writer.write_all(&bytes)?;
            drop(writer);
            inheritable(&read)?;
            Ok(read)
        });
        match pipe {
            Ok(read) => {
                command.arg("--seccomp").arg(read.as_raw_fd().to_string());
                filter = Some(read);
            }
            Err(error) => return error,
        }
    }
    command.args(rest);
    let error = command.exec();
    drop(filter);
    error
}
