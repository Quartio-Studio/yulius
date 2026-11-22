//! Lazily started, one-per-sandbox Chromium processes (SPEC-001 BRW-002), each on its own
//! virtual display when one is configured (SPEC-002 ADR-15).

use std::collections::HashMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::{info, warn};
use yulius_domain::ids::SandboxId;

use crate::cdp::{CdpClient, CdpConfig};
use crate::discovery::resolve_executable;
use crate::display::{DisplayMode, VirtualDisplay};
use crate::error::{BrowserError, CdpError};
use crate::instance::{BrowserInstance, InstanceConfig, ScreencastConfig, SecretMemory};
use crate::network::BrowserNetwork;
use crate::process::{StopTimeouts, Supervised, passed_env};
use crate::url_policy::UrlDenyList;
use serde_json::json;

/// Wait for a browser whose DevTools pipe ended to be reaped, and for an exited browser's stderr
/// to end, before reporting how it failed.
const PIPE_END_GRACE: Duration = Duration::from_secs(1);

/// Browser adapter configuration.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// Chromium executable: a path, or a bare name searched on `PATH`
    /// (see [`crate::discover_chromium`]).
    pub chromium_exe: PathBuf,
    /// Root for default per-sandbox profiles ([`BrowserManager::default_profile_dir`]).
    pub profile_root: PathBuf,
    /// How browsers render: headless, or headful on a virtual display (SPEC-002 ADR-15).
    pub display: DisplayMode,
    /// Without a display: run with `--headless=new` (`false` only for local debugging, where
    /// Chrome opens a window on the developer's desktop).
    pub headless: bool,
    /// Initial viewport (width, height) in CSS pixels.
    pub default_viewport: (u32, u32),
    /// Stop a browser after this long without activity and without viewers.
    pub idle_timeout: Duration,
    /// Maximum wait for the browser's first DevTools answer after spawning.
    pub startup_timeout: Duration,
    /// Stop escalation deadlines.
    pub stop: StopTimeouts,
    /// Pass `--no-sandbox`. Off by default; only for hosts where Chromium's sandbox cannot run
    /// (e.g. inside some containers) and with that trade-off accepted explicitly.
    pub no_sandbox: bool,
    /// Additional Chromium switches appended before the initial URL.
    pub extra_args: Vec<String>,
    /// CDP limits.
    pub cdp: CdpConfig,
    /// Screencast parameters.
    pub screencast: ScreencastConfig,
    /// Maximum open tabs per browser.
    pub max_tabs: usize,
    /// Tool wait for the load event.
    pub load_timeout: Duration,
    /// Address of new and blank tabs, `{sandbox}` replaced by the sandbox id (the Yulius
    /// welcome page, BRW-006); `None` keeps `about:blank`.
    pub new_tab_url: Option<String>,
    /// Addresses the browser never loads: Yulius itself (BRW-008). Applied before the first
    /// page loads; a browser whose interception cannot be enabled is not used.
    pub deny: UrlDenyList,
    /// The browsers' way out (ADR-0321): each is started behind a proxy of its own that this
    /// opens, and not at all when that fails. `None`: Chrome connects by itself, from the
    /// host (tests, and a developer's machine without sandboxes).
    pub network: Option<Arc<dyn BrowserNetwork>>,
}

impl BrowserConfig {
    /// Defaults: headless without a display, 1280×800, 30 min idle timeout, 30 s startup, 5+5+10 s stop
    /// escalation, Chromium sandbox on, 32 tabs, 15 s load wait.
    #[must_use]
    pub fn new(chromium_exe: PathBuf, profile_root: PathBuf) -> Self {
        let instance = InstanceConfig::default();
        Self {
            chromium_exe,
            profile_root,
            display: DisplayMode::Headless,
            headless: true,
            default_viewport: instance.viewport,
            idle_timeout: Duration::from_secs(30 * 60),
            startup_timeout: Duration::from_secs(30),
            stop: StopTimeouts::default(),
            no_sandbox: false,
            extra_args: Vec::new(),
            cdp: instance.cdp,
            screencast: instance.screencast,
            max_tabs: instance.max_tabs,
            load_timeout: instance.load_timeout,
            new_tab_url: None,
            deny: UrlDenyList::default(),
            network: None,
        }
    }

    fn instance_config(&self) -> InstanceConfig {
        InstanceConfig {
            viewport: self.default_viewport,
            cdp: self.cdp,
            screencast: self.screencast,
            max_tabs: self.max_tabs,
            load_timeout: self.load_timeout,
        }
    }

    /// The Chromium argument vector for a profile directory, behind `proxy` when it has one.
    #[must_use]
    pub fn chromium_args(&self, profile_dir: &Path, proxy: Option<SocketAddr>) -> Vec<OsString> {
        let (width, height) = self.default_viewport;
        let mut user_data_dir = OsString::from("--user-data-dir=");
        user_data_dir.push(profile_dir.as_os_str());
        let mut args: Vec<OsString> = vec![
            // DevTools on fds 3 and 4 only: no listener anyone else could reach (ADR-0191).
            "--remote-debugging-pipe".into(),
            user_data_dir,
        ];
        match &self.display {
            DisplayMode::Virtual(display) => {
                args.extend(display.chromium_args(display.device_size(self.default_viewport)));
            }
            #[cfg(feature = "fake")]
            DisplayMode::Fake(fake) => {
                let display = fake.config();
                args.extend(display.chromium_args(display.device_size(self.default_viewport)));
            }
            DisplayMode::Headless if self.headless => args.push("--headless=new".into()),
            DisplayMode::Headless => {}
        }
        args.extend(
            [
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-default-apps",
                "--disable-background-timer-throttling",
                "--disable-renderer-backgrounding",
            ]
            .map(OsString::from),
        );
        if !self.display.is_virtual() {
            args.push(format!("--window-size={width},{height}").into());
        }
        if cfg!(target_os = "linux") {
            args.push("--password-store=basic".into());
        }
        if cfg!(target_os = "macos") {
            args.push("--use-mock-keychain".into());
        }
        if self.no_sandbox {
            args.push("--no-sandbox".into());
        }
        if let Some(proxy) = proxy {
            args.extend(crate::network::proxy_args(proxy));
        }
        args.extend(self.extra_args.iter().map(OsString::from));
        args.push("about:blank".into());
        args
    }
}

#[derive(Debug, Default)]
enum BrowserState {
    #[default]
    Stopped,
    Starting {
        display: Option<Arc<VirtualDisplay>>,
        process: Option<Arc<Supervised>>,
    },
    Unpublished(Arc<BrowserInstance>),
    Ready(Arc<BrowserInstance>),
}

#[derive(Debug, Default)]
struct BrowserSlot {
    state: BrowserState,
    // Kept across a failed replacement launch, until it succeeds or the sandbox is stopped.
    reopen: Vec<yulius_contracts::browser::TabInfo>,
}

impl BrowserSlot {
    fn ready(&self) -> Option<&Arc<BrowserInstance>> {
        match &self.state {
            BrowserState::Ready(instance) => Some(instance),
            _ => None,
        }
    }

    async fn stop(&mut self, timeouts: StopTimeouts) -> Result<bool, BrowserError> {
        if let BrowserState::Ready(instance) = &self.state {
            self.state = BrowserState::Unpublished(Arc::clone(instance));
        }
        match &self.state {
            BrowserState::Stopped => return Ok(false),
            BrowserState::Starting { process, display } => {
                if let Some(process) = process {
                    process
                        .terminate(timeouts)
                        .await
                        .map_err(|_| BrowserError::NotReaped { pid: process.pid() })?;
                }
                if let Some(display) = display {
                    display.stop(timeouts).await?;
                }
            }
            BrowserState::Ready(instance) | BrowserState::Unpublished(instance) => {
                instance.terminate(timeouts).await?
            }
        }
        self.state = BrowserState::Stopped;
        Ok(true)
    }
}

type Slot = Arc<tokio::sync::Mutex<BrowserSlot>>;

/// Owns at most one Chromium per sandbox; starts it on first use, restarts it after a crash on
/// the next use, stops it when idle or on request.
#[derive(Debug)]
pub struct BrowserManager {
    config: BrowserConfig,
    slots: Mutex<HashMap<SandboxId, Slot>>,
    /// What each sandbox's browsers remember about typed secrets, across restarts (SPEC-450).
    secrets: Mutex<HashMap<SandboxId, SecretMemory>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waits until the browser answers on its DevTools pipe, or exits, within `timeout`.
async fn wait_until_ready(
    cdp: &CdpClient,
    process: &Supervised,
    timeout: Duration,
) -> Result<(), BrowserError> {
    let exited = async |process: &Supervised| BrowserError::ExitedEarly {
        status: process.exit_status().unwrap_or_default(),
        stderr: process.stderr_tail_within(PIPE_END_GRACE).await,
    };
    tokio::select! {
        answer = cdp.call_within("Browser.getVersion", json!({}), timeout) => match answer {
            Ok(_) => Ok(()),
            Err(CdpError::Timeout { .. }) => Err(BrowserError::StartupTimeout { timeout }),
            // The pipe ends when the browser exits; give the exit a moment to be reaped.
            Err(error) => match process.wait_exit(PIPE_END_GRACE).await {
                Some(_) => Err(exited(process).await),
                None => Err(BrowserError::Cdp(error)),
            },
        },
        () = process.exited() => Err(exited(process).await),
    }
}

/// Chrome handed over to, or waited for, `holder`, which still holds the profile: say so.
fn blame_holder(error: BrowserError, holder: Option<u32>) -> BrowserError {
    match (error, holder) {
        (
            error @ (BrowserError::ExitedEarly { .. } | BrowserError::StartupTimeout { .. }),
            Some(pid),
        ) => BrowserError::ProfileInUse {
            pid,
            error: Box::new(error),
        },
        (error, _) => error,
    }
}

/// Creates the profile (mode 0700) and removes Chrome's locks no running browser holds
/// (SPEC-593); returns this host's name and the pid of a running process of it whose lock was
/// kept.
async fn prepare_profile(profile_dir: &Path) -> Result<Option<(Vec<u8>, u32)>, BrowserError> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::create_dir_all(profile_dir)
        .await
        .map_err(BrowserError::Profile)?;
    tokio::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(BrowserError::Profile)?;
    let host = crate::profile::host_name()?;
    let held =
        crate::profile::clear_stale_locks(profile_dir, &host, crate::profile::process_running)
            .await?;
    Ok(held.map(|pid| (host, pid)))
}

impl BrowserManager {
    /// A manager with no running browsers.
    #[must_use]
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            config,
            slots: Mutex::new(HashMap::new()),
            secrets: Mutex::new(HashMap::new()),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &BrowserConfig {
        &self.config
    }

    /// `<profile_root>/<sandbox>`.
    #[must_use]
    pub fn default_profile_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.config.profile_root.join(sandbox.as_str())
    }

    fn slot(&self, sandbox: &SandboxId) -> Slot {
        Arc::clone(lock(&self.slots).entry(sandbox.clone()).or_default())
    }

    /// Returns the sandbox's running browser, starting it (or restarting it after a crash or
    /// disconnect) with `profile_dir` as its profile. A running browser keeps its profile.
    ///
    /// # Errors
    /// Executable, profile, startup or CDP connection failures; a failed start leaves no
    /// process behind.
    pub async fn ensure(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        let slot = self.slot(sandbox);
        let mut guard = slot.lock().await;
        if let Some(existing) = guard.ready()
            && existing.is_alive()
        {
            existing.touch();
            return Ok(Arc::clone(existing));
        }
        self.replace(&mut guard, sandbox, profile_dir).await
    }

    /// Reaps the sandbox's current browser and reopens its visible pages on a fresh display
    /// using the same profile. Other users of this slot await the replacement.
    ///
    /// # Errors
    /// Stop and startup failures, including failures installing the address guard.
    pub async fn restart(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        let slot = self.slot(sandbox);
        let mut guard = slot.try_lock().map_err(|_| BrowserError::BrowserBusy)?;
        if let Some(existing) = guard.ready() {
            guard.reopen = existing.tabs_to_reopen()?;
        }
        self.replace(&mut guard, sandbox, profile_dir).await
    }

    async fn replace(
        &self,
        slot: &mut BrowserSlot,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        slot.stop(self.config.stop).await?;
        let instance = match self.launch(&mut slot.state, sandbox, profile_dir).await {
            Ok(instance) => instance,
            Err(error) => {
                slot.stop(StopTimeouts {
                    graceful: Duration::ZERO,
                    ..self.config.stop
                })
                .await?;
                return Err(error);
            }
        };
        slot.state = BrowserState::Unpublished(Arc::clone(&instance));
        // Recursion guard first (BRW-008): fail closed rather than run without it.
        if let Err(error) = instance.deny_urls(self.config.deny.clone()).await {
            warn!(sandbox = %sandbox, %error, "could not enable the browser's address guard");
            slot.stop(self.config.stop).await?;
            return Err(error);
        }
        instance.set_new_tab_url(
            self.config
                .new_tab_url
                .as_ref()
                .map(|template| template.replace("{sandbox}", sandbox.as_str())),
        );
        if let Err(error) = instance.reopen_tabs(&slot.reopen).await {
            slot.stop(self.config.stop).await?;
            return Err(error);
        }
        instance.show_welcome_in_blank_tabs().await;
        slot.reopen.clear();
        slot.state = BrowserState::Ready(Arc::clone(&instance));
        Ok(instance)
    }

    /// The sandbox's browser if it is running; never starts one.
    pub async fn get(&self, sandbox: &SandboxId) -> Option<Arc<BrowserInstance>> {
        let slot = lock(&self.slots).get(sandbox).cloned()?;
        let guard = slot.lock().await;
        guard.ready().filter(|i| i.is_alive()).cloned()
    }

    async fn launch(
        &self,
        state: &mut BrowserState,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        let exe = resolve_executable(&self.config.chromium_exe)?;
        let held = prepare_profile(profile_dir).await?;
        self.start(state, sandbox, profile_dir, &exe, held).await
    }

    async fn start(
        &self,
        state: &mut BrowserState,
        sandbox: &SandboxId,
        profile_dir: &Path,
        exe: &Path,
        held: Option<(Vec<u8>, u32)>,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        // Its way out first: a browser that cannot have it is not started (ADR-0321). A start
        // that fails below drops the lease, which closes it.
        let network = match &self.config.network {
            Some(network) => Some(network.open(sandbox).map_err(BrowserError::Network)?),
            None => None,
        };
        let args = self
            .config
            .chromium_args(profile_dir, network.as_ref().map(|lease| lease.proxy));
        // Own each resource before the next fallible or cancellable startup operation.
        let display =
            VirtualDisplay::start(&self.config.display, sandbox, self.config.default_viewport)
                .await?
                .map(Arc::new);
        *state = BrowserState::Starting {
            display: display.clone(),
            process: None,
        };
        let mut env = passed_env();
        if let Some(display) = &display {
            env.retain(|(name, _)| name != "DISPLAY" && name != "XAUTHORITY");
            env.extend(display.env());
        }
        let (process, pipe) =
            Supervised::spawn_with_devtools_pipe(exe, &args, &env).map_err(BrowserError::Spawn)?;
        let process = Arc::new(process);
        *state = BrowserState::Starting {
            display: display.clone(),
            process: Some(Arc::clone(&process)),
        };
        let cdp = CdpClient::over_pipe(pipe.from_child, pipe.to_child, self.config.cdp);
        if let Err(error) = wait_until_ready(&cdp, &process, self.config.startup_timeout).await {
            let holder = match held {
                Some((host, pid)) if crate::profile::still_held(profile_dir, &host, pid).await => {
                    Some(pid)
                }
                _ => None,
            };
            return Err(blame_holder(error, holder));
        }
        let display_name = display.as_deref().map(VirtualDisplay::name);
        let secrets = lock(&self.secrets)
            .entry(sandbox.clone())
            .or_default()
            .clone();
        let instance = BrowserInstance::attach_on(
            sandbox.clone(),
            cdp,
            self.config.instance_config(),
            display,
            secrets,
        )
        .await?;
        info!(sandbox = %sandbox, pid = process.pid(), display = ?display_name, "browser started");
        instance.set_process(process);
        if let Some(lease) = network {
            instance.hold_network(lease.hold);
        }
        Ok(instance)
    }

    /// Stops the sandbox's browser (graceful close, then SIGTERM/SIGKILL of its process group)
    /// and reaps it. Returns whether one was running.
    ///
    /// # Errors
    /// [`BrowserError::NotReaped`] when the process survived SIGKILL.
    pub async fn shutdown(&self, sandbox: &SandboxId) -> Result<bool, BrowserError> {
        let Some(slot) = lock(&self.slots).get(sandbox).cloned() else {
            return Ok(false);
        };
        let mut guard = slot.lock().await;
        guard.reopen.clear();
        guard.stop(self.config.stop).await
    }

    /// Stops every browser (server shutdown). Returns the sandboxes whose process could not
    /// be reaped.
    pub async fn shutdown_all(&self) -> Vec<SandboxId> {
        let sandboxes: Vec<SandboxId> = lock(&self.slots).keys().cloned().collect();
        let mut failed = Vec::new();
        for sandbox in sandboxes {
            if self.shutdown(&sandbox).await.is_err() {
                failed.push(sandbox);
            }
        }
        failed
    }

    /// Stops browsers idle since before `now - idle_timeout` with no frame subscribers, and
    /// reaps crashed ones. Busy slots (an `ensure` in progress) are skipped. Returns the
    /// stopped sandboxes.
    pub async fn reap_idle(&self, now: Instant) -> Vec<SandboxId> {
        let slots: Vec<(SandboxId, Slot)> = lock(&self.slots)
            .iter()
            .map(|(sandbox, slot)| (sandbox.clone(), Arc::clone(slot)))
            .collect();
        let mut stopped = Vec::new();
        for (sandbox, slot) in slots {
            let Ok(mut guard) = slot.try_lock() else {
                continue;
            };
            if let Some(instance) = guard.ready() {
                let idle = instance.frame_subscribers() == 0
                    && now.saturating_duration_since(instance.last_used())
                        >= self.config.idle_timeout;
                if !idle && instance.is_alive() {
                    continue;
                }
            }
            match guard.stop(self.config.stop).await {
                Ok(true) => {
                    info!(sandbox = %sandbox, "idle or dead browser stopped");
                    stopped.push(sandbox);
                }
                Ok(false) => {}
                Err(error) => warn!(sandbox = %sandbox, %error, "idle browser could not be reaped"),
            }
        }
        stopped
    }

    /// Runs [`Self::reap_idle`] every `period` until the returned handle is dropped.
    #[must_use]
    pub fn spawn_idle_reaper(self: &Arc<Self>, period: Duration) -> IdleReaper {
        let manager: Weak<Self> = Arc::downgrade(self);
        let period = period.max(Duration::from_secs(1));
        let task = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                manager.reap_idle(Instant::now()).await;
            }
        });
        IdleReaper { task }
    }
}

/// Background idle reaper; aborted on drop.
#[derive(Debug)]
pub struct IdleReaper {
    task: JoinHandle<()>,
}

impl Drop for IdleReaper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_restart_is_refused_while_the_browser_slot_is_busy()
    -> Result<(), Box<dyn std::error::Error>> {
        use futures::FutureExt as _;
        use yulius_contracts::browser::ViewerErrorCode;
        let manager = BrowserManager::new(BrowserConfig::new("unused".into(), "/unused".into()));
        let sandbox = SandboxId::parse("busy-browser")?;
        let slot = manager.slot(&sandbox);
        let _busy = slot.lock().await;
        let result = manager
            .restart(&sandbox, Path::new("/unused"))
            .now_or_never();
        assert!(matches!(result, Some(Err(e)) if e.viewer_code() == ViewerErrorCode::Capacity));
        Ok(())
    }

    /// SPEC-593 SPL-003 (review round 1): only Chrome's own give-ups (it exited, or never
    /// answered) are blamed on the process that still holds the profile.
    #[test]
    fn a_failed_start_names_the_holder_only_when_chrome_gave_up_on_it() {
        let exited = || BrowserError::ExitedEarly {
            status: "exit status: 0".into(),
            stderr: "Opening in existing browser session.".into(),
        };
        let timeout = || BrowserError::StartupTimeout {
            timeout: Duration::from_secs(30),
        };
        for error in [exited(), timeout()] {
            let blamed = blame_holder(error, Some(77));
            assert!(
                matches!(&blamed, BrowserError::ProfileInUse { pid: 77, .. }),
                "{blamed:?}"
            );
            assert!(blamed.to_string().contains("process 77"), "{blamed}");
        }
        assert!(matches!(
            blame_holder(exited(), None),
            BrowserError::ExitedEarly { .. }
        ));
        for other in [
            BrowserError::Cdp(CdpError::Closed),
            BrowserError::Display("no Xvfb".into()),
        ] {
            let kept = blame_holder(other, Some(77));
            assert!(
                !matches!(kept, BrowserError::ProfileInUse { .. }),
                "{kept:?}"
            );
        }
    }

    #[test]
    fn chromium_args_use_the_devtools_pipe_and_are_sandboxed_by_default() {
        let config = BrowserConfig::new("chromium".into(), "/data/browsers".into());
        let args: Vec<String> = config
            .chromium_args(Path::new("/data/s1/browser"), None)
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "--remote-debugging-pipe",
            "--user-data-dir=/data/s1/browser",
            "--headless=new",
            "--window-size=1280,800",
        ] {
            assert!(
                args.iter().any(|arg| arg == expected),
                "{expected} in {args:?}"
            );
        }
        assert!(!args.iter().any(|arg| arg.contains("no-sandbox")));
        assert!(!args.iter().any(|arg| arg.contains("proxy")), "{args:?}");
        // No DevTools listener: any process on the network namespace could drive it (#626).
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--remote-debugging-port")
                    || arg.starts_with("--remote-debugging-address")),
            "{args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
        let explicit = BrowserConfig {
            no_sandbox: true,
            headless: false,
            ..config
        };
        let args = explicit.chromium_args(Path::new("/p"), None);
        assert!(args.iter().any(|arg| arg == "--no-sandbox"));
        assert!(!args.iter().any(|arg| arg == "--headless=new"));
    }

    #[test]
    fn sandbox_browsers_skip_default_app_installation() {
        let mut config = BrowserConfig::new("chromium".into(), "/profiles".into());
        for display in [
            DisplayMode::Headless,
            DisplayMode::Virtual(crate::DisplayConfig::new("/run".into())),
        ] {
            config.display = display;
            let args = config.chromium_args(Path::new("/profiles/s1"), None);
            assert!(args.iter().any(|arg| arg == "--disable-default-apps"));
        }
    }

    #[test]
    fn virtual_display_skips_extensions_already_in_the_profile() {
        let mut config = BrowserConfig::new("chromium".into(), "/profiles".into());
        let profile = Path::new("/profiles/s1");
        let headless = config.chromium_args(profile, None);
        assert!(!headless.iter().any(|arg| arg == "--disable-extensions"));

        config.display = DisplayMode::Virtual(crate::DisplayConfig::new("/run".into()));
        let args = config.chromium_args(profile, None);
        assert!(args.iter().any(|arg| arg == "--disable-extensions"));
    }

    /// SPEC-483 E2: behind a proxy, Chrome is told to use it for every address (the loopback
    /// too, which it reaches directly otherwise), to resolve no name itself and to send no UDP
    /// past it; a switch an operator adds comes after and the page to open stays last.
    #[test]
    fn chromium_args_send_everything_through_the_browsers_proxy() {
        let config = BrowserConfig {
            extra_args: vec!["--lang=pl".into()],
            ..BrowserConfig::new("chromium".into(), "/data/browsers".into())
        };
        let proxy = SocketAddr::from(([127, 0, 0, 1], 41234));
        let args: Vec<String> = config
            .chromium_args(Path::new("/p"), Some(proxy))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let at = |wanted: &str| args.iter().position(|arg| arg == wanted);
        for expected in [
            "--proxy-server=http://127.0.0.1:41234",
            "--proxy-bypass-list=<-loopback>",
            "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
            "--disable-quic",
        ] {
            assert!(at(expected) < at("--lang=pl"), "{expected} in {args:?}");
            assert!(at(expected).is_some(), "{expected} in {args:?}");
        }
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
    }
}
