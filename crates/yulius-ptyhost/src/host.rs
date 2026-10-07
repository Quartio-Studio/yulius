//! The ptyhost process body (SPEC-002 §7.1, RUN-001..003).
//!
//! Threads and ownership:
//!
//! * the **main** thread sets everything up, then waits for the exit record and the linger;
//! * the **output pump** reads the PTY master (never blocking on clients), feeds the vt100
//!   model, numbers chunks and enqueues them for the controlling client;
//! * the **reaper** waits for the child, kills what is left of its process group (and, on Linux,
//!   its session), reaps it, writes `exit.json` and enqueues `Exit`;
//! * the **acceptor** hands each connection to a connection thread (at most
//!   `MAX_PENDING_HANDSHAKES` unauthenticated at a time) which authenticates the `Hello` within
//!   [`HELLO_TIMEOUT_MS`] and then serves client frames;
//! * one **writer** thread per client drains that client's bounded queue to its socket.
//!
//! Lock order: `state` → client queue, `state` → `master`. PTY input (`pty_input`) is never
//! written while `state` is held, so a child that stops reading cannot stall output.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, Permissions};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use yulius_contracts::ptyhost::{
    ClientFrame, Containment, EXIT_FILE, ExitFile, ExitReason, ExitRecord, FrameDecoder,
    HELLO_TIMEOUT_MS, HostFrame, LAUNCH_FILE, LaunchSpec, LaunchSpecError, MAX_ERROR_MESSAGE_BYTES,
    MAX_EXIT_DETAIL_BYTES, MAX_OUTPUT_CHUNK, MAX_SNAPSHOT_BYTES, PROTOCOL_VERSION, RECORD_VERSION,
    SOCKET_FILE, STATUS_FILE, SignalKind, Snapshot, StatusRecord, Welcome, encode_host, error_code,
    valid_identifier,
};

use crate::containment::bwrap_argv;
use crate::files::{atomic_write, now_ms, read_record, truncate_utf8};
use crate::process::{self, kill_session, signal_group};
use crate::reconcile::current_boot_id;
use crate::token_sha256_hex;

/// Upper bound applied to [`ClientFrame::Shutdown`] grace periods (per escalation step).
pub const MAX_SHUTDOWN_GRACE_MS: u32 = 60_000;
const MAX_PENDING_HANDSHAKES: usize = 8;
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const REJECT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const HELLO_FRAME_BYTES: usize = 40;
const INTERRUPT: &[u8] = &[0x03];

/// Failure of the host itself. Messages carry no secret values.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// `launch.json` is missing or unreadable.
    #[error("launch record: {0}")]
    Launch(String),
    /// `launch.json` violates the contract.
    #[error(transparent)]
    InvalidLaunch(#[from] LaunchSpecError),
    /// The run directory already holds a status, exit record or socket; a run is never
    /// restarted in place (DATA-002).
    #[error("run directory already used (status.json, exit.json or pty.sock present)")]
    RunDirInUse,
    /// An I/O step failed.
    #[error("{context}: {source}")]
    Io {
        /// The step.
        context: &'static str,
        /// The cause.
        #[source]
        source: io::Error,
    },
    /// The PTY could not be created or used.
    #[error("pty: {0}")]
    Pty(String),
    /// The child could not be spawned.
    #[error("spawn: {0}")]
    Spawn(String),
}

fn io_error(context: &'static str) -> impl FnOnce(io::Error) -> HostError {
    move |source| HostError::Io { context, source }
}

/// Runs a ptyhost for `run_dir` until its child has exited and the exit was delivered (or the
/// linger expired). This is the blocking body of `yulius ptyhost --run-dir <dir>`.
///
/// Background threads may still be blocked when this returns; the caller should exit the
/// process. Every failure after `launch.json` was read is recorded in `exit.json` with
/// [`ExitReason::SpawnFailed`] or [`ExitReason::HostError`].
///
/// # Errors
/// Returns the failure that prevented or ended the run abnormally.
pub fn run_host(run_dir: &Path) -> Result<(), HostError> {
    let launch = load_launch(run_dir)?;
    let attempt = launch.attempt_id.clone();
    let fail = |reason, error| record_failure(run_dir, &attempt, reason, error);

    let socket_path = run_dir.join(SOCKET_FILE);
    let listener = UnixListener::bind(&socket_path)
        .map_err(io_error("bind pty.sock"))
        .map_err(|e| fail(ExitReason::HostError, e))?;
    let _socket = SocketGuard(socket_path.clone());
    fs::set_permissions(&socket_path, Permissions::from_mode(0o600))
        .map_err(io_error("chmod pty.sock"))
        .map_err(|e| fail(ExitReason::HostError, e))?;

    let command = prepare_command(&launch).map_err(|e| fail(ExitReason::SpawnFailed, e))?;
    let size = PtySize {
        rows: launch.rows,
        cols: launch.cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|e| fail(ExitReason::HostError, HostError::Pty(e.to_string())))?;
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| fail(ExitReason::SpawnFailed, HostError::Spawn(e.to_string())))?;
    drop(pair.slave);
    let Some(child_pid) = child.process_id().and_then(process::pid) else {
        return Err(fail(
            ExitReason::HostError,
            HostError::Spawn("child has no process id".into()),
        ));
    };

    let started = start_run(run_dir, &launch, child_pid, pair.master);
    let (shared, reader) = match started {
        Ok(parts) => parts,
        Err(error) => {
            abort_child(child_pid);
            return Err(fail(ExitReason::HostError, error));
        }
    };
    if let Err(error) = start_threads(&shared, reader, listener, child) {
        abort_child(child_pid);
        return Err(fail(ExitReason::HostError, error));
    }
    let linger = Duration::from_millis(u64::from(launch.linger_ms));
    wait_for_completion(&shared, linger)
}

fn load_launch(run_dir: &Path) -> Result<LaunchSpec, HostError> {
    for name in [STATUS_FILE, EXIT_FILE, SOCKET_FILE] {
        if fs::symlink_metadata(run_dir.join(name)).is_ok() {
            return Err(HostError::RunDirInUse);
        }
    }
    let launch = match read_record::<LaunchSpec>(&run_dir.join(LAUNCH_FILE)) {
        Ok(Some(launch)) => launch,
        Ok(None) => {
            let error = HostError::Launch("launch.json is missing".into());
            return Err(record_failure(run_dir, "", ExitReason::HostError, error));
        }
        Err(error) => {
            let error = HostError::Launch(error.to_string());
            return Err(record_failure(run_dir, "", ExitReason::HostError, error));
        }
    };
    if let Err(error) = launch.validate() {
        let attempt = if valid_identifier(&launch.attempt_id) {
            launch.attempt_id.as_str()
        } else {
            ""
        };
        return Err(record_failure(
            run_dir,
            attempt,
            ExitReason::HostError,
            error.into(),
        ));
    }
    Ok(launch)
}

/// Builds the child command: launch argv (inside bwrap when contained), cleared environment plus
/// the launch env and the named secrets from the host's own environment.
fn prepare_command(launch: &LaunchSpec) -> Result<CommandBuilder, HostError> {
    let (argv, outer_cwd) = match &launch.containment {
        Containment::None => {
            if !Path::new(&launch.cwd).is_dir() {
                return Err(HostError::Spawn(format!(
                    "working directory {} does not exist or is not a directory",
                    launch.cwd
                )));
            }
            (launch.argv.clone(), launch.cwd.clone())
        }
        Containment::Bubblewrap(spec) => {
            if !cfg!(target_os = "linux") {
                return Err(HostError::Spawn(
                    "bubblewrap containment is only supported on Linux hosts".into(),
                ));
            }
            (bwrap_argv(spec, &launch.argv, &launch.cwd), "/".to_owned())
        }
    };
    let mut command = CommandBuilder::from_argv(argv.into_iter().map(OsString::from).collect());
    command.env_clear();
    for (name, value) in &launch.env {
        command.env(name, value);
    }
    for name in &launch.secret_env_names {
        let Some(value) = std::env::var_os(name) else {
            return Err(HostError::Spawn(format!(
                "secret variable {name} is not set in the host environment"
            )));
        };
        command.env(name, value);
    }
    command.cwd(outer_cwd);
    Ok(command)
}

type PtyReader = Box<dyn Read + Send>;

fn start_run(
    run_dir: &Path,
    launch: &LaunchSpec,
    child_pid: Pid,
    master: Box<dyn MasterPty + Send>,
) -> Result<(Arc<Shared>, PtyReader), HostError> {
    let reader = master
        .try_clone_reader()
        .map_err(|e| HostError::Pty(e.to_string()))?;
    let writer = master
        .take_writer()
        .map_err(|e| HostError::Pty(e.to_string()))?;
    let host_pid = std::process::id();
    let status = StatusRecord {
        version: RECORD_VERSION,
        attempt_id: launch.attempt_id.clone(),
        ptyhost_pid: host_pid,
        child_pid: u32::try_from(child_pid.as_raw()).unwrap_or(0),
        started_at_ms: now_ms(),
        boot_id: current_boot_id().unwrap_or_else(|| "unknown".into()),
        token_sha256: launch.token_sha256.clone(),
    };
    let json = serde_json::to_vec_pretty(&status).map_err(|e| HostError::Io {
        context: "encode status.json",
        source: e.into(),
    })?;
    atomic_write(run_dir, STATUS_FILE, &json, 0o600).map_err(io_error("write status.json"))?;
    let shared = Arc::new(Shared {
        run_dir: run_dir.to_path_buf(),
        attempt_id: launch.attempt_id.clone(),
        token_sha256: launch.token_sha256.clone(),
        child_pid,
        host_pid: Pid::from_raw(i32::try_from(host_pid).unwrap_or(0)),
        queue_limit: launch.client_queue_bytes(),
        state: Mutex::new(State {
            parser: vt100::Parser::new(launch.rows, launch.cols, 0),
            seq: 0,
            cols: launch.cols,
            rows: launch.rows,
            client: None,
            next_client_id: 0,
            exit: None,
            exit_delivered: false,
            exit_file_error: None,
            reaped: false,
            output_done: false,
        }),
        changed: Condvar::new(),
        pty_input: Mutex::new(writer),
        master: Mutex::new(master),
        pending_handshakes: AtomicUsize::new(0),
        shutdown_started: AtomicBool::new(false),
    });
    Ok((shared, reader))
}

fn start_threads(
    shared: &Arc<Shared>,
    reader: PtyReader,
    listener: UnixListener,
    child: Box<dyn portable_pty::Child + Send + Sync>,
) -> Result<(), HostError> {
    let pump = Arc::clone(shared);
    spawn_thread("ptyhost-output", move || pump_output(&pump, reader))?;
    let reaper = Arc::clone(shared);
    spawn_thread("ptyhost-reaper", move || reap_and_record(&reaper, child))?;
    let acceptor = Arc::clone(shared);
    spawn_thread("ptyhost-accept", move || accept_loop(&acceptor, &listener))?;
    Ok(())
}

fn spawn_thread(name: &str, body: impl FnOnce() + Send + 'static) -> Result<(), HostError> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map(drop)
        .map_err(io_error("start thread"))
}

/// Kills and reaps a child whose run could not be set up.
fn abort_child(child_pid: Pid) {
    let _ = signal_group(child_pid, Signal::SIGKILL);
    let _ = waitpid(child_pid, None);
}

fn write_exit(run_dir: &Path, attempt_id: &str, record: &ExitRecord) -> io::Result<()> {
    let file = ExitFile {
        version: RECORD_VERSION,
        attempt_id: attempt_id.to_owned(),
        exit: record.clone(),
    };
    let json = serde_json::to_vec_pretty(&file)?;
    atomic_write(run_dir, EXIT_FILE, &json, 0o600)
}

fn record_failure(run_dir: &Path, attempt_id: &str, reason: ExitReason, error: HostError) -> HostError {
    let record = ExitRecord {
        code: None,
        signal: None,
        ended_at_ms: now_ms(),
        reason,
        detail: Some(truncate_utf8(error.to_string(), MAX_EXIT_DETAIL_BYTES)),
    };
    // Best effort: the error is returned either way.
    let _ = write_exit(run_dir, attempt_id, &record);
    error
}

/// Removes the socket file when the host finishes, so no stale endpoint remains.
struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Shared {
    run_dir: PathBuf,
    attempt_id: String,
    token_sha256: String,
    child_pid: Pid,
    host_pid: Pid,
    queue_limit: usize,
    state: Mutex<State>,
    changed: Condvar,
    pty_input: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    pending_handshakes: AtomicUsize,
    shutdown_started: AtomicBool,
}

struct State {
    parser: vt100::Parser,
    seq: u64,
    cols: u16,
    rows: u16,
    client: Option<Client>,
    next_client_id: u64,
    exit: Option<ExitRecord>,
    exit_delivered: bool,
    exit_file_error: Option<String>,
    reaped: bool,
    output_done: bool,
}

/// The controlling client: its queue and a socket handle used to cut it off.
struct Client {
    id: u64,
    queue: Arc<ClientQueue>,
    socket: UnixStream,
}

impl Client {
    /// Drops the client immediately, discarding queued frames.
    fn cut_off(self) {
        self.queue.close();
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

impl State {
    /// Enqueues for the current client; an overflowing client is cut off (it will reconnect and
    /// receive a fresh snapshot).
    fn push_or_drop(&mut self, frame: &HostFrame) {
        let overflow = self
            .client
            .as_ref()
            .is_some_and(|client| client.queue.push(frame).is_err());
        if overflow && let Some(client) = self.client.take() {
            client.cut_off();
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.client.as_ref().is_some_and(|client| client.id == id)
    }
}

fn unpoison<T>(result: Result<T, PoisonError<T>>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        unpoison(self.state.lock())
    }

    fn wait_timeout<'a>(
        &self,
        guard: MutexGuard<'a, State>,
        timeout: Duration,
    ) -> MutexGuard<'a, State> {
        match self.changed.wait_timeout(guard, timeout) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        }
    }

    /// Waits until `done` holds or `timeout` elapses; returns whether `done` holds.
    fn wait_until(&self, timeout: Duration, done: impl Fn(&State) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if done(&state) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self.wait_timeout(state, deadline - now);
        }
    }

    fn notify(&self) {
        self.changed.notify_all();
    }

    fn write_input(&self, bytes: &[u8]) {
        let mut input = unpoison(self.pty_input.lock());
        // After the child exits the PTY may reject input (EIO); nothing to deliver it to.
        let _ = input.write_all(bytes).and_then(|()| input.flush());
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let mut state = self.lock();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        unpoison(self.master.lock())
            .resize(size)
            .map_err(|e| e.to_string())?;
        state.parser.screen_mut().set_size(rows, cols);
        state.cols = cols;
        state.rows = rows;
        Ok(())
    }

    /// Signals the child's process group unless the child was already reaped (whose process
    /// group id could then be reused).
    fn signal_child(&self, signal: Signal) {
        let state = self.lock();
        if !state.reaped {
            let _ = signal_group(self.child_pid, signal);
            if signal == Signal::SIGKILL {
                kill_session(self.child_pid, self.host_pid);
            }
        }
    }

    fn push_to(&self, id: u64, frame: &HostFrame) {
        let mut state = self.lock();
        if state.is_current(id) {
            state.push_or_drop(frame);
        }
    }

    /// Detaches client `id` if it is still current; queued frames are still flushed.
    fn detach(&self, id: u64) {
        let mut state = self.lock();
        if state.is_current(id)
            && let Some(client) = state.client.take()
        {
            client.queue.finish();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Output pump and reaper

fn pump_output(shared: &Shared, mut reader: PtyReader) {
    let mut buf = vec![0u8; MAX_OUTPUT_CHUNK];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let chunk = buf.get(..n).unwrap_or_default();
        let mut state = shared.lock();
        state.parser.process(chunk);
        state.seq += 1;
        if state.exit.is_none() {
            let frame = HostFrame::Output {
                seq: state.seq,
                bytes: chunk.to_vec(),
            };
            state.push_or_drop(&frame);
        }
    }
    shared.lock().output_done = true;
    shared.notify();
}

fn wait_status(pid: Pid) -> Result<WaitStatus, Errno> {
    loop {
        match waitpid(pid, None) {
            Err(Errno::EINTR) | Ok(WaitStatus::Stopped(..) | WaitStatus::Continued(_)) => {}
            other => return other,
        }
    }
}

/// Waits for the child to exit, removes what is left of its process group and reaps it.
#[cfg(target_os = "linux")]
fn reap(shared: &Shared) -> Result<WaitStatus, Errno> {
    use nix::sys::wait::{Id, WaitPidFlag, waitid};
    // Observe the exit without reaping: while the zombie exists its pid (and process group id)
    // cannot be reused, so the group kill below cannot hit an unrelated process.
    loop {
        match waitid(
            Id::Pid(shared.child_pid),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
        ) {
            Err(Errno::EINTR) => {}
            _ => break,
        }
    }
    let mut state = shared.lock();
    let _ = signal_group(shared.child_pid, Signal::SIGKILL);
    kill_session(shared.child_pid, shared.host_pid);
    let status = wait_status(shared.child_pid);
    state.reaped = true;
    status
}

/// Waits for the child to exit, reaps it and removes what is left of its process group.
#[cfg(not(target_os = "linux"))]
fn reap(shared: &Shared) -> Result<WaitStatus, Errno> {
    let status = wait_status(shared.child_pid);
    let mut state = shared.lock();
    state.reaped = true;
    // Descendants keep the group id reserved, so this reaches exactly the remaining members.
    let _ = signal_group(shared.child_pid, Signal::SIGKILL);
    drop(state);
    status
}

fn exit_record(status: Result<WaitStatus, Errno>) -> ExitRecord {
    let (code, signal, reason, detail) = match status {
        Ok(WaitStatus::Exited(_, code)) => (Some(code), None, ExitReason::Exited, None),
        Ok(WaitStatus::Signaled(_, signal, _)) => {
            (None, Some(signal as i32), ExitReason::Signalled, None)
        }
        Ok(other) => (
            None,
            None,
            ExitReason::HostError,
            Some(format!("unexpected wait status {other:?}")),
        ),
        Err(errno) => (
            None,
            None,
            ExitReason::HostError,
            Some(format!("waitpid failed: {errno}")),
        ),
    };
    ExitRecord {
        code,
        signal,
        ended_at_ms: now_ms(),
        reason,
        detail,
    }
}

fn reap_and_record(shared: &Shared, child: Box<dyn portable_pty::Child + Send + Sync>) {
    let record = exit_record(reap(shared));
    drop(child);
    // Let the pump forward output written before the exit, bounded: a process outside the
    // group may keep the PTY open.
    shared.wait_until(OUTPUT_DRAIN_TIMEOUT, |state| state.output_done);
    // exit.json is durable before the Exit frame is sent (SPEC-002 §7.1).
    let written = write_exit(&shared.run_dir, &shared.attempt_id, &record);
    let mut state = shared.lock();
    if let Err(error) = written {
        state.exit_file_error = Some(error.to_string());
    }
    state.push_or_drop(&HostFrame::Exit(record.clone()));
    state.exit = Some(record);
    drop(state);
    shared.notify();
}

fn wait_for_completion(shared: &Shared, linger: Duration) -> Result<(), HostError> {
    let mut state = shared.lock();
    while state.exit.is_none() {
        state = unpoison(shared.changed.wait(state));
    }
    drop(state);
    shared.wait_until(linger, |state| state.exit_delivered);
    let mut state = shared.lock();
    if let Some(client) = state.client.take() {
        client.queue.finish();
    }
    match state.exit_file_error.take() {
        Some(message) => Err(HostError::Io {
            context: "write exit.json",
            source: io::Error::other(message),
        }),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Shutdown escalation

fn start_shutdown(shared: &Arc<Shared>, grace_ms: u32) {
    if shared.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let grace = Duration::from_millis(u64::from(grace_ms.min(MAX_SHUTDOWN_GRACE_MS)));
    let worker = Arc::clone(shared);
    let started = thread::Builder::new()
        .name("ptyhost-shutdown".into())
        .spawn(move || {
            // The interrupt write may block when the child stopped reading its input; it must
            // not delay the escalation.
            let interrupter = Arc::clone(&worker);
            let _ = thread::Builder::new()
                .name("ptyhost-interrupt".into())
                .spawn(move || interrupter.write_input(INTERRUPT));
            if worker.wait_until(grace, |s| s.reaped) {
                return;
            }
            worker.signal_child(Signal::SIGTERM);
            if worker.wait_until(grace, |s| s.reaped) {
                return;
            }
            worker.signal_child(Signal::SIGKILL);
        });
    if started.is_err() {
        // Fail toward stopping: without an escalation thread, kill now.
        shared.signal_child(Signal::SIGKILL);
    }
}

// ---------------------------------------------------------------------------------------------
// Connections

fn accept_loop(shared: &Arc<Shared>, listener: &UnixListener) {
    for connection in listener.incoming() {
        let Ok(stream) = connection else {
            // For example EMFILE: back off instead of spinning.
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if shared.pending_handshakes.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING_HANDSHAKES {
            shared.pending_handshakes.fetch_sub(1, Ordering::SeqCst);
            reject_now(stream, error_code::BUSY, "too many pending connections");
            continue;
        }
        let worker = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name("ptyhost-conn".into())
            .spawn(move || serve_connection(&worker, stream));
        if spawned.is_err() {
            shared.pending_handshakes.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Decrements the pending-handshake counter when the handshake ends.
struct PendingGuard<'a>(&'a AtomicUsize);

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn error_frame(code: u16, message: &str) -> HostFrame {
    HostFrame::Error {
        code,
        message: truncate_utf8(message.to_owned(), MAX_ERROR_MESSAGE_BYTES),
    }
}

/// Sends an error to an unauthenticated connection and closes it.
fn reject(stream: &mut UnixStream, code: u16, message: &str) {
    let _ = stream.set_write_timeout(Some(REJECT_WRITE_TIMEOUT));
    let _ = stream.write_all(&encode_host(&error_frame(code, message)));
    let _ = stream.shutdown(Shutdown::Both);
}

/// Like [`reject`] but never blocks (used on the acceptor thread).
fn reject_now(mut stream: UnixStream, code: u16, message: &str) {
    let _ = stream.set_nonblocking(true);
    let _ = stream.write(&encode_host(&error_frame(code, message)));
    let _ = stream.shutdown(Shutdown::Both);
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads the first frame within the hello timeout and authenticates it.
fn handshake(shared: &Shared, stream: &mut UnixStream) -> Option<(bool, FrameDecoder)> {
    let deadline = Instant::now() + Duration::from_millis(HELLO_TIMEOUT_MS);
    let mut decoder = FrameDecoder::new();
    let mut buf = [0u8; 1024];
    loop {
        match decoder.next_client() {
            Ok(Some(ClientFrame::Hello {
                version,
                token,
                want_snapshot,
            })) => {
                if version != PROTOCOL_VERSION {
                    reject(
                        stream,
                        error_code::UNSUPPORTED_VERSION,
                        &format!("unsupported protocol version {version}"),
                    );
                    return None;
                }
                let digest = token_sha256_hex(&token);
                if !constant_time_eq(digest.as_bytes(), shared.token_sha256.as_bytes()) {
                    reject(stream, error_code::AUTH_FAILED, "authentication failed");
                    return None;
                }
                return Some((want_snapshot, decoder));
            }
            Ok(Some(_)) => {
                reject(stream, error_code::PROTOCOL, "first frame must be Hello");
                return None;
            }
            Err(error) => {
                reject(stream, error_code::PROTOCOL, &error.to_string());
                return None;
            }
            Ok(None) if decoder.buffered() >= HELLO_FRAME_BYTES => {
                reject(stream, error_code::PROTOCOL, "first frame must be Hello");
                return None;
            }
            Ok(None) => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            reject(stream, error_code::HELLO_TIMEOUT, "no Hello within 5 s");
            return None;
        }
        match stream.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => decoder.push(buf.get(..n).unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                reject(stream, error_code::HELLO_TIMEOUT, "no Hello within 5 s");
                return None;
            }
            Err(_) => return None,
        }
    }
}

fn serve_connection(shared: &Arc<Shared>, mut stream: UnixStream) {
    let hello = {
        let _pending = PendingGuard(&shared.pending_handshakes);
        handshake(shared, &mut stream)
    };
    let Some((want_snapshot, decoder)) = hello else {
        return;
    };
    if stream.set_read_timeout(None).is_err()
        || stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let Ok((id, queue)) = attach(shared, &stream, want_snapshot) else {
        return;
    };
    serve_client(shared, id, &queue, stream, decoder);
    shared.detach(id);
}

/// Formatted screen: visible contents, input modes, cursor and attributes; falls back to plain
/// text when the formatted form exceeds the snapshot bound.
fn snapshot_bytes(parser: &vt100::Parser) -> Vec<u8> {
    let screen = parser.screen();
    let mut bytes = Vec::new();
    if screen.alternate_screen() {
        bytes.extend_from_slice(b"\x1b[?1049h");
    }
    bytes.extend(screen.contents_formatted());
    bytes.extend(screen.input_mode_formatted());
    bytes.extend(screen.cursor_state_formatted());
    bytes.extend(screen.attributes_formatted());
    if bytes.len() <= MAX_SNAPSHOT_BYTES {
        return bytes;
    }
    let plain = format!(
        "\x1b[0m\x1b[H\x1b[2J{}",
        screen.contents().replace('\n', "\r\n")
    );
    truncate_utf8(plain, MAX_SNAPSHOT_BYTES).into_bytes()
}

/// Makes the connection the controlling client: queues Welcome, the optional Snapshot and a
/// recorded Exit atomically with respect to output, replaces the previous client and starts the
/// writer thread.
fn attach(
    shared: &Arc<Shared>,
    stream: &UnixStream,
    want_snapshot: bool,
) -> io::Result<(u64, Arc<ClientQueue>)> {
    let writer_stream = stream.try_clone()?;
    let socket = stream.try_clone()?;
    let queue = Arc::new(ClientQueue::new(shared.queue_limit));
    let id = {
        let mut state = shared.lock();
        state.next_client_id += 1;
        let id = state.next_client_id;
        let welcome = HostFrame::Welcome(Welcome {
            attempt_id: shared.attempt_id.clone(),
            child_pid: u32::try_from(shared.child_pid.as_raw()).unwrap_or(0),
            seq: state.seq,
            cols: state.cols,
            rows: state.rows,
        });
        // The queue bound (≥ 2 MiB) always fits these initial frames.
        let _ = queue.push(&welcome);
        if want_snapshot {
            let snapshot = HostFrame::Snapshot(Snapshot {
                seq: state.seq,
                bytes: snapshot_bytes(&state.parser),
            });
            let _ = queue.push(&snapshot);
        }
        if let Some(exit) = &state.exit {
            let _ = queue.push(&HostFrame::Exit(exit.clone()));
        }
        let client = Client {
            id,
            queue: Arc::clone(&queue),
            socket,
        };
        if let Some(previous) = state.client.replace(client) {
            previous.queue.finish_with(&error_frame(
                error_code::REPLACED,
                "another client took control",
            ));
        }
        id
    };
    let writer_shared = Arc::clone(shared);
    let writer_queue = Arc::clone(&queue);
    let started = thread::Builder::new()
        .name("ptyhost-writer".into())
        .spawn(move || write_frames(&writer_shared, &writer_queue, writer_stream));
    if let Err(error) = started {
        shared.detach(id);
        return Err(error);
    }
    Ok((id, queue))
}

fn write_frames(shared: &Shared, queue: &ClientQueue, mut stream: UnixStream) {
    while let Some(item) = queue.pop() {
        if stream.write_all(&item.bytes).is_err() {
            queue.close();
            break;
        }
        if item.is_exit {
            shared.lock().exit_delivered = true;
            shared.notify();
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn serve_client(
    shared: &Arc<Shared>,
    id: u64,
    queue: &ClientQueue,
    mut stream: UnixStream,
    mut decoder: FrameDecoder,
) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        loop {
            match decoder.next_client() {
                Ok(Some(frame)) => {
                    if !handle_frame(shared, id, queue, frame) {
                        return;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    queue.finish_with(&error_frame(error_code::PROTOCOL, &error.to_string()));
                    return;
                }
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => decoder.push(buf.get(..n).unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Applies one frame of client `id`; returns `false` when the connection must stop.
fn handle_frame(shared: &Arc<Shared>, id: u64, queue: &ClientQueue, frame: ClientFrame) -> bool {
    // A replaced or dropped client can no longer act on the session.
    if !shared.lock().is_current(id) {
        return false;
    }
    match frame {
        ClientFrame::Hello { .. } => {
            queue.finish_with(&error_frame(error_code::PROTOCOL, "duplicate Hello"));
            return false;
        }
        ClientFrame::Input(bytes) => shared.write_input(&bytes),
        ClientFrame::Resize { cols, rows } => {
            if let Err(message) = shared.resize(cols, rows) {
                shared.push_to(id, &error_frame(error_code::INTERNAL, &message));
            }
        }
        ClientFrame::Signal(SignalKind::Interrupt) => shared.write_input(INTERRUPT),
        ClientFrame::Signal(SignalKind::Terminate) => shared.signal_child(Signal::SIGTERM),
        ClientFrame::Signal(SignalKind::Kill) => shared.signal_child(Signal::SIGKILL),
        ClientFrame::Shutdown { grace_ms } => start_shutdown(shared, grace_ms),
        ClientFrame::Ping(nonce) => shared.push_to(id, &HostFrame::Pong(nonce)),
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Bounded per-client queue

struct Queued {
    bytes: Vec<u8>,
    is_exit: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueMode {
    Open,
    /// No new frames; the writer flushes what is queued, then closes the connection.
    Draining,
    /// Discard everything; the writer stops.
    Closed,
}

struct QueueState {
    frames: VecDeque<Queued>,
    bytes: usize,
    mode: QueueMode,
}

/// Encoded frames waiting for one client, bounded in bytes.
struct ClientQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    limit: usize,
}

/// The frame did not fit in the queue bound.
struct Overflow;

impl ClientQueue {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                frames: VecDeque::new(),
                bytes: 0,
                mode: QueueMode::Open,
            }),
            ready: Condvar::new(),
            limit,
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        unpoison(self.state.lock())
    }

    /// Enqueues a frame; fails when the bound would be exceeded. Frames pushed after the queue
    /// stopped accepting are silently discarded.
    fn push(&self, frame: &HostFrame) -> Result<(), Overflow> {
        let bytes = encode_host(frame);
        let mut state = self.lock();
        if state.mode != QueueMode::Open {
            return Ok(());
        }
        if state.bytes + bytes.len() > self.limit {
            return Err(Overflow);
        }
        state.bytes += bytes.len();
        state.frames.push_back(Queued {
            is_exit: matches!(frame, HostFrame::Exit(_)),
            bytes,
        });
        drop(state);
        self.ready.notify_all();
        Ok(())
    }

    /// Replaces pending frames with a final frame, then closes after it is flushed.
    fn finish_with(&self, frame: &HostFrame) {
        let mut state = self.lock();
        if state.mode == QueueMode::Open {
            let bytes = encode_host(frame);
            state.bytes = bytes.len();
            state.frames.clear();
            state.frames.push_back(Queued {
                bytes,
                is_exit: false,
            });
            state.mode = QueueMode::Draining;
        }
        drop(state);
        self.ready.notify_all();
    }

    /// Stops accepting frames; queued frames are still flushed.
    fn finish(&self) {
        let mut state = self.lock();
        if state.mode == QueueMode::Open {
            state.mode = QueueMode::Draining;
        }
        drop(state);
        self.ready.notify_all();
    }

    /// Discards queued frames and stops the writer.
    fn close(&self) {
        let mut state = self.lock();
        state.mode = QueueMode::Closed;
        state.frames.clear();
        state.bytes = 0;
        drop(state);
        self.ready.notify_all();
    }

    /// Next frame to write; `None` once closed, or draining and empty.
    fn pop(&self) -> Option<Queued> {
        let mut state = self.lock();
        loop {
            if state.mode == QueueMode::Closed {
                return None;
            }
            if let Some(item) = state.frames.pop_front() {
                state.bytes -= item.bytes.len();
                return Some(item);
            }
            if state.mode == QueueMode::Draining {
                return None;
            }
            state = unpoison(self.ready.wait(state));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enforces_its_byte_bound_and_drains_after_finish() {
        let queue = ClientQueue::new(100);
        let small = HostFrame::Pong(1); // 13 bytes encoded
        for _ in 0..7 {
            assert!(queue.push(&small).is_ok());
        }
        assert!(queue.push(&small).is_err(), "8 * 13 > 100");
        queue.finish();
        assert!(queue.push(&small).is_ok(), "ignored after finish");
        let mut drained = 0;
        while queue.pop().is_some() {
            drained += 1;
        }
        assert_eq!(drained, 7);
    }

    #[test]
    fn finish_with_replaces_pending_frames_and_close_discards() {
        let queue = ClientQueue::new(1 << 20);
        let _ = queue.push(&HostFrame::Pong(1));
        queue.finish_with(&error_frame(error_code::REPLACED, "replaced"));
        assert!(matches!(queue.pop(), Some(item) if !item.is_exit && item.bytes[4] == 0x86));
        assert!(queue.pop().is_none());

        let queue = ClientQueue::new(1 << 20);
        let _ = queue.push(&HostFrame::Pong(1));
        queue.close();
        assert!(queue.pop().is_none());
    }

    #[test]
    fn snapshot_reproduces_the_screen_in_a_fresh_parser() {
        let mut parser = vt100::Parser::new(5, 20, 0);
        parser.process(b"\x1b[1;31mred\x1b[0m line\r\nsecond\x1b[3;5Hcursor");
        let mut replay = vt100::Parser::new(5, 20, 0);
        replay.process(&snapshot_bytes(&parser));
        assert_eq!(replay.screen().contents(), parser.screen().contents());
        assert_eq!(
            replay.screen().cursor_position(),
            parser.screen().cursor_position()
        );
        assert_eq!(
            replay.screen().cell(0, 0).map(vt100::Cell::fgcolor),
            Some(vt100::Color::Idx(1))
        );
    }

    #[test]
    fn oversized_snapshot_falls_back_to_bounded_plain_text() {
        let mut parser = vt100::Parser::new(1000, 1000, 0);
        // Alternate colors per cell so the formatted form is far larger than the plain one.
        let mut bytes = Vec::new();
        for i in 0..1_000_000u32 {
            bytes.extend_from_slice(format!("\x1b[3{}m", i % 8).as_bytes());
            bytes.push(b'#');
        }
        parser.process(&bytes);
        let snapshot = snapshot_bytes(&parser);
        assert!(snapshot.len() <= MAX_SNAPSHOT_BYTES);
        assert!(snapshot.starts_with(b"\x1b[0m\x1b[H\x1b[2J"));
    }

    #[test]
    fn constant_time_compare_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
