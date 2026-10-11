//! Entering a sandbox: what a contained run does before its `bwrap` (SPEC-002 ADR-21 §2).
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
    use std::os::unix::fs::MetadataExt as _;

    use nix::sched::{CloneFlags, setns};

    let open = |kind: &str, expected: u64| -> io::Result<std::fs::File> {
        let file = std::fs::File::open(format!("/proc/{}/ns/{kind}", join.holder_pid))?;
        if file.metadata()?.ino() == expected {
            Ok(file)
        } else {
            Err(io::Error::other(format!(
                "the sandbox's {kind} namespace changed (holder restarted)"
            )))
        }
    };
    let user = open("user", join.user_ns)?;
    let net = open("net", join.net_ns)?;
    let mnt = open("mnt", join.mnt_ns)?;
    setns(&user, CloneFlags::CLONE_NEWUSER).map_err(io::Error::from)?;
    setns(&net, CloneFlags::CLONE_NEWNET).map_err(io::Error::from)?;
    setns(&mnt, CloneFlags::CLONE_NEWNS).map_err(io::Error::from)?;
    std::env::set_current_dir("/")
}

#[cfg(not(target_os = "linux"))]
fn join_namespaces(_join: &SandboxJoin) -> io::Result<()> {
    Err(io::Error::other("sandbox namespaces are Linux-only"))
}

/// Body of `yulius sandbox-exec -- <bwrap> <arguments>`: execs `bwrap` with the seccomp filter
/// (where the architecture has one). Returns only on failure.
#[must_use]
pub fn exec_bwrap(argv: &[OsString]) -> io::Error {
    use std::os::unix::process::CommandExt as _;

    let Some((program, rest)) = argv.split_first() else {
        return io::Error::new(io::ErrorKind::InvalidInput, "nothing to run");
    };
    let mut command = std::process::Command::new(program);
    // Kept open until exec replaces this process.
    let mut filter = None;
    if let Some(bytes) = crate::seccomp::program_bytes() {
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
