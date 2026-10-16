//! The sandbox's Docker engine (ADR-0151, SPEC-187).
//!
//! The server describes the engine in the holder spec
//! ([`EngineSpec`](yulius_contracts::sandbox::EngineSpec)): the `bwrap` argv of its file view
//! ([`bwrap_argv`]) and its cgroup. The holder serves the sandbox's Docker socket and starts the
//! engine on the first connection ([`crate::engine_supervisor`]). Inside, `yulius sandbox-host
//! engine` ([`supervise`]) uncovers `/proc/sys` and runs `dockerd` until it exits or the holder
//! closes its stdin.

use std::io::Read;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use yulius_contracts::sandbox::{
    ENGINE_CONTROL_DIR, ENGINE_CONTROL_IN_ENGINE, ENGINE_SOCKET, ENGINE_SOCKET_IN_RUNS,
    ENGINE_SYSFS_DIR, ROOT_LINKS, SYSTEM_LAYER_DIRS,
};

/// `PATH` inside the engine.
pub const ENGINE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// Where the engine keeps its images, containers and volumes.
pub const STORAGE_IN_ENGINE: &str = "/var/lib/docker";

/// What the engine's file view is made of (host paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineView {
    /// `bwrap`.
    pub bwrap: String,
    /// The `yulius` executable (bound read-only; it supervises `dockerd`).
    pub exe: String,
    /// Host name inside (the sandbox's slug).
    pub hostname: String,
    /// The holder's state directory (`runtime/sandboxes/<sandbox>`).
    pub state_dir: String,
    /// The merged system layer in the holder's mount namespace.
    pub system_root: String,
    /// Yulius's data directory, covered by an empty tmpfs as in runs.
    pub data_dir: String,
    /// The sandbox directory, bound at its host path as in runs.
    pub sandbox_dir: String,
    /// The engine's storage (`sysroots/<sandbox>/docker`).
    pub storage: String,
    /// The engine's cgroup (`sandboxes/<sandbox>/engine`), its cgroup namespace's root.
    pub cgroup: String,
}

/// `dockerd` and its arguments inside the engine: its control socket in the control directory
/// (the sandbox's socket at [`ENGINE_SOCKET_IN_RUNS`] is the holder's), cgroupfs (no systemd
/// inside), no IPv6 NAT (sandboxes have no IPv6 egress).
#[must_use]
pub fn dockerd_argv() -> Vec<String> {
    [
        "dockerd",
        "--host",
        &format!("unix://{ENGINE_CONTROL_IN_ENGINE}/docker.sock"),
        "--data-root",
        STORAGE_IN_ENGINE,
        "--exec-root",
        "/run/docker",
        "--pidfile",
        "/run/docker.pid",
        "--exec-opt",
        "native.cgroupdriver=cgroupfs",
        "--ip6tables=false",
        "--log-level",
        "warn",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The full `bwrap` argv that starts the engine (SPEC-187 DKR-003): new PID, IPC, UTS and cgroup
/// namespaces inside the sandbox's user, network and mount namespaces, every capability of the
/// sandbox's user namespace, a cleared environment, and a run's file view plus only the engine's
/// storage, its control directory, its sysfs and cgroup, the sandbox's Docker socket and the
/// `yulius` executable. Then `yulius sandbox-host engine -- dockerd …`.
#[must_use]
pub fn bwrap_argv(view: &EngineView) -> Vec<String> {
    let state = view.state_dir.trim_end_matches('/');
    let root = view.system_root.trim_end_matches('/');
    let mut argv: Vec<String> = [
        view.bwrap.as_str(),
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--hostname",
        &view.hostname,
        "--cap-add",
        "ALL",
        "--clearenv",
        "--setenv",
        "PATH",
        ENGINE_PATH,
        "--setenv",
        "HOME",
        "/root",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--perms",
        "1777",
        "--tmpfs",
        "/dev/shm",
        "--perms",
        "1777",
        "--tmpfs",
        "/tmp",
        "--perms",
        "0755",
        "--tmpfs",
        "/run",
        "--perms",
        "1777",
        "--dir",
        "/run/lock",
    ]
    .map(str::to_owned)
    .to_vec();
    for dir in SYSTEM_LAYER_DIRS {
        argv.extend(["--bind-try".into(), format!("{root}{dir}"), dir.to_owned()]);
    }
    for (target, link) in ROOT_LINKS {
        argv.extend(["--symlink".into(), target.into(), link.into()]);
    }
    argv.extend(["--tmpfs".into(), view.data_dir.clone()]);
    let pairs: [(&str, String, &str); 7] = [
        ("--bind", view.sandbox_dir.clone(), &view.sandbox_dir),
        ("--ro-bind", format!("{state}/{ENGINE_SYSFS_DIR}"), "/sys"),
        ("--bind", view.cgroup.clone(), "/sys/fs/cgroup"),
        ("--bind", view.storage.clone(), STORAGE_IN_ENGINE),
        (
            "--bind",
            format!("{state}/{ENGINE_CONTROL_DIR}"),
            ENGINE_CONTROL_IN_ENGINE,
        ),
        (
            "--bind",
            format!("{state}/{ENGINE_SOCKET}"),
            ENGINE_SOCKET_IN_RUNS,
        ),
        ("--ro-bind", view.exe.clone(), &view.exe),
    ];
    for (option, source, dest) in pairs {
        argv.extend([option.to_owned(), source, dest.to_owned()]);
    }
    argv.extend(["--chdir".into(), "/".into(), "--".into()]);
    argv.extend([view.exe.clone(), "sandbox-host".into(), "engine".into()]);
    argv.push("--".into());
    argv.extend(dockerd_argv());
    argv
}

/// Body of `yulius sandbox-host engine -- <command>` inside the engine's namespaces: uncovers
/// `/proc/sys` (bwrap covers it read-only; root of the sandbox's user namespace can write only its
/// own network's settings there, which `dockerd` needs), then [`supervise_with`] stdin.
///
/// # Errors
/// The command cannot be started.
pub fn supervise(argv: &[String]) -> Result<i32, String> {
    uncover_proc_sys();
    supervise_with(argv, std::io::stdin(), crate::engine_supervisor::STOP_GRACE)
}

#[cfg(target_os = "linux")]
fn uncover_proc_sys() {
    use nix::mount::{MntFlags, umount2};
    if let Err(error) = umount2("/proc/sys", MntFlags::MNT_DETACH) {
        eprintln!("yulius: /proc/sys stays read-only: {error}");
    }
}

#[cfg(not(target_os = "linux"))]
fn uncover_proc_sys() {}

/// Runs `argv` until it exits or `stop` reaches its end; then the command gets `SIGTERM` and,
/// after `grace`, `SIGKILL`. Returns its exit code (128 + the signal when a signal ended it).
///
/// # Errors
/// The command cannot be started or waited for.
pub fn supervise_with(
    argv: &[String],
    stop: impl Read + Send + 'static,
    grace: Duration,
) -> Result<i32, String> {
    let (program, args) = argv.split_first().ok_or("nothing to run")?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
    let exited = Arc::new(AtomicBool::new(false));
    let watching = Arc::clone(&exited);
    std::thread::spawn(move || {
        let mut stop = stop;
        let mut buf = [0u8; 256];
        while matches!(stop.read(&mut buf), Ok(n) if n > 0) {}
        let signal = |signal| {
            if !watching.load(Ordering::SeqCst) {
                let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal);
            }
        };
        signal(nix::sys::signal::Signal::SIGTERM);
        let deadline = Instant::now() + grace;
        while !watching.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        signal(nix::sys::signal::Signal::SIGKILL);
    });
    let status = child.wait().map_err(|e| e.to_string());
    exited.store(true, Ordering::SeqCst);
    let status = status?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

/// Bound on one prune call (a large build cache takes a while).
pub const PRUNE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Longest prune answer read.
const PRUNE_ANSWER_MAX: u64 = 16 << 20;

/// Prunes the engine behind its control socket `socket` (SPEC-187 DKR-006): the build cache,
/// then images no container uses. Containers and volumes are never touched. Returns the bytes the
/// engine reclaimed (each call's `SpaceReclaimed`). The socket is reached without following a
/// link (the engine can write its directory), and only while the engine runs: nothing here
/// starts it.
///
/// # Errors
/// Both calls failed (the engine is not reachable or refused).
pub async fn prune(socket: &std::path::Path) -> std::io::Result<u64> {
    let calls = [
        "/build/prune?all=1",
        // `{"dangling":["false"]}`: every unused image, not only untagged ones.
        "/images/prune?filters=%7B%22dangling%22%3A%5B%22false%22%5D%7D",
    ];
    let mut freed = 0u64;
    let mut last_error = None;
    let mut succeeded = false;
    for path in calls {
        match tokio::time::timeout(PRUNE_TIMEOUT, post(socket, path)).await {
            Ok(Ok(bytes)) => {
                succeeded = true;
                freed = freed.saturating_add(bytes);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => last_error = Some(std::io::Error::other(format!("{path} timed out"))),
        }
    }
    match (succeeded, last_error) {
        (false, Some(error)) => Err(error),
        _ => Ok(freed),
    }
}

/// `POST path` over HTTP/1.0 (the engine closes the connection after its answer); the
/// `SpaceReclaimed` of a 200 answer.
async fn post(socket: &std::path::Path, path: &str) -> std::io::Result<u64> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = crate::engine_supervisor::connect_control(socket).await?;
    let request = format!("POST {path} HTTP/1.0\r\nHost: docker\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut answer = Vec::new();
    stream
        .take(PRUNE_ANSWER_MAX)
        .read_to_end(&mut answer)
        .await?;
    let text = String::from_utf8_lossy(&answer);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no answer from the engine"))?;
    let status = head.split_whitespace().nth(1).unwrap_or_default();
    if status != "200" {
        return Err(std::io::Error::other(format!(
            "{path}: {status} {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    let json: serde_json::Value =
        serde_json::from_str(body.trim()).map_err(std::io::Error::other)?;
    Ok(json["SpaceReclaimed"].as_u64().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    /// Closes `write` (the command's stop signal) once the command wrote `ready`: its trap is
    /// set by then, so the signal never races it.
    fn close_when_ready(
        write: std::os::fd::OwnedFd,
        ready: std::path::PathBuf,
    ) -> std::thread::JoinHandle<bool> {
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(write);
            ready.exists()
        })
    }

    #[test]
    fn closing_stdin_stops_the_command_gracefully() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let (marker, ready) = (dir.path().join("term"), dir.path().join("ready"));
        let (read, write) = crate::userns::cloexec_pipe()?;
        let script = format!(
            "trap 'echo term > {}; exit 0' TERM; touch {}; while :; do sleep 0.05; done",
            marker.display(),
            ready.display()
        );
        let closer = close_when_ready(write, ready);
        let code = supervise_with(
            &sh(&script),
            std::fs::File::from(read),
            Duration::from_secs(20),
        )?;
        assert!(
            closer.join().map_err(|_| "closer panicked")?,
            "the command got ready"
        );
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(&marker)?.trim(), "term");
        Ok(())
    }

    #[test]
    fn a_command_that_ignores_term_is_killed_after_the_grace()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ready = dir.path().join("ready");
        let (read, write) = crate::userns::cloexec_pipe()?;
        let closer = close_when_ready(write, ready.clone());
        let started = Instant::now();
        let code = supervise_with(
            &sh(&format!(
                "trap '' TERM; touch {}; while :; do sleep 0.05; done",
                ready.display()
            )),
            std::fs::File::from(read),
            Duration::from_millis(300),
        )?;
        assert!(
            closer.join().map_err(|_| "closer panicked")?,
            "the command got ready"
        );
        assert_eq!(code, 128 + 9);
        assert!(started.elapsed() < Duration::from_secs(20));
        Ok(())
    }

    #[test]
    fn the_commands_own_exit_ends_the_supervision() -> Result<(), Box<dyn std::error::Error>> {
        let (read, write) = crate::userns::cloexec_pipe()?;
        let code = supervise_with(
            &sh("exit 3"),
            std::fs::File::from(read),
            Duration::from_secs(20),
        )?;
        drop(write);
        assert_eq!(code, 3);
        assert!(supervise_with(&[], std::io::empty(), Duration::ZERO).is_err());
        Ok(())
    }

    /// A fake engine API answering each prune with `answers[path]` (status, body) and recording
    /// the request heads.
    async fn fake_api(
        socket: std::path::PathBuf,
        answers: Vec<(&'static str, u16, &'static str)>,
    ) -> std::io::Result<(
        tokio::task::JoinHandle<()>,
        Arc<std::sync::Mutex<Vec<String>>>,
    )> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let heads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&heads);
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                if let Ok(mut heads) = seen.lock() {
                    heads.push(head);
                }
                let (status, body) = answers
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map_or((404, "{}"), |(_, s, b)| (*s, *b));
                let response =
                    format!("HTTP/1.0 {status} X\r\nContent-Type: application/json\r\n\r\n{body}");
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Ok((server, heads))
    }

    const BUILD_PRUNE: &str = "/build/prune?all=1";
    const IMAGE_PRUNE: &str = "/images/prune?filters=%7B%22dangling%22%3A%5B%22false%22%5D%7D";

    #[tokio::test]
    async fn pruning_frees_the_build_cache_and_images_no_container_uses()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let (server, heads) = fake_api(
            socket.clone(),
            vec![
                (
                    BUILD_PRUNE,
                    200,
                    r#"{"CachesDeleted":["a","b"],"SpaceReclaimed":1000}"#,
                ),
                (
                    IMAGE_PRUNE,
                    200,
                    r#"{"ImagesDeleted":[{"Untagged":"x"}],"SpaceReclaimed":234}"#,
                ),
            ],
        )
        .await?;
        assert_eq!(prune(&socket).await?, 1234);
        let heads = heads.lock().map_err(|_| "poisoned")?.clone();
        assert_eq!(heads.len(), 2, "{heads:?}");
        for head in &heads {
            assert!(head.starts_with("POST "), "{head}");
        }
        // Never containers or volumes.
        assert!(
            heads
                .iter()
                .all(|h| !h.contains("/containers/") && !h.contains("/volumes/"))
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn one_failed_prune_keeps_the_other_and_two_are_an_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("docker.sock");
        let (server, _) = fake_api(
            socket.clone(),
            vec![
                (BUILD_PRUNE, 500, r#"{"message":"boom"}"#),
                (IMAGE_PRUNE, 200, r#"{"SpaceReclaimed":234}"#),
            ],
        )
        .await?;
        assert_eq!(prune(&socket).await?, 234);
        server.abort();
        let none = dir.path().join("none.sock");
        assert!(prune(&none).await.is_err());
        Ok(())
    }

    fn view() -> EngineView {
        EngineView {
            bwrap: "/usr/bin/bwrap".into(),
            exe: "/usr/local/bin/yulius".into(),
            hostname: "fixture".into(),
            state_dir: "/data/runtime/sandboxes/s1".into(),
            system_root: "/data/runtime/sandboxes/s1/root".into(),
            data_dir: "/data".into(),
            sandbox_dir: "/data/sandboxes/s1".into(),
            storage: "/data/sysroots/s1/docker".into(),
            cgroup: "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine".into(),
        }
    }

    fn position(argv: &[String], window: &[&str]) -> Option<usize> {
        argv.windows(window.len())
            .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
    }

    /// Sources of every bind, as (option, source, destination).
    fn binds(argv: &[String]) -> Vec<(String, String, String)> {
        let options = [
            "--bind",
            "--ro-bind",
            "--bind-try",
            "--dev-bind",
            "--ro-bind-try",
        ];
        argv.windows(3)
            .filter(|w| options.contains(&w[0].as_str()))
            .map(|w| (w[0].clone(), w[1].clone(), w[2].clone()))
            .collect()
    }

    #[test]
    fn the_engine_gets_its_own_namespaces_capabilities_and_environment() {
        let argv = bwrap_argv(&view());
        assert_eq!(argv[0], "/usr/bin/bwrap");
        for flag in [
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        // Never a new user or network namespace: the sandbox's own are the boundary.
        for flag in [
            "--unshare-user",
            "--unshare-net",
            "--unshare-all",
            "--share-net",
        ] {
            assert!(!argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        assert!(position(&argv, &["--cap-add", "ALL"]).is_some());
        assert!(!argv.contains(&"--cap-drop".to_owned()));
        assert!(position(&argv, &["--setenv", "PATH", ENGINE_PATH]).is_some());
        assert!(position(&argv, &["--setenv", "HOME", "/root"]).is_some());
        assert!(position(&argv, &["--hostname", "fixture"]).is_some());
    }

    #[test]
    fn the_engine_sees_a_runs_view_plus_only_its_own_parts() {
        let argv = bwrap_argv(&view());
        let found = binds(&argv);
        let state = "/data/runtime/sandboxes/s1";
        let mut expected: Vec<(String, String, String)> = SYSTEM_LAYER_DIRS
            .iter()
            .map(|d| ("--bind-try".into(), format!("{state}/root{d}"), (*d).into()))
            .collect();
        for (option, source, dest) in [
            ("--bind", "/data/sandboxes/s1", "/data/sandboxes/s1"),
            ("--ro-bind", "/data/runtime/sandboxes/s1/sys", "/sys"),
            (
                "--bind",
                "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine",
                "/sys/fs/cgroup",
            ),
            ("--bind", "/data/sysroots/s1/docker", "/var/lib/docker"),
            (
                "--bind",
                "/data/runtime/sandboxes/s1/engine",
                "/run/yulius-engine",
            ),
            (
                "--bind",
                "/data/runtime/sandboxes/s1/docker.sock",
                "/run/docker.sock",
            ),
            (
                "--ro-bind",
                "/usr/local/bin/yulius",
                "/usr/local/bin/yulius",
            ),
        ] {
            expected.push((option.into(), source.into(), dest.into()));
        }
        assert_eq!(found, expected);
        // Nothing else of the holder's state (relay sockets, records, the overlay's internals),
        // no host `/sys`, no host cgroup tree, no device binds.
        for (_, source, _) in &found {
            assert_ne!(source, "/sys");
            assert_ne!(source, "/sys/fs/cgroup");
            assert_ne!(source, "/");
            for private in [
                "in.sock",
                "out.sock",
                "holder.json",
                "host.json",
                "upper",
                "work",
            ] {
                assert!(!source.ends_with(private), "{source}");
            }
        }
    }

    #[test]
    fn mounts_come_in_an_order_that_keeps_each_one_visible() {
        let argv = bwrap_argv(&view());
        let state = "/data/runtime/sandboxes/s1";
        let var = position(&argv, &["--bind-try", &format!("{state}/root/var"), "/var"]);
        let run = position(&argv, &["--tmpfs", "/run"]);
        let private = position(&argv, &["--tmpfs", "/data"]);
        let sandbox = position(&argv, &["--bind", "/data/sandboxes/s1"]);
        let storage = position(&argv, &["--bind", "/data/sysroots/s1/docker"]);
        let sys = position(&argv, &["--ro-bind", &format!("{state}/sys"), "/sys"]);
        let cgroup = position(&argv, &["--bind", &view().cgroup, "/sys/fs/cgroup"]);
        let socket = position(&argv, &["--bind", &format!("{state}/docker.sock")]);
        let exe = position(&argv, &["--ro-bind", "/usr/local/bin/yulius"]);
        assert!(run < var && var < private && private < sandbox, "{argv:?}");
        assert!(var < storage && sys < cgroup && run < socket, "{argv:?}");
        assert!(
            private < exe,
            "the executable may live in the data directory: {argv:?}"
        );
    }

    #[test]
    fn the_engine_command_is_the_supervised_dockerd() {
        let argv = bwrap_argv(&view());
        let tail = position(
            &argv,
            &[
                "--chdir",
                "/",
                "--",
                "/usr/local/bin/yulius",
                "sandbox-host",
                "engine",
                "--",
            ],
        );
        assert!(tail.is_some(), "{argv:?}");
        let dockerd = dockerd_argv();
        assert_eq!(&argv[argv.len() - dockerd.len()..], dockerd.as_slice());
        assert!(
            position(
                &dockerd,
                &["--host", "unix:///run/yulius-engine/docker.sock"]
            )
            .is_some()
        );
        assert!(position(&dockerd, &["--data-root", "/var/lib/docker"]).is_some());
        assert!(position(&dockerd, &["--exec-opt", "native.cgroupdriver=cgroupfs"]).is_some());
        assert!(
            !dockerd.iter().any(|a| a.contains("/run/docker.sock")),
            "{dockerd:?}"
        );
    }
}
