//! Run the pinned mutation tool and its Cargo workers in one POSIX session.
use std::io;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, getsid};

pub(super) fn command(cargo: &str) -> Command {
    // cargo-mutants creates separate process groups for its workers. A session
    // contains all those groups. The launcher execs Cargo, retaining its PID.
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import os,sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])",
        cargo,
    ]);
    command
}

/// Observe live members while the unreaped leader still reserves the session ID.
fn workers(session: Pid) -> io::Result<Vec<Pid>> {
    let list = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,stat="])
        .output()?;
    if !list.status.success() {
        return Err(io::Error::other(
            "could not enumerate mutation worker processes",
        ));
    }
    Ok(String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|row| {
            let mut fields = row.split_whitespace();
            let pid = Pid::from_raw(fields.next()?.parse().ok()?);
            (!fields.next()?.starts_with('Z') && getsid(Some(pid)) == Ok(session)).then_some(pid)
        })
        .collect())
}

fn stop_workers(child: &mut std::process::Child, pid: Pid) -> io::Result<()> {
    let _ = kill(pid, Signal::SIGTERM);
    let grace = Instant::now() + Duration::from_secs(3);
    while Instant::now() < grace && !workers(pid)?.is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Also targets a launcher interrupted before its setsid. Keep the leader
    // unreaped through the last scan so an unrelated session cannot reuse its ID.
    child.kill()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let live = workers(pid)?;
        if live.is_empty() {
            return Ok(());
        }
        for worker in live {
            if let Err(error) = kill(worker, Signal::SIGKILL)
                && error != nix::errno::Errno::ESRCH
            {
                return Err(error.into());
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "mutation workers did not exit after SIGKILL",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Returns None if the clock expires. Cancellation is an error even when the
/// command exits cleanly during shutdown. Reap the child before releasing scratch.
pub(super) fn run(
    command: &mut Command,
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<Option<ExitStatus>> {
    let interrupted = || io::Error::new(io::ErrorKind::Interrupted, "mutation run interrupted");
    if stop.load(Ordering::Relaxed) {
        return Err(interrupted());
    }
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let mut child = command.spawn()?;
    let pid = Pid::from_raw(i32::try_from(child.id()).map_err(io::Error::other)?);
    loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break;
        }
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let shutdown = stop_workers(&mut child, pid);
    if shutdown.is_err() {
        let _ = child.kill();
    }
    child.wait()?;
    shutdown?;
    if stop.load(Ordering::Relaxed) {
        Err(interrupted())
    } else {
        Ok(None)
    }
}
