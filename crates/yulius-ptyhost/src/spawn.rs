//! Starting a ptyhost from the server (SPEC-002 §7.1 "Launch", §9 spawn timeout).

use std::ffi::OsString;
use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal;
use tokio::net::UnixStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use yulius_contracts::ptyhost::{
    EXIT_FILE, ExitRecord, LAUNCH_FILE, LaunchSpec, LaunchSpecError, SOCKET_FILE, STATUS_FILE,
    StatusRecord, valid_env_name,
};

use crate::files::{atomic_write, read_exit, read_status};
use crate::process::{self, signal_group};
use crate::reconcile::terminate_recorded;

/// Default bound on host start-up (SPEC-002 §9).
pub const DEFAULT_SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const KILL_WAIT: Duration = Duration::from_secs(2);

/// Exit status of the ptyhost process itself (not of the agent; see `exit.json` for that).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostProcessExit {
    /// Exit code when the host exited normally.
    pub code: Option<i32>,
    /// Signal number when the host was killed.
    pub signal: Option<i32>,
}

/// A started ptyhost. A background task waits on the host process so it never lingers as a
/// zombie while the server runs; dropping this value keeps that task (and the host) running.
#[derive(Debug)]
pub struct SpawnedHost {
    status: StatusRecord,
    run_dir: PathBuf,
    host_pid: u32,
    exit: watch::Receiver<Option<HostProcessExit>>,
    _waiter: JoinHandle<()>,
}

impl SpawnedHost {
    /// The host's status record (pids, start instant, boot id).
    #[must_use]
    pub fn status(&self) -> &StatusRecord {
        &self.status
    }

    /// The run directory.
    #[must_use]
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// Process id of the host (leader of its own process group).
    #[must_use]
    pub fn host_pid(&self) -> u32 {
        self.host_pid
    }

    /// The host process's exit status, if it has exited.
    #[must_use]
    pub fn host_exit(&self) -> Option<HostProcessExit> {
        *self.exit.borrow()
    }

    /// Waits until the host process has exited and been reaped.
    pub async fn wait(&mut self) -> HostProcessExit {
        wait_exit(&mut self.exit).await
    }
}

fn describe(exit: Option<&ExitRecord>) -> String {
    match exit {
        Some(record) => format!(
            ": {:?}{}",
            record.reason,
            record
                .detail
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        ),
        None => String::new(),
    }
}

/// Failure to start a ptyhost. Messages never contain secret values.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The launch record violates the contract.
    #[error(transparent)]
    InvalidLaunch(#[from] LaunchSpecError),
    /// A secret does not match `secret_env_names` or is malformed.
    #[error("secret variable {name}: {reason}")]
    InvalidSecret {
        /// Variable name (never the value).
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// The run directory already holds a launch, status, exit record or socket.
    #[error("run directory already holds a run")]
    RunDirInUse,
    /// A filesystem or process step failed.
    #[error("{context}: {source}")]
    Io {
        /// The step.
        context: &'static str,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
    /// The host exited before reporting a running child (`exit` from `exit.json`).
    #[error("ptyhost exited before becoming ready{}", describe(.exit.as_ref()))]
    HostFailed {
        /// The recorded outcome, typically `SpawnFailed` or `HostError` with a diagnostic.
        exit: Option<ExitRecord>,
    },
    /// The host did not become ready in time; it was killed.
    #[error("ptyhost not ready within the spawn timeout{}", describe(.exit.as_ref()))]
    StartTimeout {
        /// The recorded outcome, if the host wrote one.
        exit: Option<ExitRecord>,
    },
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> SpawnError {
    move |source| SpawnError::Io { context, source }
}

fn check_secrets(launch: &LaunchSpec, secrets: &[(String, String)]) -> Result<(), SpawnError> {
    let invalid = |name: &str, reason| SpawnError::InvalidSecret {
        name: name.to_owned(),
        reason,
    };
    for (index, (name, value)) in secrets.iter().enumerate() {
        if !valid_env_name(name) {
            return Err(invalid(name, "invalid variable name"));
        }
        if !launch.secret_env_names.contains(name) {
            return Err(invalid(name, "not listed in secret_env_names"));
        }
        if secrets[..index].iter().any(|(other, _)| other == name) {
            return Err(invalid(name, "provided twice"));
        }
        if value.contains('\0') {
            return Err(invalid(name, "value contains NUL"));
        }
    }
    for name in &launch.secret_env_names {
        if !secrets.iter().any(|(provided, _)| provided == name) {
            return Err(invalid(name, "listed in secret_env_names but not provided"));
        }
    }
    Ok(())
}

fn prepare_run_dir(run_dir: &Path, launch_json: &[u8]) -> Result<(), SpawnError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(run_dir)
        .map_err(io_error("create run directory"))?;
    let meta = fs::symlink_metadata(run_dir).map_err(io_error("inspect run directory"))?;
    if !meta.is_dir() {
        return Err(SpawnError::Io {
            context: "inspect run directory",
            source: std::io::Error::other("not a directory"),
        });
    }
    fs::set_permissions(run_dir, Permissions::from_mode(0o700))
        .map_err(io_error("chmod run directory"))?;
    for name in [LAUNCH_FILE, STATUS_FILE, EXIT_FILE, SOCKET_FILE] {
        if fs::symlink_metadata(run_dir.join(name)).is_ok() {
            return Err(SpawnError::RunDirInUse);
        }
    }
    atomic_write(run_dir, LAUNCH_FILE, launch_json, 0o600).map_err(io_error("write launch.json"))
}

/// The host keeps only locale, terminal, `PATH` and `HOME` from the server environment.
fn host_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            name.to_str().is_some_and(|name| {
                matches!(name, "PATH" | "HOME" | "LANG" | "TERM") || name.starts_with("LC_")
            })
        })
        .collect()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, SpawnError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| SpawnError::Io {
            context: "blocking task",
            source: std::io::Error::other(e.to_string()),
        })
}

/// Starts `exe ptyhost --run-dir <run_dir>` for `launch` and waits until it serves the socket.
///
/// Creates `run_dir` (0700), writes `launch.json` (0600) atomically, spawns the host in its own
/// process group with stdio closed and an environment reduced to `PATH`, `HOME`, `LANG`,
/// `LC_*`, `TERM` plus `secrets` (each must be listed in `secret_env_names`; values never reach
/// disk or argv). Ready means `status.json` exists and the socket accepts a connection (or the
/// run already ended and `exit.json` exists). Otherwise the host and any recorded child are
/// killed and the error carries the `exit.json` diagnostic.
///
/// # Errors
/// Invalid launch or secrets, a used run directory, I/O failures, an early host exit
/// ([`SpawnError::HostFailed`]) or [`SpawnError::StartTimeout`].
pub async fn spawn_host(
    exe: &Path,
    run_dir: &Path,
    launch: &LaunchSpec,
    secrets: &[(String, String)],
    timeout: Duration,
) -> Result<SpawnedHost, SpawnError> {
    launch.validate()?;
    check_secrets(launch, secrets)?;
    let json = serde_json::to_vec_pretty(launch).map_err(|e| SpawnError::Io {
        context: "encode launch.json",
        source: e.into(),
    })?;
    let dir = run_dir.to_path_buf();
    blocking(move || prepare_run_dir(&dir, &json)).await??;

    let mut command = tokio::process::Command::new(exe);
    command
        .arg("ptyhost")
        .arg("--run-dir")
        .arg(run_dir)
        .env_clear()
        .envs(host_env())
        .envs(secrets.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(false);
    let mut child = command.spawn().map_err(io_error("spawn ptyhost"))?;
    let host_pid = child.id().unwrap_or(0);
    let (sender, mut exit) = watch::channel(None);
    let waiter = tokio::spawn(async move {
        let status = child.wait().await;
        let exit = status.map_or(
            HostProcessExit {
                code: None,
                signal: None,
            },
            |s| HostProcessExit {
                code: s.code(),
                signal: s.signal(),
            },
        );
        let _ = sender.send(Some(exit));
    });
    match wait_ready(run_dir, &exit, timeout).await {
        Ok(status) => Ok(SpawnedHost {
            status,
            run_dir: run_dir.to_path_buf(),
            host_pid,
            exit,
            _waiter: waiter,
        }),
        Err(error) => {
            if exit.borrow().is_none() {
                if let Some(pid) = process::pid(host_pid) {
                    let _ = signal_group(pid, Signal::SIGKILL);
                }
                let _ = tokio::time::timeout(KILL_WAIT, wait_exit(&mut exit)).await;
            }
            Err(error)
        }
    }
}

async fn wait_exit(exit: &mut watch::Receiver<Option<HostProcessExit>>) -> HostProcessExit {
    loop {
        if let Some(status) = *exit.borrow() {
            return status;
        }
        if exit.changed().await.is_err() {
            return exit.borrow().unwrap_or(HostProcessExit {
                code: None,
                signal: None,
            });
        }
    }
}

async fn wait_ready(
    run_dir: &Path,
    exit_rx: &watch::Receiver<Option<HostProcessExit>>,
    timeout: Duration,
) -> Result<StatusRecord, SpawnError> {
    let deadline = Instant::now() + timeout;
    loop {
        let dir = run_dir.to_path_buf();
        let (status, exit) = blocking(move || {
            let status = read_status(&dir.join(STATUS_FILE)).ok().flatten();
            let exit = read_exit(&dir).ok().flatten();
            (status, exit)
        })
        .await?;
        if let Some(status) = &status
            && (UnixStream::connect(run_dir.join(SOCKET_FILE)).await.is_ok() || exit.is_some())
        {
            return Ok(status.clone());
        }
        let host_gone = exit_rx.borrow().is_some();
        if host_gone || Instant::now() >= deadline {
            if let Some(status) = status {
                // The child runs in its own session: make sure it does not outlive the host.
                let _ = blocking(move || terminate_recorded(&status)).await;
            }
            return Err(if host_gone {
                SpawnError::HostFailed { exit }
            } else {
                SpawnError::StartTimeout { exit }
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use yulius_contracts::ptyhost::{Containment, RECORD_VERSION};

    fn launch() -> LaunchSpec {
        LaunchSpec {
            version: RECORD_VERSION,
            attempt_id: "a1".into(),
            task_id: "t1".into(),
            argv: vec!["/bin/true".into()],
            cwd: "/".into(),
            env: BTreeMap::new(),
            secret_env_names: vec!["TOKEN_A".into()],
            cols: 80,
            rows: 24,
            containment: Containment::None,
            ring_bytes: 0,
            token_sha256: "0".repeat(64),
            linger_ms: 0,
        }
    }

    #[test]
    fn secrets_must_match_the_declared_names_exactly() {
        let spec = launch();
        let ok = vec![("TOKEN_A".to_owned(), "v".to_owned())];
        assert!(check_secrets(&spec, &ok).is_ok());
        assert!(check_secrets(&spec, &[]).is_err(), "declared but missing");
        let extra = vec![
            ("TOKEN_A".to_owned(), "v".to_owned()),
            ("OTHER".to_owned(), "v".to_owned()),
        ];
        assert!(check_secrets(&spec, &extra).is_err(), "undeclared");
        let twice = vec![
            ("TOKEN_A".to_owned(), "v".to_owned()),
            ("TOKEN_A".to_owned(), "w".to_owned()),
        ];
        assert!(check_secrets(&spec, &twice).is_err());
        let error = check_secrets(&spec, &[("TOKEN_A".to_owned(), "a\0b".to_owned())]);
        assert!(error.is_err_and(|e| !e.to_string().contains("a\0b")));
    }

    #[test]
    fn run_directory_is_private_and_single_use() -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let dir = parent.path().join("runs/a1");
        prepare_run_dir(&dir, b"{}")?;
        let mode = fs::metadata(&dir)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let file_mode = fs::metadata(dir.join(LAUNCH_FILE))?.permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert!(matches!(
            prepare_run_dir(&dir, b"{}"),
            Err(SpawnError::RunDirInUse)
        ));
        Ok(())
    }
}
