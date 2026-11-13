//! `BrowserManager` with the fake Chromium launcher: one browser per sandbox, launch flags and
//! the DevTools pipe (no TCP endpoint, ADR-0191),
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
    BrowserConfig, BrowserError, BrowserManager, BrowserNetwork, CdpError, NetworkLease, PIPE_URL,
    StopTimeouts,
};
use yulius_contracts::browser::{NavigateParams, ScreenshotParams, TabId, TypeParams};
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
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
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
    assert_eq!(a.cdp().url(), PIPE_URL, "Yulius speaks to it over the pipe");
    assert_eq!(fake.connections(), 2, "one relayed pipe per browser");
    assert_eq!(
        fake.calls_for("Browser.getVersion").len(),
        2,
        "startup waits for an answer"
    );
    assert_eq!(a.tabs().len(), 1);
    assert!(manager.get(&s1).await.is_some());

    let argv = std::fs::read_to_string(p1.join(FAKE_ARGV_FILE))?;
    let args: Vec<&str> = argv.lines().collect();
    for expected in ["--remote-debugging-pipe", "--headless=new", "about:blank"] {
        assert!(args.contains(&expected), "{expected} in {args:?}");
    }
    assert!(
        !argv.contains("--remote-debugging-port") && !argv.contains("--remote-debugging-address"),
        "no DevTools listener is asked for: {args:?}"
    );
    assert!(!p1.join("DevToolsActivePort").exists());
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
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
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

/// SPEC-450 (review round 3): what a sandbox's browser remembers about typed secrets outlives
/// it (stopped idle, crashed), as the site keeps what was typed: a site exposed by a fill into a
/// text field stays refused in the next browser, another site is captured.
#[tokio::test]
async fn typed_secrets_outlive_the_browser() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
    )?;
    let manager = BrowserManager::new(config(exe, dir.path()));
    let sandbox = SandboxId::parse("sbx-secrets")?;
    let profile = manager.default_profile_dir(&sandbox);
    let navigate = |tab: &TabId, url: &str| NavigateParams {
        tab_id: tab.clone(),
        url: url.to_owned(),
    };

    let first = manager.ensure(&sandbox, &profile).await?;
    let tab = first.tabs().first().ok_or("no tab")?.id.clone();
    first
        .tool_navigate(&navigate(&tab, "https://p.example/form"))
        .await?;
    first
        .tool_type_secret(
            &TypeParams {
                tab_id: tab.clone(),
                text: "api-key-5521-qrs".into(),
                selector: Some("#api-key".into()),
            },
            &|_: &str| true,
        )
        .await?;
    assert!(manager.shutdown(&sandbox).await?);

    let second = manager.ensure(&sandbox, &profile).await?;
    assert!(!Arc::ptr_eq(&first, &second));
    let tab = second.tabs().first().ok_or("no tab")?.id.clone();
    let shot = ScreenshotParams {
        tab_id: tab.clone(),
    };
    second
        .tool_navigate(&navigate(&tab, "https://q.example/"))
        .await?;
    second.tool_screenshot_within(&shot, 100_000).await?;
    second
        .tool_navigate(&navigate(&tab, "https://p.example/keys"))
        .await?;
    assert!(matches!(
        second.tool_screenshot_within(&shot, 100_000).await,
        Err(BrowserError::SecretOnPage)
    ));
    assert!(manager.shutdown(&sandbox).await?);
    Ok(())
}

/// SPEC-450: a secret whose page was lost after typing refuses every screenshot, also in the next
/// browser of the sandbox: the site may keep it.
#[tokio::test]
async fn a_lost_secret_refuses_screenshots_after_a_restart() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
    )?;
    let manager = BrowserManager::new(config(exe, dir.path()));
    let sandbox = SandboxId::parse("sbx-lost")?;
    let profile = manager.default_profile_dir(&sandbox);
    let first = manager.ensure(&sandbox, &profile).await?;
    let tab = first.tabs().first().ok_or("no tab")?.id.clone();
    fake.set_password_field("#pw", true);
    let reads = fake.calls_for("Page.getFrameTree").len();
    // The read after the typing: two tell the page before the keys.
    fake.fail_nth("Page.getFrameTree", reads + 3, -32000, "no frame tree");
    first
        .tool_type_secret(
            &TypeParams {
                tab_id: tab,
                text: "pw-lost-secret-2".into(),
                selector: Some("#pw".into()),
            },
            &|_: &str| true,
        )
        .await?;
    assert!(manager.shutdown(&sandbox).await?);

    let second = manager.ensure(&sandbox, &profile).await?;
    let tab = second.tabs().first().ok_or("no tab")?.id.clone();
    second
        .tool_navigate(&NavigateParams {
            tab_id: tab.clone(),
            url: "https://elsewhere.example/".into(),
        })
        .await?;
    assert!(matches!(
        second
            .tool_screenshot_within(&ScreenshotParams { tab_id: tab }, 100_000)
            .await,
        Err(BrowserError::SecretOnPage)
    ));
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
        FakeChromium::IgnoresSigterm {
            port: fake.pipe_port(),
        },
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
            // Written by a descendant after the exit: the error waits for stderr's end.
            assert!(
                stderr.contains("cannot open shared object file"),
                "{stderr}"
            );
        }
        other => return Err(format!("expected an early exit, got {other:?}").into()),
    }

    // A browser that closes its DevTools pipe and stays alive is a DevTools failure, reaped.
    let closing = write_fake_chromium(dir.path(), "closing", FakeChromium::ClosesPipe)?;
    let manager = BrowserManager::new(config(closing, dir.path()));
    match manager.ensure(&sandbox, &profile).await {
        Err(BrowserError::Cdp(CdpError::Closed)) => {}
        other => return Err(format!("expected a closed pipe, got {other:?}").into()),
    }
    let pid = read_pid(&profile, FAKE_PID_FILE)?;
    assert!(!pid_alive(pid), "a failed start reaps its process");
    // The next case checks its own pid, not this one.
    std::fs::remove_file(profile.join(FAKE_PID_FILE))?;

    // A browser that keeps its pipe open but never answers.
    let hanging = write_fake_chromium(dir.path(), "hanging", FakeChromium::NeverReady)?;
    let mut settings = config(hanging, dir.path());
    settings.startup_timeout = Duration::from_secs(1);
    let manager = BrowserManager::new(settings);
    match manager.ensure(&sandbox, &profile).await {
        Err(BrowserError::StartupTimeout { timeout }) => {
            assert_eq!(timeout, Duration::from_secs(1));
        }
        other => return Err(format!("expected a startup timeout, got {other:?}").into()),
    }
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
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
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
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
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

/// BRW-008: a started browser refuses Yulius before any page loads, and a browser whose guard
/// cannot be enabled is not used.
#[tokio::test]
async fn the_address_guard_is_on_before_use_or_the_browser_is_not_used() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
    )?;
    let listeners: Vec<std::net::SocketAddr> = vec!["127.0.0.1:8080".parse()?];
    let manager = BrowserManager::new(BrowserConfig {
        deny: yulius_browser::UrlDenyList::yulius(
            "https://yulius.example.com",
            &listeners,
            Vec::new(),
        ),
        ..config(exe, dir.path())
    });
    let sandbox = SandboxId::parse("sbx-guard")?;
    let instance = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await?;
    assert_eq!(fake.calls_for("Fetch.enable").len(), 1);
    assert!(instance.open("http://localhost:8080/").await.is_err());
    assert!(manager.shutdown(&sandbox).await?);

    fake.fail("Fetch.enable", -32000, "not supported");
    let refused = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await;
    assert!(refused.is_err(), "a browser without its guard is not used");
    assert!(manager.get(&sandbox).await.is_none());
    Ok(())
}

/// A way out for tests: counts the proxies open, numbers them by port, fails when told to.
#[derive(Debug, Default)]
struct CountedNetwork {
    open: Arc<std::sync::atomic::AtomicUsize>,
    opened: std::sync::atomic::AtomicU16,
    failing: std::sync::atomic::AtomicBool,
}

struct Open(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl BrowserNetwork for CountedNetwork {
    fn open(&self, _: &SandboxId) -> std::io::Result<NetworkLease> {
        use std::sync::atomic::Ordering;
        if self.failing.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("no proxy today"));
        }
        self.open.fetch_add(1, Ordering::SeqCst);
        let port = 40_000 + self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(NetworkLease {
            proxy: ([127, 0, 0, 1], port).into(),
            hold: Box::new(Open(Arc::clone(&self.open))),
        })
    }
}

/// SPEC-483 E2 (ADR-0321): with a network configured, each browser is started behind a proxy
/// of its own, kept open exactly as long as the browser runs (a stop, a crash's restart and a
/// start that fails let it go), and a browser whose proxy cannot be opened is not started.
#[tokio::test]
async fn a_browser_runs_behind_its_own_proxy_or_not_at_all() -> TestResult {
    use std::sync::atomic::Ordering;
    let fake = FakeCdpServer::start().await?;
    let dir = tempfile::tempdir()?;
    let exe = write_fake_chromium(
        dir.path(),
        "fake-chromium",
        FakeChromium::Ready {
            port: fake.pipe_port(),
        },
    )?;
    let network = Arc::new(CountedNetwork::default());
    let manager = BrowserManager::new(BrowserConfig {
        network: Some(network.clone()),
        ..config(exe, dir.path())
    });
    let (s1, s2) = (SandboxId::parse("sbx-a")?, SandboxId::parse("sbx-b")?);
    let (p1, p2) = (
        manager.default_profile_dir(&s1),
        manager.default_profile_dir(&s2),
    );
    let proxy_of = |profile: &Path| -> Result<String, Box<dyn std::error::Error>> {
        let argv = std::fs::read_to_string(profile.join(FAKE_ARGV_FILE))?;
        assert!(
            argv.lines()
                .any(|arg| arg == "--proxy-bypass-list=<-loopback>"),
            "{argv}"
        );
        Ok(argv
            .lines()
            .find_map(|arg| arg.strip_prefix("--proxy-server="))
            .ok_or("no proxy in the arguments")?
            .to_owned())
    };

    // The browsers are held on to throughout, as a viewer's session holds one: a proxy closes
    // when its browser ends, not when the last holder lets go.
    let a = manager.ensure(&s1, &p1).await?;
    manager.ensure(&s1, &p1).await?;
    let b = manager.ensure(&s2, &p2).await?;
    assert_eq!(proxy_of(&p1)?, "http://127.0.0.1:40000");
    assert_eq!(proxy_of(&p2)?, "http://127.0.0.1:40001", "one each");
    assert_eq!(
        network.open.load(Ordering::SeqCst),
        2,
        "a running browser keeps its own"
    );

    // A crashed browser's proxy closes when the next one starts, behind a new one.
    kill_pid(a.pid().ok_or("no pid")?, Signal::SIGKILL)?;
    assert!(wait_dead(a.pid().ok_or("no pid")?).await);
    let restarted = manager.ensure(&s1, &p1).await?;
    assert_eq!(proxy_of(&p1)?, "http://127.0.0.1:40002");
    assert_eq!(network.open.load(Ordering::SeqCst), 2);

    assert!(manager.shutdown(&s2).await?);
    assert_eq!(
        network.open.load(Ordering::SeqCst),
        1,
        "a stopped browser's is closed"
    );

    // No proxy, no browser: nothing is spawned.
    network.failing.store(true, Ordering::SeqCst);
    let s3 = SandboxId::parse("sbx-c")?;
    let p3 = manager.default_profile_dir(&s3);
    let refused = manager.ensure(&s3, &p3).await;
    assert!(
        matches!(&refused, Err(BrowserError::Network(error)) if error.to_string() == "no proxy today"),
        "{refused:?}"
    );
    assert!(!p3.join(FAKE_ARGV_FILE).exists(), "it was not started");

    // A start that fails after the proxy was opened closes it.
    network.failing.store(false, Ordering::SeqCst);
    let broken = write_fake_chromium(
        dir.path(),
        "broken-chromium",
        FakeChromium::ExitsImmediately,
    )?;
    let failing = BrowserManager::new(BrowserConfig {
        network: Some(network.clone()),
        ..config(broken, dir.path())
    });
    assert!(failing.ensure(&s3, &p3).await.is_err());
    assert_eq!(network.open.load(Ordering::SeqCst), 1);

    assert!(manager.shutdown(&s1).await?);
    assert_eq!(network.open.load(Ordering::SeqCst), 0);
    drop((a, b, restarted));
    Ok(())
}
