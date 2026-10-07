//! Bounded, argv-only execution of the Git executable.
//!
//! Every command: explicit argv (no shell), cleared environment plus an allowlist,
//! `GIT_TERMINAL_PROMPT=0`, fixed `-c` hardening options that never come from input,
//! bounded stdout/stderr capture and a deadline after which the whole process group is
//! killed and reaped.

use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::Git;
use crate::error::GitError;
use crate::redact::redact;

/// Bytes of standard error retained for diagnostics; the rest is drained and discarded.
const STDERR_LIMIT: usize = 64 * 1024;
/// Bytes of standard error copied into [`GitError::Failed`].
const STDERR_IN_ERROR: usize = 4 * 1024;
/// How long to wait for output pipes to close after the process exits.
const PIPE_GRACE: Duration = Duration::from_secs(2);
/// Upper bound for the exit polling interval.
const MAX_POLL: Duration = Duration::from_millis(20);

/// Variables inherited from the parent environment; everything else is cleared.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SYSTEMROOT",
    "XDG_CONFIG_HOME",
];

/// Hardening configuration applied to every command. Constant; never derived from input.
/// `protocol.ext.allow=never` blocks `ext::` transports that run arbitrary commands and
/// `core.fsmonitor=false` stops repository configuration from launching a monitor daemon.
const FIXED_CONFIG: &[&str] = &[
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.fsmonitor=false",
];

/// Captured result of a finished command.
pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) status: ExitStatus,
}

impl Output {
    pub(crate) fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub(crate) fn stdout_trimmed(&self) -> String {
        self.stdout_text().trim_end_matches(['\n', '\r']).to_owned()
    }
}

/// One command invocation.
pub(crate) struct Invocation<'a> {
    pub(crate) label: &'static str,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: Vec<(&'static str, String)>,
    pub(crate) timeout: Duration,
}

impl<'a> Invocation<'a> {
    pub(crate) fn new(git: &Git, label: &'static str, cwd: Option<&'a Path>) -> Self {
        Self {
            label,
            cwd,
            args: Vec::new(),
            env: Vec::new(),
            timeout: git.timeout,
        }
    }

    pub(crate) fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub(crate) fn env(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.env.push((key, value.into()));
        self
    }

    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

enum Stream {
    Stdout(std::io::Result<Vec<u8>>),
    Stderr(std::io::Result<Vec<u8>>),
}

impl Git {
    /// Runs and requires exit status 0.
    pub(crate) fn run_ok(&self, invocation: Invocation<'_>) -> Result<Output, GitError> {
        let label = invocation.label;
        let output = self.run(invocation)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(failure(label, &output))
        }
    }

    /// Runs and returns the output whatever the exit status.
    pub(crate) fn run(&self, invocation: Invocation<'_>) -> Result<Output, GitError> {
        let mut command = Command::new(&self.exe);
        command
            .args(FIXED_CONFIG)
            .args(&invocation.args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in INHERITED_ENV {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C");
        for (key, value) in &invocation.env {
            command.env(key, value);
        }
        if let Some(cwd) = invocation.cwd {
            command.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(GitError::Spawn)?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            kill_tree(&mut child);
            return Err(GitError::Io {
                context: "capturing git output",
                source: std::io::Error::other("missing pipe"),
            });
        };
        let overflow = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        spawn_readers(stdout, stderr, self.max_output_bytes, &overflow, &tx);
        drop(tx);

        let deadline = Instant::now() + invocation.timeout;
        let mut poll = Duration::from_millis(1);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(source) => {
                    kill_tree(&mut child);
                    return Err(GitError::Io {
                        context: "waiting for git",
                        source,
                    });
                }
            }
            if overflow.load(Ordering::SeqCst) {
                kill_tree(&mut child);
                return Err(GitError::OutputTooLarge {
                    command: invocation.label,
                    limit: self.max_output_bytes,
                });
            }
            let now = Instant::now();
            if now >= deadline {
                kill_tree(&mut child);
                return Err(GitError::Timeout {
                    command: invocation.label,
                    after: invocation.timeout,
                });
            }
            thread::sleep(poll.min(deadline - now));
            poll = (poll * 2).min(MAX_POLL);
        };

        let mut stdout = None;
        let mut stderr = None;
        let grace_deadline = Instant::now() + PIPE_GRACE;
        while stdout.is_none() || stderr.is_none() {
            let wait = grace_deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(Stream::Stdout(result)) => stdout = Some(result),
                Ok(Stream::Stderr(result)) => stderr = Some(result),
                Err(_) => {
                    // A descendant escaped and holds a pipe open: kill the group again.
                    #[cfg(unix)]
                    kill_group(child.id());
                    return Err(GitError::Io {
                        context: "collecting git output",
                        source: std::io::Error::new(
                            ErrorKind::TimedOut,
                            "output pipe stayed open after git exited",
                        ),
                    });
                }
            }
        }
        if overflow.load(Ordering::SeqCst) {
            return Err(GitError::OutputTooLarge {
                command: invocation.label,
                limit: self.max_output_bytes,
            });
        }
        let io_error = |source| GitError::Io {
            context: "reading git output",
            source,
        };
        let stdout = stdout.unwrap_or_else(|| Ok(Vec::new())).map_err(io_error)?;
        let stderr = stderr.unwrap_or_else(|| Ok(Vec::new())).map_err(io_error)?;
        Ok(Output {
            stdout,
            stderr,
            status,
        })
    }
}

fn spawn_readers(
    stdout: ChildStdout,
    stderr: ChildStderr,
    limit: usize,
    overflow: &Arc<AtomicBool>,
    tx: &mpsc::Sender<Stream>,
) {
    let out_tx = tx.clone();
    let out_overflow = Arc::clone(overflow);
    thread::spawn(move || {
        let result = read_bounded(stdout, limit, &out_overflow);
        let _ = out_tx.send(Stream::Stdout(result));
    });
    let err_tx = tx.clone();
    thread::spawn(move || {
        let result = read_capped(stderr, STDERR_LIMIT);
        let _ = err_tx.send(Stream::Stderr(result));
    });
}

/// Reads until EOF; on exceeding `limit` flags overflow and stops (closing the pipe).
fn read_bounded(
    mut pipe: impl Read,
    limit: usize,
    overflow: &AtomicBool,
) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(buffer);
        }
        if buffer.len().saturating_add(read) > limit {
            overflow.store(true, Ordering::SeqCst);
            return Ok(buffer);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// Keeps the first `cap` bytes and drains the rest so the child never blocks on stderr.
fn read_capped(mut pipe: impl Read, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(buffer);
        }
        let room = cap.saturating_sub(buffer.len());
        buffer.extend_from_slice(&chunk[..read.min(room)]);
    }
}

/// Kills the child's process group (Unix) and the child itself, then reaps it.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    if let Ok(raw) = i32::try_from(pid) {
        let _ = killpg(Pid::from_raw(raw), Signal::SIGKILL);
    }
}

/// Builds a [`GitError::Failed`] with redacted, truncated stderr.
pub(crate) fn failure(label: &'static str, output: &Output) -> GitError {
    let raw = String::from_utf8_lossy(&output.stderr);
    let mut text = redact(raw.trim());
    if text.len() > STDERR_IN_ERROR {
        let mut end = STDERR_IN_ERROR;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    GitError::Failed {
        command: label,
        code: output.status.code(),
        stderr: text,
    }
}
