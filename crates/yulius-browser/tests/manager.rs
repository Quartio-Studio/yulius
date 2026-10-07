//! `BrowserManager` with the fake Chromium launcher: one browser per sandbox, launch flags,
//! crash detection and restart, kill-and-reap of the whole process group (including a
//! SIGTERM-ignoring child and a forked descendant), typed startup failures and idle reaping.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use yulius_browser::fake::{
    FAKE_ARGV_FILE, FAKE_CHILD_PID_FILE, FAKE_PID_FILE, FakeCdpServer, FakeChromium,
    write_fake_chromium,
};
use yulius_browser::{
    BrowserConfig, BrowserError, BrowserManager, DEVTOOLS_PORT_FILE, StopTimeouts,
};
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(5);

fn config(exe: PathBuf, root: &Path) -> BrowserConfig {
    BrowserConfig {
        // Generous: the first exec of a freshly written script can be slow on a loaded macOS
        // host (executable scanning); a ready fake normally starts in milliseconds.
        startup_timeout: Duration::from_secs(30),
        stop: StopTimeouts {
            graceful: Duration::from_millis(100),
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        },
        idle_timeout: Duration::from_secs(60),
        ..BrowserConfig::new(exe, root.join("profiles"))
    }
}

fn pid_alive(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|pid| kill(Pid::from_raw(pid), None).is_ok())
}

/// Polls (bounded) until `pid` is gone: an orphaned descendant is reaped by init shortly
/// after its group is killed.
async fn wait_dead(pid: u32) -> bool {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn read_pid(dir: &Path, file: &str) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string(dir.join(file))?.trim().parse()?)
}

fn kill_pid(pid: u32, signal: Signal) -> TestResult {
    kill(Pid::from_raw(i32::try_from(pid)?), signal)?;
    Ok(())
}

#[tokio::test]
async fn one_browser_per_sandbox_with_isolated_profiles_and_full_shutdown() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let manager = BrowserManager::new(config(exe, dir.path()));
    let (s1, s2) = (SandboxId::parse("sbx-a")?, SandboxId::parse("sbx-b")?);
    let (p1, p2) = (
        manager.default_profile_dir(&s1),
        manager.default_profile_dir(&s2),
    );
    assert!(manager.get(&s1).await.is_none());

    let a = manager.ensure(&s1, &p1).await?;
    let again = manager.ensure(&s1, &p1).await?;
    assert!(Arc::ptr_eq(&a, &again), "the running browser is reused");
    let b = manager.ensure(&s2, &p2).await?;
    let (pid_a, pid_b) = (a.pid().ok_or("no pid")?, b.pid().ok_or("no pid")?);
    assert_ne!(pid_a, pid_b);
    assert_eq!(a.cdp_ws_url(), fake.ws_url());
    assert_eq!(a.tabs().len(), 1);
    assert!(manager.get(&s1).await.is_some());

    let argv = std::fs::read_to_string(p1.join(FAKE_ARGV_FILE))?;
    let args: Vec<&str> = argv.lines().collect();
    for expected in [
        "--remote-debugging-port=0",
        "--remote-debugging-address=127.0.0.1",
        "--headless=new",
        "about:blank",
    ] {
        assert!(args.contains(&expected), "{expected} in {args:?}");
    }
    assert!(args.contains(&format!("--user-data-dir={}", p1.display()).as_str()));
    assert!(!argv.contains("--no-sandbox"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&p1)?.permissions().mode() & 0o777, 0o700);
    assert!(std::fs::read_to_string(p2.join(FAKE_ARGV_FILE))?.contains(&p2.display().to_string()));

    let child_a = read_pid(&p1, FAKE_CHILD_PID_FILE)?;
    assert!(pid_alive(child_a));
    assert!(manager.shutdown(&s1).await?);
    assert!(
        !pid_alive(pid_a),
        "the browser is reaped when shutdown returns"
    );
    assert!(
        wait_dead(child_a).await,
        "the process group's descendants are killed"
    );
    assert!(!a.is_alive());
    assert!(manager.get(&s1).await.is_none());
    assert!(!manager.shutdown(&s1).await?);

    assert!(manager.shutdown_all().await.is_empty());
    assert!(!pid_alive(pid_b));
    Ok(())
}

#[tokio::test]
async fn crash_is_detected_and_the_next_use_restarts() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let manager = BrowserManager::new(config(exe, dir.path()));
    let sandbox = SandboxId::parse("sbx-crash")?;
    let profile = manager.default_profile_dir(&sandbox);

    let first = manager.ensure(&sandbox, &profile).await?;
    let pid = first.pid().ok_or("no pid")?;
    let orphan = read_pid(&profile, FAKE_CHILD_PID_FILE)?;
    kill_pid(pid, Signal::SIGKILL)?;
    tokio::time::timeout(WAIT, first.wait_disconnected()).await?;
    assert!(!first.is_alive());
    assert!(manager.get(&sandbox).await.is_none());

    let second = manager.ensure(&sandbox, &profile).await?;
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.is_alive());
    assert_ne!(second.pid(), Some(pid));
    assert!(
        wait_dead(orphan).await,
        "restart cleans the crashed browser's group"
    );

    // A dropped DevTools connection with the process still running also counts as a crash:
    // the old process is stopped before a new one starts on the same profile.
    let pid_second = second.pid().ok_or("no pid")?;
    fake.disconnect_all();
    tokio::time::timeout(WAIT, second.wait_disconnected()).await?;
    let third = manager.ensure(&sandbox, &profile).await?;
    assert!(third.is_alive());
    assert!(!pid_alive(pid_second));
    assert!(manager.shutdown(&sandbox).await?);
    Ok(())
}

#[tokio::test]
async fn shutdown_escalates_to_sigkill_for_a_browser_ignoring_sigterm() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "stubborn",
        FakeChromium::IgnoresSigterm { port: fake.port() },
    )?;
    let mut settings = config(exe, dir.path());
    settings.stop = StopTimeouts {
        graceful: Duration::from_millis(50),
        term: Duration::from_millis(300),
        kill: Duration::from_secs(5),
    };
    let manager = BrowserManager::new(settings);
    let sandbox = SandboxId::parse("sbx-stubborn")?;
    let profile = manager.default_profile_dir(&sandbox);
    let browser = manager.ensure(&sandbox, &profile).await?;
    let pid = browser.pid().ok_or("no pid")?;
    let child = read_pid(&profile, FAKE_CHILD_PID_FILE)?;
    assert!(manager.shutdown(&sandbox).await?);
    assert!(!pid_alive(pid));
    assert!(wait_dead(child).await);
    assert!(
        !fake.calls_for("Browser.close").is_empty(),
        "graceful close is tried first"
    );
    Ok(())
}

#[tokio::test]
async fn startup_failures_are_typed_and_leave_no_process() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sandbox = SandboxId::parse("sbx-fail")?;

    let missing = BrowserManager::new(config(dir.path().join("missing-chromium"), dir.path()));
    let profile = missing.default_profile_dir(&sandbox);
    assert!(matches!(
        missing.ensure(&sandbox, &profile).await,
        Err(BrowserError::ExecutableNotFound { .. })
    ));
    let bare = BrowserManager::new(config("yulius-no-such-chromium".into(), dir.path()));
    match bare.ensure(&sandbox, &profile).await {
        Err(error @ BrowserError::ExecutableNotFound { .. }) => {
            assert!(
                error.to_string().contains("yulius-no-such-chromium"),
                "{error}"
            );
        }
        other => return Err(format!("expected a missing executable, got {other:?}").into()),
    }

    let crashing = write_fake_chromium(dir.path(), "crashing", FakeChromium::ExitsImmediately)?;
    let manager = BrowserManager::new(config(crashing, dir.path()));
    match manager.ensure(&sandbox, &profile).await {
        Err(BrowserError::ExitedEarly { status, stderr }) => {
            assert!(status.contains('1'), "{status}");
            assert!(
                stderr.contains("cannot open shared object file"),
                "{stderr}"
            );
        }
        other => return Err(format!("expected an early exit, got {other:?}").into()),
    }

    // A stale DevToolsActivePort from a previous run must not be trusted.
    let stale_server = FakeCdpServer::start().await?;
    std::fs::create_dir_all(&profile)?;
    std::fs::write(
        profile.join(DEVTOOLS_PORT_FILE),
        format!("{}\n/devtools/browser/stale\n", stale_server.port()),
    )?;
    let hanging = write_fake_chromium(dir.path(), "hanging", FakeChromium::NeverReady)?;
    let mut settings = config(hanging, dir.path());
    settings.startup_timeout = Duration::from_secs(1);
    let manager = BrowserManager::new(settings);
    match manager.ensure(&sandbox, &profile).await {
        Err(BrowserError::StartupTimeout { timeout, .. }) => {
            assert_eq!(timeout, Duration::from_secs(1));
        }
        other => return Err(format!("expected a startup timeout, got {other:?}").into()),
    }
    assert_eq!(
        stale_server.connections(),
        0,
        "the stale port file was ignored"
    );
    // The process is reaped before `ensure` returns, so no pid file can appear later. If the
    // script ran far enough to record its pid, that pid must be gone.
    if let Ok(pid) = read_pid(&profile, FAKE_PID_FILE) {
        assert!(!pid_alive(pid), "a failed start reaps its process");
    }
    assert!(manager.get(&sandbox).await.is_none());
    Ok(())
}

#[tokio::test]
async fn idle_browsers_are_reaped_unless_a_viewer_watches() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let settings = config(exe, dir.path());
    let idle = settings.idle_timeout;
    let manager = BrowserManager::new(settings);
    let sandbox = SandboxId::parse("sbx-idle")?;
    let profile = manager.default_profile_dir(&sandbox);
    let browser = manager.ensure(&sandbox, &profile).await?;
    let pid = browser.pid().ok_or("no pid")?;
    let tab = browser
        .tabs()
        .first()
        .map(|t| t.id.clone())
        .ok_or("no tab")?;

    assert!(
        manager.reap_idle(Instant::now()).await.is_empty(),
        "recently used"
    );
    let frames = browser.subscribe_frames(&tab).await?;
    let later = Instant::now() + idle + Duration::from_secs(1);
    assert!(
        manager.reap_idle(later).await.is_empty(),
        "a viewer is watching"
    );
    drop(frames);
    assert_eq!(browser.frame_subscribers(), 0);
    assert_eq!(manager.reap_idle(later).await, vec![sandbox.clone()]);
    assert!(!pid_alive(pid));
    assert!(manager.get(&sandbox).await.is_none());

    // Crashed browsers are reaped regardless of activity.
    let restarted = manager.ensure(&sandbox, &profile).await?;
    kill_pid(restarted.pid().ok_or("no pid")?, Signal::SIGKILL)?;
    tokio::time::timeout(WAIT, restarted.wait_disconnected()).await?;
    assert_eq!(
        manager.reap_idle(Instant::now()).await,
        vec![sandbox.clone()]
    );
    Ok(())
}

#[tokio::test]
async fn background_reaper_stops_idle_browsers() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready { port: fake.port() },
    )?;
    let mut settings = config(exe, dir.path());
    settings.idle_timeout = Duration::ZERO;
    let manager = Arc::new(BrowserManager::new(settings));
    let sandbox = SandboxId::parse("sbx-reaper")?;
    let browser = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await?;
    let pid = browser.pid().ok_or("no pid")?;
    let _reaper = manager.spawn_idle_reaper(Duration::from_secs(1));
    tokio::time::timeout(WAIT, browser.wait_disconnected()).await?;
    assert!(wait_dead(pid).await);
    assert!(manager.get(&sandbox).await.is_none());
    Ok(())
}
