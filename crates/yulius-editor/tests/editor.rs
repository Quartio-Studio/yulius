//! `EditorManager` with the fake openvscode-server: start and flags, token file and health
//! check, reuse, stop with reaping, missing binary, port collisions (held ports, a full range
//! and a lost bind race), crash restart, startup timeout and idle reaping (AC-EDT-1 at the
//! adapter level).

use std::net::TcpListener;
use std::ops::RangeInclusive;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use yulius_domain::ids::SandboxId;
use yulius_editor::{
    EditorConfig, EditorError, EditorInstance, EditorManager, StopTimeouts, TOKEN_FILE,
    default_base_path,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FAKE: &str = env!("CARGO_BIN_EXE_fake-openvscode-server");
const WAIT: Duration = Duration::from_secs(5);

/// A range starting at an OS-assigned free port; the manager skips ports in use.
fn port_range(len: u16) -> Result<RangeInclusive<u16>, Box<dyn std::error::Error>> {
    let start = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    Ok(start..=start.saturating_add(len))
}

fn config(exe: &Path, root: &Path, ports: RangeInclusive<u16>) -> EditorConfig {
    EditorConfig {
        stop: StopTimeouts {
            graceful: Duration::ZERO,
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        },
        ..EditorConfig::new(exe.to_path_buf(), root.join("editors"), ports)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace)?;
    Ok(Fixture {
        _dir: dir,
        root,
        workspace,
    })
}

fn pid_alive(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|pid| kill(Pid::from_raw(pid), None).is_ok())
}

fn user_data(root: &Path, sandbox: &SandboxId) -> PathBuf {
    root.join("editors")
        .join(sandbox.as_str())
        .join("user-data")
}

async fn get(
    port: u16,
    path: &str,
    cookie: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let cookie = cookie
        .map(|c| format!("Cookie: {c}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: x\r\n{cookie}Connection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let mut response = String::new();
    tokio::time::timeout(WAIT, stream.read_to_string(&mut response)).await??;
    Ok(response)
}

async fn ensure(
    manager: &EditorManager,
    sandbox: &SandboxId,
    workspace: &Path,
) -> Result<EditorInstance, EditorError> {
    manager
        .ensure(sandbox, workspace, &default_base_path(sandbox))
        .await
}

#[tokio::test]
async fn starts_with_a_private_token_and_reuses_then_stops_and_reaps() -> TestResult {
    let fx = fixture()?;
    let ports = port_range(40)?;
    let manager = EditorManager::new(config(Path::new(FAKE), &fx.root, ports.clone()));
    let sandbox = SandboxId::parse("sbx-edit")?;
    assert!(manager.get(&sandbox).await.is_none());

    let editor = ensure(&manager, &sandbox, &fx.workspace).await?;
    assert!(ports.contains(&editor.port()));
    assert!(pid_alive(editor.pid()));
    assert_eq!(editor.base_path(), "/s/sbx-edit/editor");
    assert_eq!(editor.workspace_dir(), fx.workspace);
    assert_eq!(
        editor.upstream_origin(),
        format!("http://127.0.0.1:{}", editor.port())
    );
    assert!(!format!("{editor:?}").contains(editor.connection_token().expose_secret()));

    let token_file = fx.root.join("editors/sbx-edit").join(TOKEN_FILE);
    assert_eq!(
        std::fs::read_to_string(&token_file)?,
        editor.connection_token().expose_secret()
    );
    assert_eq!(
        std::fs::metadata(&token_file)?.permissions().mode() & 0o777,
        0o600
    );

    let cookie = editor.connection_token().cookie_value();
    let ok = get(editor.port(), "/s/sbx-edit/editor/", Some(&cookie)).await?;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let anonymous = get(editor.port(), "/s/sbx-edit/editor/", None).await?;
    assert!(anonymous.starts_with("HTTP/1.1 403"), "{anonymous}");

    let argv = std::fs::read_to_string(user_data(&fx.root, &sandbox).join("fake-argv.txt"))?;
    let args: Vec<&str> = argv.lines().collect();
    let pair = |name: &str| {
        args.iter()
            .position(|a| *a == name)
            .and_then(|i| args.get(i + 1))
            .copied()
    };
    assert_eq!(pair("--host"), Some("127.0.0.1"));
    assert_eq!(pair("--port"), Some(editor.port().to_string().as_str()));
    assert_eq!(pair("--connection-token-file"), token_file.to_str());
    assert_eq!(pair("--server-base-path"), Some("/s/sbx-edit/editor"));
    assert_eq!(pair("--default-folder"), fx.workspace.to_str());
    assert!(pair("--user-data-dir").is_some() && pair("--extensions-dir").is_some());
    assert!(!argv.contains("--without-connection-token"));

    let again = ensure(&manager, &sandbox, &fx.workspace).await?;
    assert_eq!(again, editor, "the running editor is reused");
    assert_eq!(manager.get(&sandbox).await, Some(editor.clone()));

    assert!(manager.stop(&sandbox).await?);
    assert!(!pid_alive(editor.pid()), "reaped when stop returns");
    assert!(
        TcpListener::bind(("127.0.0.1", editor.port())).is_ok(),
        "port released"
    );
    assert!(manager.get(&sandbox).await.is_none());
    assert!(!manager.stop(&sandbox).await?);
    Ok(())
}

#[tokio::test]
async fn missing_binary_and_invalid_inputs_are_typed_errors() -> TestResult {
    let fx = fixture()?;
    let sandbox = SandboxId::parse("sbx-missing")?;
    for exe in [
        fx.root.join("no-openvscode-server"),
        PathBuf::from("yulius-no-such-openvscode"),
    ] {
        let manager = EditorManager::new(config(&exe, &fx.root, port_range(5)?));
        match ensure(&manager, &sandbox, &fx.workspace).await {
            Err(error @ EditorError::ExecutableNotFound { .. }) => {
                assert!(error.to_string().contains("openvscode-server"), "{error}");
            }
            other => return Err(format!("expected a missing binary, got {other:?}").into()),
        }
    }
    let manager = EditorManager::new(config(Path::new(FAKE), &fx.root, port_range(5)?));
    assert!(matches!(
        manager
            .ensure(&sandbox, &fx.workspace, "/s/../editor")
            .await,
        Err(EditorError::InvalidBasePath(_))
    ));
    assert!(matches!(
        manager
            .ensure(&sandbox, &fx.root.join("absent"), "/s/x/editor")
            .await,
        Err(EditorError::InvalidWorkspace(_))
    ));
    assert!(matches!(
        manager
            .ensure(&sandbox, Path::new("relative"), "/s/x/editor")
            .await,
        Err(EditorError::InvalidWorkspace(_))
    ));
    Ok(())
}

#[tokio::test]
async fn port_collisions_are_skipped_retried_or_reported() -> TestResult {
    let fx = fixture()?;
    let held = TcpListener::bind("127.0.0.1:0")?;
    let taken = held.local_addr()?.port();

    let full = EditorManager::new(config(Path::new(FAKE), &fx.root, taken..=taken));
    let sandbox = SandboxId::parse("sbx-ports")?;
    match ensure(&full, &sandbox, &fx.workspace).await {
        Err(EditorError::NoFreePort { start, end }) => assert_eq!((start, end), (taken, taken)),
        other => return Err(format!("expected no free port, got {other:?}").into()),
    }

    let manager = EditorManager::new(config(
        Path::new(FAKE),
        &fx.root,
        taken..=taken.saturating_add(40),
    ));
    let editor = ensure(&manager, &sandbox, &fx.workspace).await?;
    assert_ne!(editor.port(), taken, "a held port is skipped");
    let other = SandboxId::parse("sbx-ports-2")?;
    let second = ensure(&manager, &other, &fx.workspace).await?;
    assert_ne!(second.port(), editor.port());
    assert_ne!(second.connection_token(), editor.connection_token());
    assert!(manager.stop_all().await.is_empty());

    // The editor loses a bind race once (exits like EADDRINUSE): the manager retries on
    // another port.
    let racing = SandboxId::parse("sbx-race")?;
    let data = user_data(&fx.root, &racing);
    std::fs::create_dir_all(&data)?;
    std::fs::write(data.join("fake-fail-bind-once"), "")?;
    let editor = ensure(&manager, &racing, &fx.workspace).await?;
    let starts = std::fs::read_to_string(data.join("fake-starts.txt"))?;
    let ports: Vec<&str> = starts.lines().collect();
    assert_eq!(ports.len(), 2, "{starts}");
    assert_ne!(ports[0], ports[1]);
    assert_eq!(ports[1], editor.port().to_string());
    assert!(manager.stop(&racing).await?);
    drop(held);
    Ok(())
}

#[tokio::test]
async fn crash_restarts_on_next_use_with_a_new_token() -> TestResult {
    let fx = fixture()?;
    let manager = EditorManager::new(config(Path::new(FAKE), &fx.root, port_range(40)?));
    let sandbox = SandboxId::parse("sbx-crash")?;
    let first = ensure(&manager, &sandbox, &fx.workspace).await?;
    kill(Pid::from_raw(i32::try_from(first.pid())?), Signal::SIGKILL)?;
    let deadline = Instant::now() + WAIT;
    while manager.get(&sandbox).await.is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(manager.get(&sandbox).await.is_none(), "crash detected");
    let second = ensure(&manager, &sandbox, &fx.workspace).await?;
    assert_ne!(second.pid(), first.pid());
    assert_ne!(second.connection_token(), first.connection_token());
    let ok = get(
        second.port(),
        "/s/sbx-crash/editor/",
        Some(&second.connection_token().cookie_value()),
    )
    .await?;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(manager.stop(&sandbox).await?);
    Ok(())
}

#[tokio::test]
async fn unhealthy_editor_times_out_and_is_reaped() -> TestResult {
    let fx = fixture()?;
    let mut settings = config(Path::new(FAKE), &fx.root, port_range(40)?);
    settings.startup_timeout = Duration::from_millis(800);
    let manager = EditorManager::new(settings);
    let sandbox = SandboxId::parse("sbx-forbid")?;
    let data = user_data(&fx.root, &sandbox);
    std::fs::create_dir_all(&data)?;
    std::fs::write(data.join("fake-forbid"), "")?;
    match ensure(&manager, &sandbox, &fx.workspace).await {
        Err(EditorError::StartupTimeout { timeout, detail }) => {
            assert_eq!(timeout, Duration::from_millis(800));
            assert!(
                detail.contains("403") || detail.contains("connect"),
                "{detail}"
            );
        }
        other => return Err(format!("expected a startup timeout, got {other:?}").into()),
    }
    // Reaped before `ensure` returned; if the fake got far enough to record its pid, that
    // pid is gone.
    if let Ok(pid) = std::fs::read_to_string(data.join("fake-pid")) {
        assert!(!pid_alive(pid.trim().parse()?));
    }
    assert!(manager.get(&sandbox).await.is_none());
    Ok(())
}

#[tokio::test]
async fn idle_editors_are_reaped_and_touch_keeps_them() -> TestResult {
    let fx = fixture()?;
    let settings = config(Path::new(FAKE), &fx.root, port_range(40)?);
    let idle = settings.idle_timeout;
    let manager = Arc::new(EditorManager::new(settings));
    let sandbox = SandboxId::parse("sbx-idle")?;
    let editor = ensure(&manager, &sandbox, &fx.workspace).await?;
    let just_before = Instant::now() + idle - Duration::from_secs(1);
    manager.touch(&sandbox);
    assert!(manager.reap_idle(just_before).await.is_empty());
    let after = Instant::now() + idle + Duration::from_secs(1);
    assert_eq!(manager.reap_idle(after).await, vec![sandbox.clone()]);
    assert!(!pid_alive(editor.pid()));
    assert!(manager.get(&sandbox).await.is_none());

    let mut quick = config(Path::new(FAKE), &fx.root, port_range(40)?);
    quick.idle_timeout = Duration::ZERO;
    let quick = Arc::new(EditorManager::new(quick));
    let editor = ensure(&quick, &sandbox, &fx.workspace).await?;
    let _reaper = quick.spawn_idle_reaper(Duration::from_secs(1));
    let deadline = Instant::now() + WAIT;
    while pid_alive(editor.pid()) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !pid_alive(editor.pid()),
        "the background reaper stopped the idle editor"
    );
    Ok(())
}
