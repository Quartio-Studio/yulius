//! The per-sandbox namespace holder (SPEC-002 ADR-22 §1).
//!
//! `yulius sandbox-host start --spec <state>/holder.json` ([`start`]) creates the namespaces
//! with `bwrap` and the subordinate id map ([`crate::userns`]), waits until the holder inside is
//! ready, attaches `slirp4netns` for egress, writes `host.json` and exits. The holder processes
//! stay, detached from the server:
//!
//! - `bwrap` (the namespace creator; it exits when its child does);
//! - `yulius sandbox-host inner` ([`inner`]) in the new user, network and mount namespaces, as
//!   root of the sandbox with every capability there. It mounts the system layer, serves
//!   `in.sock`, the forwarded ports and, when the sandbox has an engine, its Docker socket
//!   ([`crate::engine_supervisor`]; the engine is its child), and waits for `SIGTERM`;
//! - `slirp4netns`, when the sandbox has egress.
//!
//! Runs join the namespaces through `/proc/<inner pid>/ns/*` after checking the recorded inodes
//! ([`crate::enter`]); the server stops the holder ([`stop`]) when the sandbox has been idle.

use std::ffi::OsString;
use std::io::{self, Read as _, Write as _};
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use yulius_contracts::sandbox::{
    ENGINE_CONTROL_DIR, ENGINE_LOG_FILE, ENGINE_SOCKET, ENGINE_SYSFS_DIR, EngineSpec,
    HOLDER_LOG_FILE, HOLDER_RECORD_FILE, HOLDER_RECORD_VERSION, HolderRecord, HolderSpec,
    INBOUND_SOCKET, OUTBOUND_SOCKET, SYSTEM_ROOT_DIR,
};

use crate::engine_supervisor::Supervisor;

use crate::userns::{Mapper, cloexec_pipe, inheritable};

/// Time the holder has to mount its system layer and serve its sockets.
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Time `slirp4netns` has to configure the network.
pub const SLIRP_TIMEOUT: Duration = Duration::from_secs(10);
/// Grace after `SIGTERM` before `SIGKILL` when stopping a holder.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// Largest record read.
const MAX_RECORD_BYTES: u64 = 1 << 20;
/// The holder's readiness report when it serves everything asked of it.
const READY: &str = "ready";
/// The report when it serves all but the sandbox's Docker socket (the sandbox still works).
const READY_WITHOUT_DOCKER: &str = "ready without docker.sock";

/// Whether a readiness report means ready, and whether the Docker socket is served then.
fn readiness(line: &str) -> Option<bool> {
    match line {
        READY => Some(true),
        READY_WITHOUT_DOCKER => Some(false),
        _ => None,
    }
}

/// Reads a JSON record (bounded).
///
/// # Errors
/// Unreadable, too large or not the expected JSON.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    read_json_limited(path, MAX_RECORD_BYTES)
}

/// Reads a JSON file of at most `limit` bytes.
///
/// # Errors
/// Unreadable, too large or not the expected JSON.
pub fn read_json_limited<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> io::Result<T> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("record too large"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::from)
}

/// Writes `value` as JSON to `path` atomically (0600).
///
/// # Errors
/// Any write, sync or rename failure.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// The boot this process runs in.
#[must_use]
pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "unknown".into())
}

/// Inode of `/proc/<pid>/ns/<kind>`, when readable.
#[must_use]
pub fn ns_inode(pid: u32, kind: &str) -> Option<u64> {
    std::fs::metadata(format!("/proc/{pid}/ns/{kind}"))
        .ok()
        .map(|m| m.ino())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Whether the holder of `record` still runs in this boot with the recorded namespaces.
#[must_use]
pub fn alive(record: &HolderRecord) -> bool {
    record.boot_id == boot_id()
        && ns_inode(record.inner_pid, "user") == Some(record.user_ns)
        && ns_inode(record.inner_pid, "net") == Some(record.net_ns)
        && ns_inode(record.inner_pid, "mnt") == Some(record.mnt_ns)
}

/// The state directory's paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatePaths {
    /// The directory.
    pub dir: PathBuf,
}

impl StatePaths {
    /// Paths under `dir`.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    /// `holder.json`.
    #[must_use]
    pub fn spec(&self) -> PathBuf {
        self.dir.join(yulius_contracts::sandbox::HOLDER_SPEC_FILE)
    }
    /// `host.json`.
    #[must_use]
    pub fn record(&self) -> PathBuf {
        self.dir.join(HOLDER_RECORD_FILE)
    }
    /// `holder.log`.
    #[must_use]
    pub fn log(&self) -> PathBuf {
        self.dir.join(HOLDER_LOG_FILE)
    }
    /// `in.sock`.
    #[must_use]
    pub fn inbound(&self) -> PathBuf {
        self.dir.join(INBOUND_SOCKET)
    }
    /// `out.sock`.
    #[must_use]
    pub fn outbound(&self) -> PathBuf {
        self.dir.join(OUTBOUND_SOCKET)
    }
    /// `root/` (the merged system layer inside the holder).
    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.dir.join(SYSTEM_ROOT_DIR)
    }
    /// `docker.sock`, the sandbox's Docker socket (ADR-0151).
    #[must_use]
    pub fn engine_socket(&self) -> PathBuf {
        self.dir.join(ENGINE_SOCKET)
    }
    /// `engine/`, the engine's control directory.
    #[must_use]
    pub fn engine_control(&self) -> PathBuf {
        self.dir.join(ENGINE_CONTROL_DIR)
    }
    /// `sys/`, where the holder mounts the engine's sysfs.
    #[must_use]
    pub fn engine_sysfs(&self) -> PathBuf {
        self.dir.join(ENGINE_SYSFS_DIR)
    }
    /// `engine.log`.
    #[must_use]
    pub fn engine_log(&self) -> PathBuf {
        self.dir.join(ENGINE_LOG_FILE)
    }
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let tail: Vec<&str> = text.lines().rev().take(5).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

fn read_line_with_timeout(reader: OwnedFd, limit: Duration) -> io::Result<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(reader);
        let mut text = Vec::new();
        let mut byte = [0u8; 1];
        while text.len() < 4096 {
            match file.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => text.push(byte[0]),
                _ => break,
            }
        }
        let _ = sender.send(String::from_utf8_lossy(&text).into_owned());
    });
    receiver
        .recv_timeout(limit)
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "not ready in time"))
}

/// Body of `yulius sandbox-host start`: see the module docs. Returns the record it wrote.
///
/// # Errors
/// A message for the server's log (no secrets).
pub fn start(spec_path: &Path) -> Result<HolderRecord, String> {
    let spec: HolderSpec =
        read_json(spec_path).map_err(|e| format!("holder spec {}: {e}", spec_path.display()))?;
    spec.validate().map_err(|e| e.to_string())?;
    let paths = StatePaths::new(PathBuf::from(&spec.state_dir));
    let _ = std::fs::remove_file(paths.record());
    std::fs::create_dir_all(paths.root()).map_err(|e| format!("system root: {e}"))?;
    if let Some(cgroup) = &spec.cgroup {
        crate::cgroup::join(Path::new(cgroup)).map_err(|e| format!("join {cgroup}: {e}"))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths.log())
        .map_err(|e| format!("holder log: {e}"))?;
    let (ready_read, ready_write) = cloexec_pipe().map_err(|e| e.to_string())?;
    inheritable(&ready_write).map_err(|e| e.to_string())?;
    let args: Vec<OsString> = [
        "--unshare-net",
        "--dev-bind",
        "/",
        "/",
        "--cap-add",
        "ALL",
        "--chdir",
        "/",
        "--",
        &spec.exe,
        "sandbox-host",
        "inner",
        "--spec",
        &spec_path.to_string_lossy(),
        "--ready-fd",
        &ready_write.as_raw_fd().to_string(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let mapper = Mapper {
        bwrap: PathBuf::from(&spec.bwrap),
        newuidmap: PathBuf::from(&spec.newuidmap),
        newgidmap: PathBuf::from(&spec.newgidmap),
        uid: spec.uid,
        gid: spec.gid,
    };
    let stderr = log.try_clone().map_err(|e| e.to_string())?;
    let (mut child, inner_pid) = mapper
        .spawn(&args, Stdio::from(stderr))
        .map_err(|e| format!("namespaces: {e} ({})", log_tail(&paths.log())))?;
    drop(ready_write);
    let fail = |child: &mut std::process::Child, message: String| {
        let _ = child.kill();
        let _ = child.wait();
        Err(format!("{message} ({})", log_tail(&paths.log())))
    };
    let docker = match read_line_with_timeout(ready_read, READY_TIMEOUT) {
        Ok(line) if readiness(&line).is_some() => readiness(&line) == Some(true),
        Ok(line) => return fail(&mut child, format!("holder failed: {line}")),
        Err(e) => return fail(&mut child, format!("holder: {e}")),
    };
    let slirp_pid = match &spec.slirp4netns {
        None => None,
        Some(program) => match start_slirp(program, inner_pid, &log) {
            Ok(pid) => Some(pid),
            Err(e) => return fail(&mut child, format!("slirp4netns: {e}")),
        },
    };
    let ns = |kind| ns_inode(inner_pid, kind).ok_or_else(|| format!("namespace {kind} gone"));
    let record = HolderRecord {
        version: HOLDER_RECORD_VERSION,
        sandbox: spec.sandbox.clone(),
        base_id: spec.base_id.clone(),
        inner_pid,
        bwrap_pid: child.id(),
        slirp_pid,
        user_ns: ns("user")?,
        net_ns: ns("net")?,
        mnt_ns: ns("mnt")?,
        boot_id: boot_id(),
        started_at_ms: now_ms(),
        system_root: paths.root().to_string_lossy().into_owned(),
        engine: spec.engine.is_some() && docker,
    };
    write_json(&paths.record(), &record).map_err(|e| format!("host.json: {e}"))?;
    Ok(record)
}

fn start_slirp(program: &str, pid: u32, log: &std::fs::File) -> io::Result<u32> {
    let (ready_read, ready_write) = cloexec_pipe()?;
    inheritable(&ready_write)?;
    let mut command = Command::new(program);
    command
        .args([
            "--configure",
            "--mtu=65520",
            "--disable-host-loopback",
            "--netns-type=path",
        ])
        .arg(format!("--userns-path=/proc/{pid}/ns/user"))
        .arg(format!("--ready-fd={}", ready_write.as_raw_fd()))
        .arg(format!("/proc/{pid}/ns/net"))
        .arg("tap0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log.try_clone()?));
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let child = command.spawn()?;
    drop(ready_write);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(ready_read);
        let mut byte = [0u8; 1];
        let ok = matches!(file.read(&mut byte), Ok(1)) && byte[0] == b'1';
        let _ = sender.send(ok);
    });
    match receiver.recv_timeout(SLIRP_TIMEOUT) {
        Ok(true) => Ok(child.id()),
        _ => {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap_or(0)),
                nix::sys::signal::Signal::SIGKILL,
            );
            Err(io::Error::other("did not configure the network"))
        }
    }
}

/// Mount options of the system layer.
///
/// # Errors
/// A path contains a character overlayfs options cannot carry.
pub fn overlay_options(spec: &HolderSpec) -> Result<String, String> {
    for path in [&spec.base, &spec.upper, &spec.work] {
        if path.contains([',', ':', '\\']) {
            return Err(format!(
                "{path}: ',' ':' and '\\' cannot be used in layer paths"
            ));
        }
    }
    Ok(format!(
        "lowerdir={},upperdir={},workdir={},userxattr",
        spec.base, spec.upper, spec.work
    ))
}

/// `/etc/hosts` of a sandbox.
#[must_use]
pub fn hosts_file(hostname: &str) -> String {
    format!(
        "127.0.0.1\tlocalhost\n127.0.1.1\t{hostname}\n::1\tlocalhost ip6-localhost ip6-loopback\nff02::1\tip6-allnodes\nff02::2\tip6-allrouters\n"
    )
}

fn write_if_changed(path: &Path, text: &str) {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        // Through the overlay: lands in the sandbox's upper layer.
        let _ = std::fs::remove_file(path);
        let _ = std::fs::write(path, text);
    }
}

/// Body of `yulius sandbox-host inner` (inside the namespaces): see the module docs.
///
/// # Errors
/// Setup failed; the message also goes to the ready pipe.
pub fn inner(spec_path: &Path, ready_fd: i32) -> Result<(), String> {
    // The descriptor number was handed to us by `start` across exec.
    let ready = owned_fd(ready_fd)?;
    let mut ready = std::fs::File::from(ready);
    let result = inner_setup(spec_path);
    let (spec, runtime) = match result {
        Ok(parts) => parts,
        Err(message) => {
            let _ = writeln!(ready, "{message}");
            return Err(message);
        }
    };
    let paths = StatePaths::new(PathBuf::from(&spec.state_dir));
    runtime.block_on(async move {
        let _ = std::fs::remove_file(paths.inbound());
        let listener = match tokio::net::UnixListener::bind(paths.inbound()) {
            Ok(listener) => listener,
            Err(e) => {
                let _ = writeln!(ready, "in.sock: {e}");
                return Err(format!("in.sock: {e}"));
            }
        };
        let (first, last) = (spec.port_first, spec.port_last);
        let ports: Arc<dyn Fn() -> Vec<u16> + Send + Sync> =
            Arc::new(move || crate::procnet::listening_ports(first, last));
        tokio::spawn(crate::relay::serve_inbound(listener, first, last, ports));
        let containers = spec.engine.is_some();
        for port in &spec.forwards {
            if let Err(e) = crate::relay::serve_forward(*port, paths.outbound(), containers).await {
                let _ = writeln!(ready, "forward {port}: {e}");
                return Err(format!("forward {port}: {e}"));
            }
        }
        // Without its Docker socket the sandbox still works: runs get no socket (`host.json`
        // says so) and the error stays in the holder's log.
        let engine = spec.engine.as_ref().and_then(|engine| {
            serve_engine(engine, &paths)
                .map_err(|e| eprintln!("yulius: no Docker socket in this sandbox: {e}"))
                .ok()
        });
        let report = if spec.engine.is_some() && engine.is_none() {
            READY_WITHOUT_DOCKER
        } else {
            READY
        };
        let _ = writeln!(ready, "{report}");
        drop(ready);
        wait_for_termination().await;
        if let Some(engine) = engine {
            // `stop` kills the holder after STOP_GRACE; the engine dies with it anyway.
            engine.stop(Duration::from_secs(2)).await;
        }
        Ok(())
    })
}

/// Serves the sandbox's Docker socket (runs and the engine bind it, so it exists before the
/// holder is ready); the engine starts on its first connection.
fn serve_engine(engine: &EngineSpec, paths: &StatePaths) -> io::Result<Arc<Supervisor>> {
    use std::os::unix::fs::PermissionsExt as _;
    for dir in [paths.engine_control(), paths.engine_sysfs()] {
        std::fs::create_dir_all(dir)?;
    }
    let socket = paths.engine_socket();
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))?;
    let (state, sysfs) = (paths.dir.clone(), paths.engine_sysfs());
    let supervisor = Supervisor::new(
        engine.command.clone(),
        paths.engine_control().join(ENGINE_SOCKET),
        paths.engine_log(),
        PathBuf::from(&engine.cgroup),
        Duration::from_secs(engine.idle_secs),
        Box::new(move || prepare_engine(&state, &sysfs)),
    );
    tokio::spawn(Arc::clone(&supervisor).serve(listener));
    tokio::spawn(Arc::clone(&supervisor).watch_idle());
    Ok(supervisor)
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

fn owned_fd(fd: i32) -> Result<OwnedFd, String> {
    if fd < 3 {
        return Err("invalid ready descriptor".into());
    }
    let path = format!("/proc/self/fd/{fd}");
    if std::fs::symlink_metadata(&path).is_err() {
        return Err("ready descriptor is not open".into());
    }
    // Reopen through /proc instead of adopting the raw number (no unsafe code): a pipe's
    // /proc/self/fd entry opens the same pipe.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map(OwnedFd::from)
        .map_err(|e| format!("ready descriptor: {e}"))
}

fn inner_setup(spec_path: &Path) -> Result<(HolderSpec, tokio::runtime::Runtime), String> {
    let spec: HolderSpec =
        read_json(spec_path).map_err(|e| format!("holder spec {}: {e}", spec_path.display()))?;
    spec.validate().map_err(|e| e.to_string())?;
    let paths = StatePaths::new(PathBuf::from(&spec.state_dir));
    mount_system_layer(&spec, &paths.root())?;
    let root = paths.root();
    write_if_changed(&root.join("etc/hosts"), &hosts_file(&spec.hostname));
    write_if_changed(&root.join("etc/hostname"), &format!("{}\n", spec.hostname));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    Ok((spec, runtime))
}

#[cfg(target_os = "linux")]
fn mount_system_layer(spec: &HolderSpec, target: &Path) -> Result<(), String> {
    use nix::mount::{MsFlags, mount};
    let options = overlay_options(spec)?;
    mount(
        Some("overlay"),
        target,
        Some("overlay"),
        MsFlags::MS_NODEV,
        Some(options.as_str()),
    )
    .map_err(|e| format!("mount the system layer: {e}"))
}

#[cfg(not(target_os = "linux"))]
fn mount_system_layer(_spec: &HolderSpec, _target: &Path) -> Result<(), String> {
    Err("sandbox namespaces are Linux-only".into())
}

async fn wait_for_termination() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

fn signal(pid: u32, signal: nix::sys::signal::Signal) {
    if let Ok(raw) = i32::try_from(pid)
        && raw > 1
    {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), signal);
    }
}

fn exists(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|raw| {
        raw > 1
            && nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None).is_ok()
            && !std::fs::read_to_string(format!("/proc/{raw}/stat"))
                .ok()
                .and_then(|s| {
                    s.rsplit_once(')')
                        .map(|(_, rest)| rest.trim_start().starts_with('Z'))
                })
                .unwrap_or(false)
    })
}

fn command_line(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

/// Stops the holder of `record` (`SIGTERM`, then `SIGKILL` after [`STOP_GRACE`]) and its
/// `slirp4netns`, and removes `host.json`. Processes are signalled only while they are still
/// the recorded ones (namespace inodes, command lines).
pub fn stop(state: &StatePaths, record: &HolderRecord) {
    let holder_ours = alive(record);
    if holder_ours {
        signal(record.inner_pid, nix::sys::signal::Signal::SIGTERM);
    }
    let deadline = Instant::now() + STOP_GRACE;
    while holder_ours && exists(record.inner_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if holder_ours && exists(record.inner_pid) && alive(record) {
        signal(record.inner_pid, nix::sys::signal::Signal::SIGKILL);
    }
    if let Some(slirp) = record.slirp_pid
        && command_line(slirp).contains("slirp4netns")
        && command_line(slirp).contains(&format!("/proc/{}/ns/net", record.inner_pid))
    {
        signal(slirp, nix::sys::signal::Signal::SIGKILL);
    }
    if command_line(record.bwrap_pid).contains("sandbox-host inner") {
        signal(record.bwrap_pid, nix::sys::signal::Signal::SIGKILL);
    }
    let _ = std::fs::remove_file(state.record());
    let _ = std::fs::remove_file(state.inbound());
    let _ = std::fs::remove_file(state.engine_socket());
}

/// `slirp4netns` of `record` still runs (egress works).
#[must_use]
pub fn egress_alive(record: &HolderRecord) -> bool {
    record.slirp_pid.is_none_or(|pid| {
        exists(pid) && command_line(pid).contains(&format!("/proc/{}/ns/net", record.inner_pid))
    })
}

/// Reattaches `slirp4netns` to a live holder whose egress process died, in `cgroup` (the
/// sandbox's `system` group) when there is one.
///
/// # Errors
/// It cannot be started.
pub fn restart_egress(
    program: &str,
    state: &StatePaths,
    record: &mut HolderRecord,
    cgroup: Option<&Path>,
) -> Result<(), String> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(state.log())
        .map_err(|e| e.to_string())?;
    let pid = start_slirp(program, record.inner_pid, &log).map_err(|e| e.to_string())?;
    // Started by the server, it would otherwise stay in the server's group, outside the limits.
    if let Some(cgroup) = cgroup
        && let Err(error) = crate::cgroup::move_pid(cgroup, pid)
    {
        tracing::warn!(cgroup = %cgroup.display(), %error, "egress runs outside the sandbox's group");
    }
    record.slirp_pid = Some(pid);
    write_json(&state.record(), record).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review of SPEC-187 (low): whether the holder serves the Docker socket comes from the
    /// holder's own readiness report, never from a socket file a crashed holder may have left.
    #[test]
    fn the_readiness_line_says_whether_the_docker_socket_is_served() {
        assert_eq!(readiness(READY), Some(true));
        assert_eq!(readiness(READY_WITHOUT_DOCKER), Some(false));
        assert_eq!(readiness("mount the system layer: EPERM"), None);
        assert_ne!(READY, READY_WITHOUT_DOCKER);
    }

    #[test]
    fn layer_paths_must_fit_overlay_options() {
        let mut spec = HolderSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: "s1".into(),
            hostname: "h".into(),
            state_dir: "/d/runtime/sandboxes/s1".into(),
            base: "/d/system/base-1".into(),
            base_id: "base-1".into(),
            upper: "/d/sysroots/s1/upper".into(),
            work: "/d/sysroots/s1/work".into(),
            exe: "/usr/local/bin/yulius".into(),
            bwrap: "/usr/bin/bwrap".into(),
            newuidmap: "/usr/bin/newuidmap".into(),
            newgidmap: "/usr/bin/newgidmap".into(),
            slirp4netns: None,
            uid: yulius_contracts::sandbox::IdMap {
                host: 1,
                sub_start: 100_000,
                count: 65_535,
            },
            gid: yulius_contracts::sandbox::IdMap {
                host: 1,
                sub_start: 100_000,
                count: 65_535,
            },
            cgroup: None,
            port_first: 1,
            port_last: 2,
            forwards: vec![],
            engine: None,
        };
        assert_eq!(
            overlay_options(&spec).as_deref(),
            Ok(
                "lowerdir=/d/system/base-1,upperdir=/d/sysroots/s1/upper,workdir=/d/sysroots/s1/work,userxattr"
            )
        );
        spec.upper = "/d/a,b".into();
        assert!(overlay_options(&spec).is_err());
        assert!(hosts_file("fixture").contains("127.0.1.1\tfixture\n"));
    }
}
