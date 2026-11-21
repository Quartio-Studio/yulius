//! Supervise the pinned mutation tool and wait for its build workers.
#[cfg(any(not(target_os = "linux"), test))]
use std::io::Seek;
use std::io::{self, Read};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
#[cfg(any(not(target_os = "linux"), test))]
use nix::unistd::getsid;

pub(super) fn command(cargo: &str) -> Command {
    // cargo-mutants creates separate process groups for its workers. A session
    // contains all those groups. The launcher execs Cargo, retaining its PID.
    let mut command = Command::new("python3");
    #[cfg(target_os = "linux")]
    command.args([
        "-c",
        include_str!("../../../../scripts/build_subreaper.py"),
        cargo,
    ]);
    #[cfg(not(target_os = "linux"))]
    command.args([
        "-c",
        "import os,sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])",
        cargo,
    ]);
    command
}

/// Observe live members while the unreaped leader still reserves the session ID.
#[cfg(any(not(target_os = "linux"), test))]
fn workers(session: Pid) -> io::Result<Vec<Pid>> {
    let mut ps = Command::new("/bin/ps");
    ps.arg("-A");
    #[cfg(target_os = "linux")]
    ps.arg("-L"); // A zombie thread-group leader can still have running threads.
    let list = process_listing(ps.args(["-o", "pid=,stat="]), Duration::from_secs(1))?;
    let mut live: Vec<_> = list
        .lines()
        .filter_map(|row| {
            let mut fields = row.split_whitespace();
            let pid = Pid::from_raw(fields.next()?.parse().ok()?);
            (!fields.next()?.starts_with('Z') && getsid(Some(pid)) == Ok(session)).then_some(pid)
        })
        .collect();
    live.sort_unstable();
    live.dedup();
    Ok(live)
}

fn wait_within(child: &mut std::process::Child, limit: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process did not exit",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(not(target_os = "linux"), test))]
fn process_listing(command: &mut Command, limit: Duration) -> io::Result<String> {
    // A file lets ps exit even when its snapshot exceeds a pipe buffer. A
    // stalled or oversized snapshot cannot establish that the workers stopped.
    let mut output = tempfile::tempfile()?;
    let mut child = command.stdout(output.try_clone()?).spawn()?;
    match wait_within(&mut child, limit) {
        Ok(status) if status.success() => {}
        Ok(_) => return Err(io::Error::other("could not enumerate build workers")),
        Err(error) => {
            child.kill()?;
            wait_within(&mut child, Duration::from_secs(1))?;
            return Err(error);
        }
    }
    const MAX_BYTES: u64 = 16 * 1024 * 1024;
    if output.metadata()?.len() > MAX_BYTES {
        return Err(io::Error::other("process listing exceeded 16 MiB"));
    }
    output.rewind()?;
    let mut listing = String::new();
    output.read_to_string(&mut listing)?;
    Ok(listing)
}

#[cfg(not(target_os = "linux"))]
fn stop_workers(child: &mut std::process::Child, pid: Pid, graceful: bool) -> io::Result<()> {
    if graceful {
        let _ = kill(pid, Signal::SIGTERM);
        let grace = Instant::now() + Duration::from_secs(3);
        while Instant::now() < grace && workers(pid).is_ok_and(|live| !live.is_empty()) {
            std::thread::sleep(Duration::from_millis(100));
        }
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

#[derive(Debug)]
pub(super) enum RunError {
    Command(io::Error),
    Unreaped(io::Error),
}

impl From<io::Error> for RunError {
    fn from(error: io::Error) -> Self {
        Self::Command(error)
    }
}

impl RunError {
    pub(super) fn into_io(self) -> io::Error {
        match self {
            Self::Command(e) | Self::Unreaped(e) => e,
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run(
    command: &mut Command,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<Option<ExitStatus>, RunError> {
    use std::os::unix::process::ExitStatusExt;
    let interrupted = || io::Error::new(io::ErrorKind::Interrupted, "mutation run interrupted");
    if stop.load(Ordering::Relaxed) {
        return Err(interrupted().into());
    }
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let control = std::env::var_os("YULIUS_BUILD_SCRATCH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut receipt = tempfile::NamedTempFile::new_in(&control)?;
    let mut cancellation = tempfile::NamedTempFile::new_in(&control)?;
    let mut monitor = command
        .env("YULIUS_BUILD_STATUS", receipt.path())
        .env("YULIUS_BUILD_CANCEL", cancellation.path())
        .spawn()?;
    let (status, timed_out) = loop {
        if let Some(status) = monitor.try_wait().map_err(RunError::Unreaped)? {
            break (status, false);
        }
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            // Cancellation before Python initializes must not kill the monitor
            // before it has a chance to acknowledge that no build was started.
            if std::io::Write::write_all(cancellation.as_file_mut(), b"1").is_err() {
                // The disk quota can fill during a build. The monitor is still
                // our waitable child and handles TERM after initialization.
                if let Ok(pid) = i32::try_from(monitor.id()) {
                    let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
                }
            }
            match wait_within(&mut monitor, Duration::from_secs(8)) {
                Ok(status) => break (status, true),
                Err(error) => {
                    let _ = monitor.kill();
                    wait_within(&mut monitor, Duration::from_secs(2))
                        .map_err(RunError::Unreaped)?;
                    return Err(RunError::Unreaped(error));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let unconfirmed = || {
        RunError::Unreaped(io::Error::other(
            "build monitor exited without confirming worker shutdown",
        ))
    };
    if !status.success() {
        return Err(unconfirmed());
    }
    let mut result = String::new();
    receipt
        .as_file_mut()
        .take(8)
        .read_to_string(&mut result)
        .map_err(RunError::Unreaped)?;
    let code = result
        .parse::<i32>()
        .ok()
        .filter(|c| (-127..=255).contains(c))
        .ok_or_else(unconfirmed)?;
    if stop.load(Ordering::Relaxed) {
        Err(interrupted().into())
    } else if timed_out {
        Ok(None)
    } else {
        Ok(Some(ExitStatus::from_raw(if code < 0 {
            -code
        } else {
            code << 8
        })))
    }
}

/// Returns None if the clock expires. Cancellation is an error even when the
/// command exits cleanly during shutdown. Reap the child before releasing scratch.
#[cfg(not(target_os = "linux"))]
pub(super) fn run(
    command: &mut Command,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<Option<ExitStatus>, RunError> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    let interrupted = || io::Error::new(io::ErrorKind::Interrupted, "mutation run interrupted");
    if stop.load(Ordering::Relaxed) {
        return Err(interrupted().into());
    }
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let mut child = command.spawn()?;
    let leader = rustix::process::Pid::from_child(&child);
    let pid = Pid::from_raw(leader.as_raw_nonzero().get());
    let observe = WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG;
    let finished = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break Ok(false);
        }
        match waitid(WaitId::Pid(leader), observe) {
            Ok(Some(_)) => break Ok(true),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => break Err(io::Error::from(error)),
        }
    };
    // A tool can exit while its workers still hold build files. Reserve its
    // session ID until the last worker check, including on ordinary completion.
    let shutdown = stop_workers(&mut child, pid, !matches!(finished, Ok(true)));
    if shutdown.is_err() {
        let _ = child.kill();
    }
    let status = wait_within(&mut child, Duration::from_secs(3)).map_err(RunError::Unreaped)?;
    shutdown.map_err(RunError::Unreaped)?;
    if stop.load(Ordering::Relaxed) {
        Err(interrupted().into())
    } else {
        Ok(finished?.then_some(status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stalled_process_listing_returns_an_observation_error() -> io::Result<()> {
        let result = process_listing(
            Command::new("/bin/sleep").arg("1"),
            Duration::from_millis(50),
        );
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn command_shutdown_observes_live_threads_after_their_leader_exits() -> io::Result<()> {
        use std::os::unix::net::UnixListener;
        let temp = tempfile::tempdir()?;
        let address = temp.path().join("worker.sock");
        let listener = UnixListener::bind(&address)?;
        let pidfile = temp.path().join("worker.pid");
        let mut child = command("python3");
        child
            .args([
                "-c",
                r"
import ctypes, os, socket, sys, threading, time
from pathlib import Path
worker = os.fork()
if worker == 0:
    def hold():
        with socket.socket(socket.AF_UNIX) as conn:
            conn.connect(sys.argv[1])
            conn.recv(1)
    threading.Thread(target=hold).start()
    ctypes.CDLL(None).pthread_exit(None)
Path(sys.argv[2]).write_text(str(worker))
deadline = time.monotonic() + 5
while Path(f'/proc/{worker}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z':
    if time.monotonic() >= deadline: sys.exit(10)
    time.sleep(.01)
",
            ])
            .arg(&address)
            .arg(&pidfile);
        let status = run(
            &mut child,
            Instant::now() + Duration::from_secs(8),
            &AtomicBool::new(false),
        );
        listener.set_nonblocking(true)?;
        let (mut conn, _) = listener.accept()?;
        conn.set_nonblocking(true)?;
        let was_closed = matches!(conn.read(&mut [0]), Ok(0));
        if !was_closed {
            let worker = std::fs::read_to_string(pidfile)?
                .parse()
                .map_err(io::Error::other)?;
            let _ = kill(Pid::from_raw(worker), Signal::SIGKILL);
        }
        assert!(was_closed, "worker thread survived its zombie leader");
        assert_eq!(
            status.map_err(RunError::into_io)?.and_then(|s| s.code()),
            Some(0)
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn command_waits_for_workers_that_start_another_session() -> io::Result<()> {
        check_detached_worker(false)?;
        check_detached_worker(true)
    }

    #[cfg(target_os = "linux")]
    fn check_detached_worker(cancel: bool) -> io::Result<()> {
        use std::os::unix::net::UnixListener;
        let temp = tempfile::tempdir()?;
        let address = temp.path().join("worker.sock");
        let listener = UnixListener::bind(&address)?;
        let pid_file = temp.path().join("worker.pid");
        let mut child = command("python3");
        child
            .args([
                "-c",
                r"
import os, signal, socket, sys
from pathlib import Path
reader, writer = os.pipe()
pid = os.fork()
if pid == 0:
    os.close(reader)
    os.setsid()
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(sys.argv[1])
        os.write(writer, b'1')
        conn.recv(1)
    os._exit(0)
os.close(writer)
os.read(reader, 1)
Path(sys.argv[2]).write_text(str(pid))
if sys.argv[3] == 'cancel': signal.pause()
sys.exit(7)
",
            ])
            .arg(&address)
            .arg(&pid_file)
            .arg(if cancel { "cancel" } else { "finish" });
        let stop = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(8);
        let result = std::thread::scope(|threads| {
            if cancel {
                threads.spawn(|| {
                    while !pid_file.exists() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    stop.store(true, Ordering::Relaxed);
                });
            }
            run(&mut child, deadline, &stop)
        });
        listener.set_nonblocking(true)?;
        let (mut connection, _) = listener.accept()?;
        connection.set_nonblocking(true)?;
        let closed = matches!(connection.read(&mut [0]), Ok(0));
        if !closed {
            let pid = std::fs::read_to_string(pid_file)?
                .parse()
                .map_err(io::Error::other)?;
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
        assert!(
            closed,
            "a worker in its own session survived command completion"
        );
        if cancel {
            assert!(
                matches!(result, Err(RunError::Command(error)) if error.kind() == io::ErrorKind::Interrupted)
            );
        } else {
            assert_eq!(
                result.map_err(RunError::into_io)?.and_then(|s| s.code()),
                Some(7)
            );
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_monitor_cannot_acknowledge_build_cleanup() {
        // Exit 0 alone is not proof that a monitor waited for the last worker.
        let result = run(
            &mut Command::new("true"),
            Instant::now() + Duration::from_secs(5),
            &AtomicBool::new(false),
        );
        assert!(matches!(result, Err(RunError::Unreaped(_))));
    }

    #[test]
    fn command_leader_exit_stops_workers_before_returning_its_status() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let file = temp.path().join("session");
        let mut child = command("python3");
        child
            .args([
                "-c",
                r"
import os, signal, sys
from pathlib import Path
reader, writer = os.pipe()
if os.fork() == 0:
    os.close(reader)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    os.write(writer, b'1')
    signal.pause()
    os._exit(0)
os.close(writer)
os.read(reader, 1)
Path(sys.argv[1]).write_text(str(os.getsid(0)))
sys.exit(7)
",
            ])
            .arg(&file);
        let result = run(
            &mut child,
            Instant::now() + Duration::from_secs(10),
            &AtomicBool::new(false),
        );
        let session = Pid::from_raw(
            std::fs::read_to_string(file)?
                .parse()
                .map_err(io::Error::other)?,
        );
        let remaining = workers(session)?;
        for pid in &remaining {
            let _ = kill(*pid, Signal::SIGKILL);
        }
        assert!(remaining.is_empty(), "worker survived leader completion");
        assert_eq!(
            result.map_err(RunError::into_io)?.and_then(|s| s.code()),
            Some(7)
        );
        Ok(())
    }
}
