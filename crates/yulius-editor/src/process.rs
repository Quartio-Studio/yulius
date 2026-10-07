//! Supervised child process (same contract as `yulius-browser`'s; kept local to avoid a shared
//! crate for ~200 lines): own process group, bounded stderr tail, exit watch, escalating
//! termination with reaping (docs/engineering/runtime-contracts.md RUN-001/RUN-002).

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Bytes of stderr kept for diagnostics.
const STDERR_TAIL_BYTES: usize = 2048;

/// Environment variables passed through to the editor process; everything else (including any
/// credentials in the server's environment) is withheld.
const PASSED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TZ",
    "XDG_RUNTIME_DIR",
];

/// Escalation deadlines for stopping a process (SPEC-002 §9 "Stop escalation").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopTimeouts {
    /// Wait for a voluntary exit after a graceful request (e.g. `Browser.close`).
    pub graceful: Duration,
    /// Wait after SIGTERM to the process group.
    pub term: Duration,
    /// Wait after SIGKILL to the process group before reporting the process as not reaped.
    pub kill: Duration,
}

impl Default for StopTimeouts {
    fn default() -> Self {
        Self {
            graceful: Duration::from_secs(5),
            term: Duration::from_secs(5),
            kill: Duration::from_secs(10),
        }
    }
}

/// The child could not be reaped within the SIGKILL deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NotReaped;

pub(crate) struct Supervised {
    pid: u32,
    exit: watch::Receiver<Option<String>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    _monitor: JoinHandle<()>,
    stderr_reader: Option<JoinHandle<()>>,
}

/// The allowlisted subset of the current environment.
pub(crate) fn passed_env() -> Vec<(OsString, OsString)> {
    PASSED_ENV
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect()
}

impl Supervised {
    /// Spawns `program` with `args` in `cwd` in a new process group with a cleared
    /// environment plus `env`. Stdin and stdout are null; stderr feeds a bounded tail.
    pub(crate) fn spawn(
        program: &Path,
        args: &[OsString],
        env: &[(OsString, OsString)],
        cwd: &Path,
    ) -> std::io::Result<Self> {
        let mut command = tokio::process::Command::new(program);
        command
            .current_dir(cwd)
            .args(args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("spawned child has no pid"))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = child.stderr.take().map(|mut pipe| {
            let tail = Arc::clone(&stderr);
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                loop {
                    match pipe.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                            tail.extend_from_slice(&buf[..n]);
                            let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                            tail.drain(..excess);
                        }
                    }
                }
            })
        });
        let (tx, exit) = watch::channel(None);
        let monitor = tokio::spawn(async move {
            let text = match child.wait().await {
                Ok(status) => status.to_string(),
                Err(error) => format!("wait failed: {error}"),
            };
            let _ = tx.send(Some(text));
        });
        Ok(Self {
            pid,
            exit,
            stderr,
            _monitor: monitor,
            stderr_reader,
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Exit status text once the child has been reaped.
    pub(crate) fn exit_status(&self) -> Option<String> {
        self.exit.borrow().clone()
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exit.borrow().is_some()
    }

    /// Waits up to `timeout` for the child to be reaped.
    pub(crate) async fn wait_exit(&self, timeout: Duration) -> Option<String> {
        let mut exit = self.exit.clone();
        match tokio::time::timeout(timeout, exit.wait_for(Option::is_some)).await {
            Ok(Ok(status)) => status.clone(),
            _ => self.exit_status(),
        }
    }

    /// Lossy UTF-8 of the last stderr bytes.
    pub(crate) fn stderr_tail(&self) -> String {
        let tail = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&tail).trim().to_owned()
    }

    fn signal_group(&self, signal: Signal) {
        if let Ok(pid) = i32::try_from(self.pid) {
            // ESRCH (group already gone) is the expected outcome after exit.
            let _ = killpg(Pid::from_raw(pid), signal);
        }
    }

    /// Stops the process group: optional voluntary-exit wait, SIGTERM, SIGKILL; returns the
    /// exit status once reaped. Remaining group members are killed after the leader exits.
    pub(crate) async fn terminate(&self, timeouts: StopTimeouts) -> Result<String, NotReaped> {
        let mut status = self.exit_status();
        if status.is_none() && !timeouts.graceful.is_zero() {
            status = self.wait_exit(timeouts.graceful).await;
        }
        if status.is_none() {
            self.signal_group(Signal::SIGTERM);
            status = self.wait_exit(timeouts.term).await;
        }
        if status.is_none() {
            self.signal_group(Signal::SIGKILL);
            status = self.wait_exit(timeouts.kill).await;
        }
        // Descendants left in the group (the leader was just reaped, so the group id cannot
        // have been reused while members remain).
        self.signal_group(Signal::SIGKILL);
        status.ok_or(NotReaped)
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        if !self.has_exited() {
            self.signal_group(Signal::SIGKILL);
        }
        if let Some(reader) = &self.stderr_reader {
            reader.abort();
        }
        // The monitor task keeps running until the child is reaped; it owns only the child.
    }
}

/// Whether `path` is an existing regular file with an execute bit.
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Resolves a bare program name against `path_var`, or checks an explicit path.
pub(crate) fn resolve_program(
    program: &Path,
    path_var: Option<&OsStr>,
) -> Option<std::path::PathBuf> {
    if program.components().count() > 1 || program.is_absolute() {
        return is_executable(program).then(|| program.to_path_buf());
    }
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}
