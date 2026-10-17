//! The sandbox's Docker socket, served by the holder (ADR-0151, SPEC-187 DKR-002).
//!
//! [`Supervisor::serve`] accepts on the sandbox's socket. The first connection starts the engine
//! ([`EngineSpec::command`](yulius_contracts::sandbox::EngineSpec)) and waits until its control
//! socket answers. Every connection is then spliced to it, bytes unchanged. When the engine
//! cannot start, the client gets a Docker-API error (HTTP 503 with a JSON `message`) naming the
//! reason and the engine log's last lines. [`Supervisor::stop_if_idle`] stops the engine once no
//! client has been connected for the idle time and no process is left in a group below its
//! cgroup (a container). Stopping closes the engine's stdin, which `yulius sandbox-host engine`
//! takes as "stop `dockerd`", then kills it after [`STOP_GRACE`].
//!
//! The holder serves at most [`crate::relay::MAX_CONNECTIONS`] clients at once; one start runs at
//! a time and later clients wait for it. The engine can write its control directory, so the
//! holder never follows a link there (`connect_control`).

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Child;
use tokio::sync::Semaphore;

/// Bound on the engine becoming ready after it is started.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// Grace after the engine's stdin closes before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(30);
/// How often the holder looks whether the engine is idle.
pub const IDLE_CHECK: Duration = Duration::from_secs(30);
/// A failed start is the answer this long, so clients queued behind it do not each start the
/// engine again.
pub const FAILURE_HOLD: Duration = Duration::from_secs(5);
/// The engine log is started afresh beyond this size.
const LOG_MAX_BYTES: u64 = 8 << 20;
/// Longest request head read before an error answer.
const REQUEST_HEAD_MAX: usize = 64 * 1024;
/// Time a client has to send its request head before an error answer.
const REQUEST_HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// Directories below the engine's cgroup looked at for containers.
const MAX_GROUPS: usize = 4096;

/// Prepares the namespaces before each start (the holder mounts the engine's sysfs).
pub type Prepare = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// See the module docs.
pub struct Supervisor {
    command: Vec<String>,
    control: PathBuf,
    log: PathBuf,
    cgroup: PathBuf,
    idle: Duration,
    start_timeout: Duration,
    prepare: Prepare,
    running: tokio::sync::Mutex<Option<Child>>,
    failed: Mutex<Option<(Instant, String)>>,
    connections: AtomicUsize,
    last_activity: Mutex<Instant>,
}

impl Supervisor {
    /// A supervisor of `command` whose engine answers on `control`, writes `log` and puts its
    /// containers below `cgroup`.
    #[must_use]
    pub fn new(
        command: Vec<String>,
        control: PathBuf,
        log: PathBuf,
        cgroup: PathBuf,
        idle: Duration,
        prepare: Prepare,
    ) -> Arc<Self> {
        Self::with_start_timeout(command, control, log, cgroup, idle, prepare, START_TIMEOUT)
    }

    /// As [`Supervisor::new`] with another start bound (tests).
    #[must_use]
    pub fn with_start_timeout(
        command: Vec<String>,
        control: PathBuf,
        log: PathBuf,
        cgroup: PathBuf,
        idle: Duration,
        prepare: Prepare,
        start_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            command,
            control,
            log,
            cgroup,
            idle,
            start_timeout,
            prepare,
            running: tokio::sync::Mutex::new(None),
            failed: Mutex::new(None),
            connections: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
        })
    }

    fn touch(&self) {
        *self
            .last_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    /// Serves the sandbox's socket until the listener fails.
    pub async fn serve(self: Arc<Self>, listener: UnixListener) {
        let limit = Arc::new(Semaphore::new(crate::relay::MAX_CONNECTIONS));
        loop {
            let Ok((client, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
                continue;
            };
            let supervisor = Arc::clone(&self);
            tokio::spawn(async move {
                supervisor.connections.fetch_add(1, Ordering::SeqCst);
                supervisor.touch();
                match supervisor.connect().await {
                    Ok(engine) => crate::relay::splice(client, engine).await,
                    Err(reason) => refuse(client, &reason).await,
                }
                supervisor.touch();
                supervisor.connections.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            });
        }
    }

    /// A connection to the engine, started first when it does not run.
    ///
    /// # Errors
    /// Why the engine is not there, for the client.
    pub async fn connect(&self) -> Result<UnixStream, String> {
        let mut running = self.running.lock().await;
        let alive = running
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
        if alive && let Ok(stream) = connect_control(&self.control).await {
            return Ok(stream);
        }
        if !alive {
            let failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, reason)) = failed.as_ref()
                && at.elapsed() < FAILURE_HOLD
            {
                return Err(reason.clone());
            }
            drop(failed);
            match self.spawn() {
                Ok(child) => *running = Some(child),
                Err(reason) => return Err(self.failure(reason)),
            }
        }
        let Some(child) = running.as_mut() else {
            return Err("the engine is gone".into());
        };
        match self.wait_ready(child).await {
            Ok(stream) => {
                *self.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Ok(stream)
            }
            Err(reason) => {
                if let Some(mut child) = running.take() {
                    let _ = child.kill().await;
                }
                Err(self.failure(format!("{reason}{}", log_tail(&self.log))))
            }
        }
    }

    /// Keeps `reason` as the answer for [`FAILURE_HOLD`] and returns it.
    fn failure(&self, reason: String) -> String {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Instant::now(), reason.clone()));
        reason
    }

    fn spawn(&self) -> Result<Child, String> {
        (self.prepare)()?;
        // A restart of `yulius.service` prunes an idle engine's empty leaf (SPEC-201 DBX-005).
        crate::cgroup::restore_leaf(&self.cgroup)
            .map_err(|e| format!("the engine's cgroup {}: {e}", self.cgroup.display()))?;
        let Some((program, args)) = self.command.split_first() else {
            return Err("no engine command".into());
        };
        let _ = std::fs::remove_file(&self.control);
        if std::fs::metadata(&self.log).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
            let _ = std::fs::remove_file(&self.log);
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| format!("engine log: {e}"))?;
        let err = log.try_clone().map_err(|e| format!("engine log: {e}"))?;
        tokio::process::Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start the engine: {e}"))
    }

    async fn wait_ready(&self, child: &mut Child) -> Result<UnixStream, String> {
        let deadline = Instant::now() + self.start_timeout;
        loop {
            if let Ok(stream) = connect_control(&self.control).await {
                return Ok(stream);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("the engine exited ({status})"));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the engine did not answer within {} s",
                    self.start_timeout.as_secs_f32()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Whether the engine runs now.
    pub async fn running(&self) -> bool {
        self.running
            .lock()
            .await
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// Stops the engine when no client has been connected for the idle time and no container
    /// runs; returns whether it stopped it.
    pub async fn stop_if_idle(&self) -> bool {
        if self.connections.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let quiet = self
            .last_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed();
        if quiet < self.idle {
            return false;
        }
        // A start in progress holds the lock: not idle.
        let Ok(mut running) = self.running.try_lock() else {
            return false;
        };
        if running.is_none() {
            return false;
        }
        let cgroup = self.cgroup.clone();
        let busy = tokio::task::spawn_blocking(move || containers_running(&cgroup))
            .await
            .unwrap_or(true);
        if busy {
            return false;
        }
        if let Some(child) = running.take() {
            stop(child, STOP_GRACE).await;
        }
        true
    }

    /// Stops the engine (the holder is stopping), waiting at most `grace`.
    pub async fn stop(&self, grace: Duration) {
        if let Some(child) = self.running.lock().await.take() {
            stop(child, grace).await;
        }
    }

    /// Calls [`Supervisor::stop_if_idle`] every [`IDLE_CHECK`].
    pub async fn watch_idle(self: Arc<Self>) {
        loop {
            tokio::time::sleep(IDLE_CHECK).await;
            if self.stop_if_idle().await {
                tracing::info!("the sandbox's Docker engine stopped (idle)");
            }
        }
    }
}

/// Connects to the engine's control socket without following a link the engine (or one of its
/// containers, which may bind the directory) put there: the holder sees the whole host, so the
/// name is opened with `O_PATH | O_NOFOLLOW`, must be a socket, and is connected through
/// `/proc/self/fd`, which names exactly that inode (no second lookup to race).
#[cfg(target_os = "linux")]
pub(crate) async fn connect_control(path: &Path) -> io::Result<UnixStream> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _};
    let flags = nix::fcntl::OFlag::O_PATH | nix::fcntl::OFlag::O_NOFOLLOW;
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits())
        .open(path)?;
    if !handle.metadata()?.file_type().is_socket() {
        return Err(io::Error::other(
            "the engine's control socket is not a socket",
        ));
    }
    let stream = UnixStream::connect(format!("/proc/self/fd/{}", handle.as_raw_fd())).await;
    drop(handle);
    stream
}

/// The sandbox's Docker engine runs in a holder's namespaces, which are Linux-only.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn connect_control(_path: &Path) -> io::Result<UnixStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the sandbox's Docker engine is Linux-only",
    ))
}

async fn stop(mut child: Child, grace: Duration) {
    drop(child.stdin.take());
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

/// Whether a process runs in a group below `cgroup` (a container: the engine itself lives in
/// `cgroup`), bounded to 4096 directories.
#[must_use]
pub fn containers_running(cgroup: &Path) -> bool {
    let mut stack: Vec<PathBuf> = children(cgroup);
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        seen += 1;
        if seen > MAX_GROUPS {
            return true;
        }
        let procs = std::fs::read_to_string(dir.join("cgroup.procs")).unwrap_or_default();
        if !procs.trim().is_empty() {
            return true;
        }
        stack.extend(children(&dir));
    }
    false
}

fn children(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(5).collect();
    if lines.is_empty() {
        return String::new();
    }
    let tail: Vec<&str> = lines.into_iter().rev().collect();
    format!("; engine log: {}", tail.join(" | "))
}

/// Answers a client the engine could not serve: its request head is read (bounded), then a
/// Docker-API error, which `docker` prints as "Error response from daemon: …", naming how the
/// operator checks the host (SPEC-196 DKL-005).
pub(crate) async fn refuse(mut client: UnixStream, reason: &str) {
    let mut head = Vec::new();
    let _ = tokio::time::timeout(REQUEST_HEAD_TIMEOUT, async {
        let mut buf = [0u8; 4096];
        while head.len() < REQUEST_HEAD_MAX && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match client.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await;
    let body = serde_json::json!({
        "message": format!(
            "the sandbox's Docker engine is not available: {reason}. The operator checks the host \
             with `yulius doctor` and `yulius doctor --docker-smoke`"
        )
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = client.write_all(response.as_bytes()).await;
    let _ = client.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake engine: listens on `argv[1]`, answers `engine:<request>` per connection (holding
    /// a connection that sends nothing open), counts its starts in `argv[2]`, and writes
    /// `argv[3]` when its stdin closes.
    const FAKE: &str = r#"
import os, socket, sys, threading
path, starts, stopped = sys.argv[1:4]
open(starts, "a").write("start\n")
try:
    os.unlink(path)
except FileNotFoundError:
    pass
s = socket.socket(socket.AF_UNIX)
s.bind(path)
s.listen(64)
def one(c):
    data = c.recv(65536)
    c.sendall(b"engine:" + data)
    c.close()
def serve():
    while True:
        c, _ = s.accept()
        threading.Thread(target=one, args=(c,), daemon=True).start()
threading.Thread(target=serve, daemon=True).start()
sys.stdin.read()
open(stopped, "w").write("stopped")
"#;

    struct Fixture {
        dir: tempfile::TempDir,
        socket: PathBuf,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            let dir = tempfile::tempdir()?;
            std::fs::create_dir_all(dir.path().join("cg/docker/c1"))?;
            std::fs::write(dir.path().join("cg/docker/c1/cgroup.procs"), "")?;
            let socket = dir.path().join("docker.sock");
            Ok(Self { dir, socket })
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn fake(&self) -> Vec<String> {
            [
                "python3".into(),
                "-I".into(),
                "-c".into(),
                FAKE.into(),
                self.path("control.sock").to_string_lossy().into_owned(),
                self.path("starts").to_string_lossy().into_owned(),
                self.path("stopped").to_string_lossy().into_owned(),
            ]
            .to_vec()
        }

        fn supervisor(
            &self,
            command: Vec<String>,
            idle: Duration,
            prepare: Prepare,
        ) -> Arc<Supervisor> {
            Supervisor::with_start_timeout(
                command,
                self.path("control.sock"),
                self.path("engine.log"),
                self.path("cg"),
                idle,
                prepare,
                Duration::from_secs(20),
            )
        }

        fn serve(&self, supervisor: &Arc<Supervisor>) -> io::Result<tokio::task::JoinHandle<()>> {
            let listener = UnixListener::bind(&self.socket)?;
            Ok(tokio::spawn(Arc::clone(supervisor).serve(listener)))
        }

        fn starts(&self) -> usize {
            std::fs::read_to_string(self.path("starts"))
                .unwrap_or_default()
                .lines()
                .count()
        }
    }

    fn ok() -> Prepare {
        Box::new(|| Ok(()))
    }

    async fn ask(socket: &Path, text: &str) -> io::Result<String> {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_all(text.as_bytes()).await?;
        let mut out = String::new();
        tokio::time::timeout(Duration::from_secs(30), stream.read_to_string(&mut out))
            .await
            .map_err(|_| io::Error::other("no answer"))??;
        Ok(out)
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!("timed out waiting for {what}")));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_first_connections_start_one_engine_and_reach_it() -> io::Result<()> {
        let f = Fixture::new()?;
        let supervisor = f.supervisor(f.fake(), Duration::from_secs(300), ok());
        let server = f.serve(&supervisor)?;
        let mut clients = Vec::new();
        for i in 0..6 {
            let socket = f.socket.clone();
            clients.push(tokio::spawn(async move {
                ask(&socket, &format!("ping{i}")).await
            }));
        }
        for (i, client) in clients.into_iter().enumerate() {
            let answer = client.await.map_err(io::Error::other)??;
            assert_eq!(answer, format!("engine:ping{i}"));
        }
        assert_eq!(f.starts(), 1);
        assert!(supervisor.running().await);
        assert_eq!(ask(&f.socket, "again").await?, "engine:again");
        assert_eq!(f.starts(), 1, "a running engine is not started again");
        supervisor.stop(Duration::from_secs(10)).await;
        server.abort();
        Ok(())
    }

    /// SPEC-201 DBX-005 (#614): `yulius.service` stops with `KillMode=process`, and systemd then
    /// prunes the unit's empty groups, the leaf of an idle engine among them. The next start
    /// recreates that leaf (only it) before the engine joins it.
    #[tokio::test]
    async fn a_start_recreates_the_engine_leaf_a_restart_pruned() -> io::Result<()> {
        let f = Fixture::new()?;
        // Stands in for `yulius sandbox-exec --cgroup <leaf>`: no group, no engine.
        let joining = |leaf: &Path| {
            let mut command: Vec<String> = vec![
                "/bin/sh".into(),
                "-c".into(),
                r#"test -d "$0" || { echo "cannot join $0" >&2; exit 1; }; exec "$@""#.into(),
                leaf.to_string_lossy().into_owned(),
            ];
            command.extend(f.fake());
            command
        };
        let sandbox = f.path("sandbox");
        std::fs::create_dir_all(&sandbox)?;
        let leaf = sandbox.join("engine");
        let supervisor = Supervisor::with_start_timeout(
            joining(&leaf),
            f.path("control.sock"),
            f.path("engine.log"),
            leaf.clone(),
            Duration::from_secs(300),
            ok(),
            Duration::from_secs(20),
        );
        supervisor.connect().await.map_err(io::Error::other)?;
        assert!(leaf.is_dir(), "the leaf is back");
        supervisor.stop(Duration::from_secs(10)).await;

        // Without the sandbox's group, nothing is made up in its place (it would have no limits).
        let orphan_leaf = f.path("gone/engine");
        let orphan = Supervisor::with_start_timeout(
            joining(&orphan_leaf),
            f.path("control.sock"),
            f.path("engine.log"),
            orphan_leaf,
            Duration::from_secs(300),
            ok(),
            Duration::from_secs(20),
        );
        let reason = orphan.connect().await.err().unwrap_or_default();
        // Review round 1 (low): refused before the engine is spawned, naming the group.
        assert!(reason.starts_with("the engine's cgroup "), "{reason}");
        assert!(reason.contains("gone/engine"), "{reason}");
        assert!(!f.path("gone").exists());
        Ok(())
    }

    #[tokio::test]
    async fn an_engine_that_exits_answers_a_docker_error_with_its_log() -> io::Result<()> {
        let f = Fixture::new()?;
        let command = [
            "/bin/sh",
            "-c",
            "echo 'failed to start daemon: boom' >&2; exit 3",
        ]
        .map(str::to_owned)
        .to_vec();
        let supervisor = f.supervisor(command, Duration::from_secs(300), ok());
        let server = f.serve(&supervisor)?;
        let answer = ask(&f.socket, "GET /_ping HTTP/1.1\r\nHost: docker\r\n\r\n").await?;
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        let body = answer.split("\r\n\r\n").nth(1).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(body).map_err(io::Error::other)?;
        let message = json["message"].as_str().unwrap_or_default();
        assert!(message.contains("exited"), "{message}");
        assert!(message.contains("boom"), "{message}");
        assert!(!supervisor.running().await);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn a_start_is_bounded_and_the_slow_engine_killed() -> io::Result<()> {
        let f = Fixture::new()?;
        let pidfile = f.path("pid");
        let command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("echo $$ > {}; exec sleep 30", pidfile.display()),
        ];
        let supervisor = Supervisor::with_start_timeout(
            command,
            f.path("control.sock"),
            f.path("engine.log"),
            f.path("cg"),
            Duration::from_secs(300),
            ok(),
            Duration::from_millis(500),
        );
        let started = Instant::now();
        let reason = supervisor.connect().await.err().unwrap_or_default();
        assert!(reason.contains("did not answer"), "{reason}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid: i32 = std::fs::read_to_string(&pidfile)?
            .trim()
            .parse()
            .map_err(io::Error::other)?;
        until("the slow engine to be gone", || {
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err()
        })
        .await?;
        assert!(!supervisor.running().await);
        Ok(())
    }

    /// Review of SPEC-187 (low): clients queued behind a start that fails share its answer
    /// instead of starting the engine again one after the other (N × the start bound).
    #[tokio::test]
    async fn clients_queued_behind_a_failed_start_share_its_answer() -> io::Result<()> {
        let f = Fixture::new()?;
        let command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!(
                "echo start >> {}; exec sleep 30",
                f.path("starts").display()
            ),
        ];
        let supervisor = Supervisor::with_start_timeout(
            command,
            f.path("control.sock"),
            f.path("engine.log"),
            f.path("cg"),
            Duration::from_secs(300),
            ok(),
            Duration::from_millis(700),
        );
        let started = Instant::now();
        let mut clients = Vec::new();
        for _ in 0..4 {
            let supervisor = Arc::clone(&supervisor);
            clients.push(tokio::spawn(
                async move { supervisor.connect().await.err() },
            ));
        }
        for client in clients {
            let reason = client.await.map_err(io::Error::other)?.unwrap_or_default();
            assert!(reason.contains("did not answer"), "{reason}");
        }
        assert_eq!(f.starts(), 1, "one start for the queue");
        assert!(
            started.elapsed() < Duration::from_millis(2_000),
            "{:?}",
            started.elapsed()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_preparation_is_the_answer() -> io::Result<()> {
        let f = Fixture::new()?;
        let supervisor = f.supervisor(
            f.fake(),
            Duration::from_secs(300),
            Box::new(|| Err("cannot mount the engine's sysfs: EPERM".into())),
        );
        let server = f.serve(&supervisor)?;
        let answer = ask(&f.socket, "GET /_ping HTTP/1.1\r\n\r\n").await?;
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert!(answer.contains("sysfs: EPERM"), "{answer}");
        assert_eq!(f.starts(), 0);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn an_idle_engine_stops_gracefully_and_starts_again_on_use() -> io::Result<()> {
        let f = Fixture::new()?;
        let supervisor = f.supervisor(f.fake(), Duration::ZERO, ok());
        let server = f.serve(&supervisor)?;
        assert_eq!(ask(&f.socket, "one").await?, "engine:one");
        until("the client to be counted out", || {
            supervisor.connections.load(Ordering::SeqCst) == 0
        })
        .await?;
        assert!(supervisor.stop_if_idle().await);
        until("the engine's graceful stop", || f.path("stopped").exists()).await?;
        assert!(!supervisor.running().await);
        assert!(!supervisor.stop_if_idle().await, "nothing left to stop");
        assert_eq!(ask(&f.socket, "two").await?, "engine:two");
        assert_eq!(f.starts(), 2);
        supervisor.stop(Duration::from_secs(10)).await;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn a_client_a_container_or_recent_use_keeps_the_engine() -> io::Result<()> {
        let f = Fixture::new()?;
        let supervisor = f.supervisor(f.fake(), Duration::ZERO, ok());
        let server = f.serve(&supervisor)?;
        assert_eq!(ask(&f.socket, "start").await?, "engine:start");
        // A client that holds its connection.
        let held = UnixStream::connect(&f.socket).await?;
        until("the held client to be counted", || {
            supervisor.connections.load(Ordering::SeqCst) == 1
        })
        .await?;
        assert!(!supervisor.stop_if_idle().await);
        drop(held);
        until("the held client to be counted out", || {
            supervisor.connections.load(Ordering::SeqCst) == 0
        })
        .await?;
        // A container: a process in a group below the engine's.
        std::fs::write(f.path("cg/docker/c1/cgroup.procs"), "4242\n")?;
        assert!(!supervisor.stop_if_idle().await);
        assert!(supervisor.running().await);
        std::fs::write(f.path("cg/docker/c1/cgroup.procs"), "")?;
        assert!(supervisor.stop_if_idle().await);
        server.abort();

        let recent = f.supervisor(f.fake(), Duration::from_secs(300), ok());
        drop(recent.connect().await.map_err(io::Error::other)?);
        assert!(!recent.stop_if_idle().await, "used within the idle time");
        recent.stop(Duration::from_secs(10)).await;
        Ok(())
    }

    /// Review of SPEC-187 (high): the engine can write its control directory (any container
    /// may bind it), while the holder sees the whole host. A link planted at the control socket
    /// must never lead the holder to another socket of the host.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_link_at_the_control_socket_is_never_followed() -> io::Result<()> {
        let f = Fixture::new()?;
        let decoy_path = f.path("host-secret.sock");
        let decoy = UnixListener::bind(&decoy_path)?;
        let command = vec![
            "/bin/sh".into(),
            "-c".into(),
            format!(
                "ln -sf {} {}; exec sleep 30",
                decoy_path.display(),
                f.path("control.sock").display()
            ),
        ];
        let supervisor = Supervisor::with_start_timeout(
            command,
            f.path("control.sock"),
            f.path("engine.log"),
            f.path("cg"),
            Duration::from_secs(300),
            ok(),
            Duration::from_millis(800),
        );
        let reason = supervisor.connect().await.err().unwrap_or_default();
        assert!(reason.contains("did not answer"), "{reason}");
        let reached = tokio::time::timeout(Duration::from_millis(300), decoy.accept()).await;
        assert!(
            reached.is_err(),
            "the holder followed the link to the decoy"
        );
        // A real socket at the same place is reached.
        let _ = std::fs::remove_file(f.path("control.sock"));
        let real = UnixListener::bind(f.path("control.sock"))?;
        let stream = connect_control(&f.path("control.sock")).await?;
        drop(stream);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), real.accept())
                .await
                .is_ok(),
            "the real control socket is reached"
        );
        Ok(())
    }

    #[test]
    fn only_processes_below_the_engines_group_are_containers() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let cg = dir.path();
        // The engine itself lives in the group: not a container.
        std::fs::write(cg.join("cgroup.procs"), "1\n2\n")?;
        assert!(!containers_running(cg));
        std::fs::create_dir_all(cg.join("docker/a/nested"))?;
        std::fs::write(cg.join("docker/cgroup.procs"), "")?;
        std::fs::write(cg.join("docker/a/cgroup.procs"), "")?;
        std::fs::write(cg.join("docker/a/nested/cgroup.procs"), "")?;
        assert!(!containers_running(cg));
        std::fs::write(cg.join("docker/a/nested/cgroup.procs"), "77\n")?;
        assert!(containers_running(cg));
        assert!(!containers_running(&cg.join("missing")));
        Ok(())
    }
}
