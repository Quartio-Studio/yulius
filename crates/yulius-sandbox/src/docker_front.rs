//! The sandbox's Docker front (ADR-0158, SPEC-196 DKL-002).
//!
//! `yulius sandbox-host docker --spec <state>/docker.json` ([`run`]) joins the sandbox's `system`
//! group and its holder's user, network and mount namespaces, binds `<state>/docker/docker.sock`
//! and prints `ready` (or why not) on stdout. It is started by the server for every live holder,
//! whatever the holder's age, and lives detached from the server like the holder. It serves the
//! socket in one of the modes of [`DockerServe`]:
//!
//! - **engine**: the sandbox's engine on demand ([`crate::engine_supervisor`]; the engine is its
//!   child, in the holder's namespaces as with SPEC-187);
//! - **relay**: bytes to the socket a SPEC-187 holder serves itself;
//! - **refuse**: a Docker-API error naming why there is no engine and the operator's fix.
//!
//! It stops (its engine first) on `SIGTERM`, when its holder is gone, and when its socket was
//! replaced (a newer front serves the directory). Runs see the directory, not the socket file,
//! so a socket recreated by the next front reaches them.

use std::io;
use std::net::Ipv4Addr;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use yulius_contracts::sandbox::{
    DOCKER_FRONT_RECORD_FILE, DOCKER_FRONT_SPEC_FILE, DockerFrontRecord, DockerFrontSpec,
    DockerServe, ENGINE_SOCKET,
};

use crate::engine_supervisor::{Prepare, Supervisor};
use crate::holder::StatePaths;

/// The front's readiness line.
pub const READY: &str = "ready";
/// How often the front looks whether its holder and its socket are still there.
pub const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Grace for the engine when the front stops.
pub const ENGINE_STOP_GRACE: Duration = Duration::from_secs(2);
/// Grace after `SIGTERM` before the server kills a front.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// The engine's default bridge gateway (`host-gateway` of containers): where a pre-engine
/// holder's forwards are served for containers.
pub const BRIDGE_GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 17, 0, 1);

impl StatePaths {
    /// `docker/`, the sandbox's Docker directory, bound into runs (ADR-0158).
    #[must_use]
    pub fn docker_dir(&self) -> PathBuf {
        self.dir.join(yulius_contracts::sandbox::DOCKER_DIR)
    }
    /// `docker/docker.sock`, the socket the front serves.
    #[must_use]
    pub fn docker_socket(&self) -> PathBuf {
        self.docker_dir().join(ENGINE_SOCKET)
    }
    /// `docker.json`.
    #[must_use]
    pub fn docker_spec(&self) -> PathBuf {
        self.dir.join(DOCKER_FRONT_SPEC_FILE)
    }
    /// `docker-front.json`.
    #[must_use]
    pub fn docker_record(&self) -> PathBuf {
        self.dir.join(DOCKER_FRONT_RECORD_FILE)
    }
    /// `docker-front.log`.
    #[must_use]
    pub fn docker_log(&self) -> PathBuf {
        self.dir
            .join(yulius_contracts::sandbox::DOCKER_FRONT_LOG_FILE)
    }
}

/// A serving front.
pub struct Front {
    socket: PathBuf,
    inode: u64,
    supervisor: Option<Arc<Supervisor>>,
    tasks: Vec<JoinHandle<()>>,
    bridge_forwards: Vec<u16>,
    outbound: PathBuf,
}

impl Front {
    /// Binds `paths`' Docker socket (replacing a stale one) and serves it as `serve` says.
    /// `prepare` runs before each engine start (the holder's sysfs and forwarding).
    ///
    /// # Errors
    /// The socket cannot be bound.
    pub fn start(
        paths: &StatePaths,
        serve: &DockerServe,
        bridge_forwards: &[u16],
        prepare: Prepare,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(paths.docker_dir())?;
        let socket = paths.docker_socket();
        let _ = std::fs::remove_file(&socket);
        // Created 0660 by the umask, never chmod'ed by name: the engine (and a container binding
        // the directory) can write it, and could put a link there between bind and chmod.
        let mask = nix::sys::stat::Mode::from_bits_truncate(0o117);
        let previous = nix::sys::stat::umask(mask);
        let listener = UnixListener::bind(&socket);
        nix::sys::stat::umask(previous);
        let listener = listener?;
        let inode = std::fs::symlink_metadata(&socket)?.ino();
        let mut tasks = Vec::new();
        let mut supervisor = None;
        match serve {
            DockerServe::Engine { engine } => {
                for dir in [paths.engine_control(), paths.engine_sysfs()] {
                    std::fs::create_dir_all(dir)?;
                }
                let engine = Supervisor::new(
                    engine.command.clone(),
                    paths.engine_control().join(ENGINE_SOCKET),
                    paths.engine_log(),
                    PathBuf::from(&engine.cgroup),
                    Duration::from_secs(engine.idle_secs),
                    prepare,
                );
                tasks.push(tokio::spawn(Arc::clone(&engine).serve(listener)));
                tasks.push(tokio::spawn(Arc::clone(&engine).watch_idle()));
                supervisor = Some(engine);
            }
            DockerServe::Relay { socket } => {
                tasks.push(tokio::spawn(relay(listener, PathBuf::from(socket))));
            }
            DockerServe::Refuse { reason } => {
                tasks.push(tokio::spawn(refuse_all(listener, reason.clone())));
            }
        }
        Ok(Self {
            socket,
            inode,
            supervisor,
            tasks,
            bridge_forwards: bridge_forwards.to_vec(),
            outbound: paths.outbound(),
        })
    }

    /// Whether another process replaced (or removed) the socket this front bound.
    #[must_use]
    pub fn replaced(&self) -> bool {
        std::fs::symlink_metadata(&self.socket).map_or(true, |m| m.ino() != self.inode)
    }

    /// Serves the holder's loopback-only forwards on the engine's bridge gateway once the engine
    /// has made it (each port once).
    async fn serve_bridge_forwards(&mut self) {
        let Some(engine) = &self.supervisor else {
            return;
        };
        if self.bridge_forwards.is_empty() || !engine.running().await {
            return;
        }
        let mut left = Vec::new();
        for port in std::mem::take(&mut self.bridge_forwards) {
            match crate::relay::serve_forward_at(BRIDGE_GATEWAY, port, self.outbound.clone(), false)
                .await
            {
                Ok(task) => self.tasks.push(task),
                Err(_) => left.push(port),
            }
        }
        self.bridge_forwards = left;
    }

    /// Stops serving; an engine gets `grace` to stop.
    pub async fn stop(self, grace: Duration) {
        for task in &self.tasks {
            task.abort();
        }
        if let Some(engine) = &self.supervisor {
            engine.stop(grace).await;
        }
        if !self.replaced() {
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

async fn relay(listener: UnixListener, target: PathBuf) {
    let limit = Arc::new(Semaphore::new(crate::relay::MAX_CONNECTIONS));
    loop {
        let Ok((client, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
            continue;
        };
        let target = target.clone();
        tokio::spawn(async move {
            match UnixStream::connect(&target).await {
                Ok(engine) => crate::relay::splice(client, engine).await,
                Err(error) => {
                    let reason =
                        format!("the sandbox holder's Docker socket does not answer: {error}");
                    crate::engine_supervisor::refuse(client, &reason).await;
                }
            }
            drop(permit);
        });
    }
}

async fn refuse_all(listener: UnixListener, reason: String) {
    let limit = Arc::new(Semaphore::new(crate::relay::MAX_CONNECTIONS));
    loop {
        let Ok((client, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = Arc::clone(&limit).try_acquire_owned() else {
            continue;
        };
        let reason = reason.clone();
        tokio::spawn(async move {
            crate::engine_supervisor::refuse(client, &reason).await;
            drop(permit);
        });
    }
}

/// Serves `front` until `SIGTERM`/`SIGINT`, until `holder_alive` says no, or until its socket is
/// replaced, then stops it.
pub async fn serve_until_done(mut front: Front, holder_alive: impl Fn() -> bool) {
    let mut watch = tokio::time::interval(WATCH_EVERY);
    let terminated = crate::holder::wait_for_termination();
    tokio::pin!(terminated);
    loop {
        tokio::select! {
            () = &mut terminated => break,
            _ = watch.tick() => {
                if front.replaced() || !holder_alive() {
                    break;
                }
                front.serve_bridge_forwards().await;
            }
        }
    }
    front.stop(ENGINE_STOP_GRACE).await;
}

/// Body of `yulius sandbox-host docker`: see the module docs.
///
/// # Errors
/// Setup failed; the message also goes to stdout instead of `ready`.
pub fn run(spec_path: &Path) -> Result<(), String> {
    let result = setup(spec_path);
    let (spec, runtime) = match result {
        Ok(parts) => parts,
        Err(message) => {
            println!("{message}");
            return Err(message);
        }
    };
    let paths = StatePaths::new(PathBuf::from(&spec.state_dir));
    runtime.block_on(async move {
        let (state, sysfs) = (paths.dir.clone(), paths.engine_sysfs());
        let prepare: Prepare = Box::new(move || prepare_engine(&state, &sysfs));
        let front = match Front::start(&paths, &spec.serve, &spec.bridge_forwards, prepare) {
            Ok(front) => front,
            Err(error) => {
                let message = format!("docker.sock: {error}");
                println!("{message}");
                return Err(message);
            }
        };
        println!("{READY}");
        let (pid, net_ns) = (spec.holder_pid, spec.net_ns);
        serve_until_done(front, move || {
            crate::holder::ns_inode(pid, "net") == Some(net_ns)
        })
        .await;
        Ok(())
    })
}

fn setup(spec_path: &Path) -> Result<(DockerFrontSpec, tokio::runtime::Runtime), String> {
    let spec: DockerFrontSpec = crate::holder::read_json(spec_path)
        .map_err(|e| format!("docker front spec {}: {e}", spec_path.display()))?;
    spec.validate().map_err(|e| e.to_string())?;
    if let Some(cgroup) = &spec.cgroup {
        crate::cgroup::join(Path::new(cgroup)).map_err(|e| format!("join {cgroup}: {e}"))?;
    }
    // Still single-threaded: `setns` into a user namespace needs it.
    crate::enter::join_holder(spec.holder_pid, spec.user_ns, spec.net_ns, spec.mnt_ns)
        .map_err(|e| format!("join the sandbox's namespaces: {e}"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    Ok((spec, runtime))
}

/// Before each engine start: a fresh read-only sysfs of the sandbox's network at `sysfs` (the
/// engine's `/sys`; `runc` may mount a sysfs only where one is fully visible, and the host's
/// carries the host's cgroup tree), and IPv4 forwarding in the sandbox's network, which
/// containers' traffic crosses.
#[cfg(target_os = "linux")]
fn prepare_engine(state: &Path, sysfs: &Path) -> Result<(), String> {
    use nix::mount::{MsFlags, mount};
    let device = |path: &Path| std::fs::metadata(path).map(|m| m.dev()).ok();
    if device(sysfs).is_none() || device(sysfs) == device(state) {
        mount(
            Some("sysfs"),
            sysfs,
            Some("sysfs"),
            MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
            None::<&str>,
        )
        .map_err(|e| format!("cannot mount the engine's sysfs: {e}"))?;
    }
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1")
        .map_err(|e| format!("cannot turn on forwarding in the sandbox's network: {e}"))
}

#[cfg(not(target_os = "linux"))]
fn prepare_engine(_state: &Path, _sysfs: &Path) -> Result<(), String> {
    Err("sandbox namespaces are Linux-only".into())
}

/// Whether `pid` is still the front of the holder whose network namespace is `net_ns`.
#[must_use]
pub fn is_front(pid: u32, net_ns: u64) -> bool {
    crate::holder::exists(pid)
        && crate::holder::command_line(pid).contains("sandbox-host docker")
        && crate::holder::ns_inode(pid, "net") == Some(net_ns)
}

/// Stops the front recorded in `paths` (`SIGTERM`, then `SIGKILL` after [`STOP_GRACE`]) while
/// it is still the front of the holder with `net_ns`, and removes its record.
pub fn stop(paths: &StatePaths, net_ns: u64) {
    if let Ok(record) = crate::holder::read_json::<DockerFrontRecord>(&paths.docker_record())
        && is_front(record.pid, net_ns)
    {
        crate::holder::signal(record.pid, nix::sys::signal::Signal::SIGTERM);
        let deadline = std::time::Instant::now() + STOP_GRACE;
        while crate::holder::exists(record.pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if is_front(record.pid, net_ns) {
            crate::holder::signal(record.pid, nix::sys::signal::Signal::SIGKILL);
        }
    }
    let _ = std::fs::remove_file(paths.docker_record());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn ok() -> Prepare {
        Box::new(|| Ok(()))
    }

    async fn ask(socket: &Path, text: &str) -> io::Result<String> {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_all(text.as_bytes()).await?;
        let mut out = String::new();
        tokio::time::timeout(Duration::from_secs(20), stream.read_to_string(&mut out))
            .await
            .map_err(|_| io::Error::other("no answer"))??;
        Ok(out)
    }

    /// The socket is 0660, and a link planted in the directory (which the engine can write) is
    /// replaced, never followed.
    #[tokio::test]
    async fn the_socket_replaces_a_planted_link_and_is_group_only() -> io::Result<()> {
        use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        std::fs::create_dir_all(paths.docker_dir())?;
        let target = dir.path().join("secret");
        std::fs::write(&target, "secret")?;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))?;
        std::os::unix::fs::symlink(&target, paths.docker_socket())?;
        let front = Front::start(
            &paths,
            &DockerServe::Refuse { reason: "r".into() },
            &[],
            ok(),
        )?;
        let meta = std::fs::symlink_metadata(paths.docker_socket())?;
        assert!(meta.file_type().is_socket());
        assert_eq!(meta.permissions().mode() & 0o777, 0o660);
        assert_eq!(
            std::fs::metadata(&target)?.permissions().mode() & 0o777,
            0o600
        );
        front.stop(Duration::from_secs(1)).await;
        Ok(())
    }

    /// The process umask, read without changing it.
    fn process_umask() -> io::Result<String> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        status
            .lines()
            .find_map(|line| line.strip_prefix("Umask:"))
            .map(|mask| mask.trim().to_owned())
            .ok_or_else(|| io::Error::other("no Umask in /proc/self/status"))
    }

    /// SPEC-304 (#825): fronts started at once in one process (as unit tests are) each bind a
    /// 0660 socket, and the process umask is never left changed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fronts_started_at_once_bind_group_only_sockets_and_keep_the_umask() -> io::Result<()>
    {
        use std::os::unix::fs::PermissionsExt as _;
        const FRONTS: usize = 8;
        let before = process_umask()?;
        for _ in 0..25 {
            let dirs = (0..FRONTS)
                .map(|_| tempfile::tempdir())
                .collect::<io::Result<Vec<_>>>()?;
            let barrier = Arc::new(std::sync::Barrier::new(FRONTS));
            let starts = dirs.iter().map(|dir| {
                let paths = StatePaths::new(dir.path().to_path_buf());
                let barrier = Arc::clone(&barrier);
                tokio::task::spawn_blocking(move || {
                    barrier.wait();
                    let serve = DockerServe::Refuse { reason: "r".into() };
                    Front::start(&paths, &serve, &[], ok())
                })
            });
            let mut fronts = Vec::new();
            for start in starts.collect::<Vec<_>>() {
                fronts.push(start.await.map_err(io::Error::other)??);
            }
            for dir in &dirs {
                let socket = StatePaths::new(dir.path().to_path_buf()).docker_socket();
                let mode = std::fs::symlink_metadata(&socket)?.permissions().mode() & 0o777;
                assert_eq!(mode, 0o660, "{}", socket.display());
            }
            for front in fronts {
                front.stop(Duration::from_secs(1)).await;
            }
        }
        assert_eq!(process_umask()?, before);
        Ok(())
    }

    /// SPEC-196 DKL-002/005: without a possible engine, `docker` gets a Docker-API error with
    /// the reason and the operator's fix, never a missing socket.
    #[tokio::test]
    async fn a_refusing_front_names_the_reason_in_a_docker_error() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let reason = "the server has no cgroup delegation; an operator sets Delegate=yes";
        let front = Front::start(
            &paths,
            &DockerServe::Refuse {
                reason: reason.into(),
            },
            &[],
            ok(),
        )?;
        let answer = ask(
            &paths.docker_socket(),
            "GET /_ping HTTP/1.1\r\nHost: docker\r\n\r\n",
        )
        .await?;
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        let body = answer.split("\r\n\r\n").nth(1).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(body).map_err(io::Error::other)?;
        let message = json["message"].as_str().unwrap_or_default();
        assert!(message.contains(reason), "{message}");
        assert!(message.contains("yulius doctor"), "{message}");
        front.stop(Duration::from_secs(1)).await;
        assert!(!paths.docker_socket().exists());
        Ok(())
    }

    /// SPEC-196 DKL-002: in engine mode the first connection starts the engine (prepared first)
    /// and reaches it; stopping the front stops the engine.
    #[tokio::test]
    async fn an_engine_front_starts_its_engine_on_first_use() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let control = paths.engine_control().join(ENGINE_SOCKET);
        let fake = format!(
            "import os, socket, sys\ns = socket.socket(socket.AF_UNIX)\ns.bind({:?})\ns.listen(8)\n\
             while True:\n    c, _ = s.accept()\n    c.sendall(b'engine:' + c.recv(1024))\n    c.close()\n",
            control.display().to_string()
        );
        std::fs::create_dir_all(dir.path().join("cg"))?;
        let engine = yulius_contracts::sandbox::EngineSpec {
            command: vec![
                "/usr/bin/env".into(),
                "python3".into(),
                "-I".into(),
                "-c".into(),
                fake,
            ],
            cgroup: dir.path().join("cg").to_string_lossy().into_owned(),
            idle_secs: 300,
        };
        let prepared = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&prepared);
        let front = Front::start(
            &paths,
            &DockerServe::Engine { engine },
            &[],
            Box::new(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
        )?;
        assert_eq!(ask(&paths.docker_socket(), "ping").await?, "engine:ping");
        assert_eq!(prepared.load(std::sync::atomic::Ordering::SeqCst), 1);
        front.stop(Duration::from_millis(300)).await;
        assert!(!paths.docker_socket().exists());
        Ok(())
    }

    /// SPEC-196 DKL-002: a front of a SPEC-187 holder passes bytes to the holder's own socket.
    #[tokio::test]
    async fn a_relaying_front_reaches_the_holders_engine_byte_for_byte() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let old = UnixListener::bind(paths.engine_socket())?;
        let engine = tokio::spawn(async move {
            while let Ok((mut stream, _)) = old.accept().await {
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let _ = stream.write_all(b"old-engine:").await;
                let _ = stream.write_all(&buf[..n]).await;
            }
        });
        let front = Front::start(
            &paths,
            &DockerServe::Relay {
                socket: paths.engine_socket().to_string_lossy().into_owned(),
            },
            &[],
            ok(),
        )?;
        assert_eq!(
            ask(&paths.docker_socket(), "ping").await?,
            "old-engine:ping"
        );
        drop(engine);
        front.stop(Duration::from_secs(1)).await;
        Ok(())
    }

    /// SPEC-196 DKL-001/002: a newer front binding the directory replaces the socket; the older
    /// one notices and stops, and clients reach the newer one at the same path.
    #[tokio::test]
    async fn a_front_whose_socket_was_replaced_stops() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let refuse = |reason: &str| DockerServe::Refuse {
            reason: reason.into(),
        };
        let first = Front::start(&paths, &refuse("first"), &[], ok())?;
        assert!(!first.replaced());
        let second = Front::start(&paths, &refuse("second"), &[], ok())?;
        assert!(first.replaced());
        let done = tokio::spawn(serve_until_done(first, || true));
        tokio::time::timeout(WATCH_EVERY * 3, done)
            .await
            .map_err(|_| io::Error::other("the replaced front did not stop"))?
            .map_err(io::Error::other)?;
        let answer = ask(&paths.docker_socket(), "GET / HTTP/1.1\r\n\r\n").await?;
        assert!(answer.contains("second"), "{answer}");
        assert!(
            !second.replaced(),
            "the old front left the new socket alone"
        );
        second.stop(Duration::from_secs(1)).await;
        Ok(())
    }

    /// SPEC-196 DKL-002: a front stops when its holder is gone (it would keep the sandbox's
    /// namespaces alive otherwise).
    #[tokio::test]
    async fn a_front_stops_with_its_holder() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let paths = StatePaths::new(dir.path().to_path_buf());
        let front = Front::start(
            &paths,
            &DockerServe::Refuse { reason: "r".into() },
            &[],
            ok(),
        )?;
        tokio::time::timeout(WATCH_EVERY * 3, serve_until_done(front, || false))
            .await
            .map_err(|_| io::Error::other("the front outlived its holder"))?;
        assert!(!paths.docker_socket().exists());
        Ok(())
    }
}
