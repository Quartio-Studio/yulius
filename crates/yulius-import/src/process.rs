//! Bounded `git` subprocess execution: argv only, capped output, deadline with kill and reap.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Captured stderr is truncated to this many bytes.
const MAX_STDERR_BYTES: usize = 16 * 1024;
/// Stderr text included in errors is truncated to this many characters.
const MAX_STDERR_CHARS: usize = 2000;

/// Default stdout bound for small queries.
pub(crate) const SMALL_OUTPUT: usize = 1024 * 1024;
/// Stdout bound for path listings.
pub(crate) const LIST_OUTPUT: usize = 64 * 1024 * 1024;
/// Deadline for queries.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
/// Deadline for bundle creation.
pub(crate) const BUNDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A failed `git` invocation. Messages name only the subcommand, never full arguments.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// No `git` executable on `PATH`.
    #[error("git executable not found; install Git and retry")]
    NotInstalled,
    /// The process could not be started or waited for.
    #[error("failed to run git {command}: {source}")]
    Spawn {
        /// Git subcommand.
        command: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// Git exited unsuccessfully.
    #[error("git {command} failed ({status}): {stderr}")]
    Failed {
        /// Git subcommand.
        command: String,
        /// Exit status description.
        status: String,
        /// Truncated standard error text.
        stderr: String,
    },
    /// Git wrote more standard output than allowed.
    #[error("git {command} produced more than {limit} bytes of output")]
    OutputTooLarge {
        /// Git subcommand.
        command: String,
        /// The enforced bound.
        limit: usize,
    },
    /// Git did not finish in time; it was killed and reaped.
    #[error("git {command} did not finish within {} s", timeout.as_secs())]
    TimedOut {
        /// Git subcommand.
        command: String,
        /// The enforced deadline.
        timeout: Duration,
    },
    /// A file Git produced could not be read.
    #[error("failed to read the output of git {command}: {source}")]
    ReadOutput {
        /// Git subcommand.
        command: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// Git output could not be interpreted.
    #[error("git {command} produced unexpected output: {detail}")]
    Unexpected {
        /// Git subcommand.
        command: String,
        /// What was wrong.
        detail: String,
    },
}

impl GitError {
    /// Standard error text of a failed invocation, if any.
    pub(crate) fn stderr(&self) -> Option<&str> {
        match self {
            Self::Failed { stderr, .. } => Some(stderr),
            _ => None,
        }
    }
}

/// Output of a successful invocation.
pub(crate) struct GitOutput {
    pub(crate) stdout: Vec<u8>,
}

/// Output of an invocation that may exit non-zero by design (e.g. `git config --get`).
pub(crate) struct GitRun {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

/// Runs `git -C <dir> <args…>` and requires success.
pub(crate) fn git(
    dir: &Path,
    args: &[&OsStr],
    max_stdout: usize,
    timeout: Duration,
) -> Result<GitOutput, GitError> {
    let run = git_status(dir, args, max_stdout, timeout)?;
    if run.status.success() {
        Ok(GitOutput { stdout: run.stdout })
    } else {
        Err(GitError::Failed {
            command: subcommand(args),
            status: run.status.to_string(),
            stderr: run.stderr,
        })
    }
}

/// Runs `git -C <dir> <args…>` and returns the exit status with bounded output.
pub(crate) fn git_status(
    dir: &Path,
    args: &[&OsStr],
    max_stdout: usize,
    timeout: Duration,
) -> Result<GitRun, GitError> {
    let command = subcommand(args);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                GitError::NotInstalled
            } else {
                GitError::Spawn {
                    command: command.clone(),
                    source,
                }
            }
        })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || read_capped(stdout, max_stdout));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_STDERR_BYTES));
    let waited = wait_with_deadline(&mut child, timeout);
    // The readers finish once the child (and any grandchild holding the pipes) exits.
    let stdout = stdout_reader.join();
    let stderr = stderr_reader.join();
    let status = match waited {
        Ok(Some(status)) => status,
        Ok(None) => {
            return Err(GitError::TimedOut { command, timeout });
        }
        Err(source) => return Err(GitError::Spawn { command, source }),
    };
    let (stdout, overflowed) = match stdout {
        Ok(Ok(captured)) => captured,
        Ok(Err(source)) => return Err(GitError::Spawn { command, source }),
        Err(_) => {
            return Err(GitError::Unexpected {
                command,
                detail: "stdout reader panicked".to_owned(),
            });
        }
    };
    if overflowed {
        return Err(GitError::OutputTooLarge {
            command,
            limit: max_stdout,
        });
    }
    let stderr = match stderr {
        Ok(Ok((bytes, _))) => truncate_text(&String::from_utf8_lossy(&bytes)),
        _ => String::new(),
    };
    Ok(GitRun {
        status,
        stdout,
        stderr,
    })
}

/// Waits for the child until `timeout`; on expiry kills and reaps it and returns `None`.
fn wait_with_deadline(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            // Kill can fail only if the child already exited; reaping below settles it.
            let _ = child.kill();
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// Reads up to `limit` bytes, then drains the rest so the writer never blocks.
fn read_capped<R: Read>(source: Option<R>, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let Some(mut source) = source else {
        return Ok((Vec::new(), false));
    };
    let mut captured = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    (&mut source).take(cap).read_to_end(&mut captured)?;
    let overflowed = captured.len() > limit;
    if overflowed {
        captured.truncate(limit);
        io::copy(&mut source, &mut io::sink())?;
    }
    Ok((captured, overflowed))
}

fn subcommand(args: &[&OsStr]) -> String {
    // Skip global `-c key=value` options to name the actual subcommand.
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if *arg == "-c" {
            iter.next();
            continue;
        }
        return arg.to_string_lossy().into_owned();
    }
    String::new()
}

fn truncate_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_STDERR_CHARS {
        trimmed.to_owned()
    } else {
        let mut short: String = trimmed.chars().take(MAX_STDERR_CHARS).collect();
        short.push('…');
        short
    }
}

/// Converts raw Git path output to a path without lossy conversion on Unix.
pub(crate) fn bytes_to_path(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Strips one trailing newline from command output.
pub(crate) fn trim_newline(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_bounds_memory_and_reports_overflow() -> io::Result<()> {
        let data = [7_u8; 100];
        let (captured, overflowed) = read_capped(Some(&data[..]), 10)?;
        assert_eq!(captured.len(), 10);
        assert!(overflowed);
        let (captured, overflowed) = read_capped(Some(&data[..]), 100)?;
        assert_eq!(captured.len(), 100);
        assert!(!overflowed);
        Ok(())
    }

    #[test]
    fn subcommand_skips_config_overrides() {
        let args: [&OsStr; 4] = [
            OsStr::new("-c"),
            OsStr::new("pack.threads=1"),
            OsStr::new("bundle"),
            OsStr::new("create"),
        ];
        assert_eq!(subcommand(&args), "bundle");
    }

    #[test]
    fn missing_directory_is_a_git_failure_not_a_panic() {
        let result = git(
            Path::new("/nonexistent/yulius-import-test"),
            &[OsStr::new("status")],
            SMALL_OUTPUT,
            QUERY_TIMEOUT,
        );
        assert!(matches!(
            result,
            Err(GitError::Failed { .. } | GitError::NotInstalled)
        ));
    }
}
