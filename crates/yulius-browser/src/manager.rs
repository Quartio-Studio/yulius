//! Lazily started, one-per-sandbox Chromium processes (SPEC-001 BRW-002), each on its own
//! virtual display when one is configured (SPEC-002 ADR-15).

use std::collections::HashMap;
use std::ffi::OsString;
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

/// Wait for a browser whose DevTools pipe ended to be reaped, to report its exit.
const PIPE_END_GRACE: Duration = Duration::from_secs(1);
use crate::instance::{BrowserInstance, InstanceConfig, ScreencastConfig};
use crate::process::{StopTimeouts, Supervised, passed_env};
use crate::url_policy::UrlDenyList;
use serde_json::json;

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

    /// The Chromium argument vector for a profile directory.
    #[must_use]
    pub fn chromium_args(&self, profile_dir: &Path) -> Vec<OsString> {
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
        args.extend(self.extra_args.iter().map(OsString::from));
        args.push("about:blank".into());
        args
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<BrowserInstance>>>>;

/// Owns at most one Chromium per sandbox; starts it on first use, restarts it after a crash on
/// the next use, stops it when idle or on request.
#[derive(Debug)]
pub struct BrowserManager {
    config: BrowserConfig,
    slots: Mutex<HashMap<SandboxId, Slot>>,
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
    let exited = |process: &Supervised| BrowserError::ExitedEarly {
        status: process.exit_status().unwrap_or_default(),
        stderr: process.stderr_tail(),
    };
    tokio::select! {
        answer = cdp.call_within("Browser.getVersion", json!({}), timeout) => match answer {
            Ok(_) => Ok(()),
            Err(CdpError::Timeout { .. }) => Err(BrowserError::StartupTimeout { timeout }),
            // The pipe ends when the browser exits; give the exit a moment to be reaped.
            Err(error) => match process.wait_exit(PIPE_END_GRACE).await {
                Some(_) => Err(exited(process)),
                None => Err(BrowserError::Cdp(error)),
            },
        },
        () = process.exited() => Err(exited(process)),
    }
}

async fn prepare_profile(profile_dir: &Path) -> Result<(), BrowserError> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::create_dir_all(profile_dir)
        .await
        .map_err(BrowserError::Profile)?;
    tokio::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(BrowserError::Profile)
}

impl BrowserManager {
    /// A manager with no running browsers.
    #[must_use]
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            config,
            slots: Mutex::new(HashMap::new()),
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
        if let Some(existing) = guard.as_ref()
            && existing.is_alive()
        {
            existing.touch();
            return Ok(Arc::clone(existing));
        }
        if let Some(dead) = guard.take() {
            warn!(sandbox = %sandbox, "browser exited or disconnected; restarting");
            if let Err(error) = dead.terminate(self.config.stop).await {
                warn!(sandbox = %sandbox, %error, "could not reap the previous browser");
            }
        }
        let instance = self.launch(sandbox, profile_dir).await?;
        // Recursion guard first (BRW-008): fail closed rather than run without it.
        if let Err(error) = instance.deny_urls(self.config.deny.clone()).await {
            warn!(sandbox = %sandbox, %error, "could not enable the browser's address guard");
            let _ = instance.terminate(self.config.stop).await;
            return Err(error);
        }
        instance.set_new_tab_url(
            self.config
                .new_tab_url
                .as_ref()
                .map(|template| template.replace("{sandbox}", sandbox.as_str())),
        );
        instance.show_welcome_in_blank_tabs().await;
        *guard = Some(Arc::clone(&instance));
        Ok(instance)
    }

    /// The sandbox's browser if it is running; never starts one.
    pub async fn get(&self, sandbox: &SandboxId) -> Option<Arc<BrowserInstance>> {
        let slot = lock(&self.slots).get(sandbox).cloned()?;
        let guard = slot.lock().await;
        guard.as_ref().filter(|i| i.is_alive()).cloned()
    }

    async fn launch(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<BrowserInstance>, BrowserError> {
        let exe = resolve_executable(&self.config.chromium_exe)?;
        prepare_profile(profile_dir).await?;
        let args = self.config.chromium_args(profile_dir);
        let quick = StopTimeouts {
            graceful: Duration::ZERO,
            ..self.config.stop
        };
        // The display first: Chrome connects to it at start.
        let display =
            VirtualDisplay::start(&self.config.display, sandbox, self.config.default_viewport)
                .await?;
        let mut env = passed_env();
        if let Some(display) = &display {
            env.retain(|(name, _)| name != "DISPLAY" && name != "XAUTHORITY");
            env.extend(display.env());
        }
        let stop_display = |display: Option<VirtualDisplay>| async move {
            if let Some(display) = display
                && let Err(error) = display.stop(quick).await
            {
                warn!(%error, "could not stop the display of a browser that failed to start");
            }
        };
        let (process, pipe) = match Supervised::spawn_with_devtools_pipe(&exe, &args, &env) {
            Ok(spawned) => spawned,
            Err(error) => {
                stop_display(display).await;
                return Err(BrowserError::Spawn(error));
            }
        };
        let cdp = CdpClient::over_pipe(pipe.from_child, pipe.to_child, self.config.cdp);
        if let Err(error) = wait_until_ready(&cdp, &process, self.config.startup_timeout).await {
            let _ = process.terminate(quick).await;
            stop_display(display).await;
            return Err(error);
        }
        let display_name = display.as_ref().map(VirtualDisplay::name);
        match BrowserInstance::attach_on(
            sandbox.clone(),
            cdp,
            self.config.instance_config(),
            display,
        )
        .await
        {
            Ok(instance) => {
                info!(sandbox = %sandbox, pid = process.pid(), display = ?display_name, "browser started");
                instance.set_process(process);
                Ok(instance)
            }
            Err(error) => {
                // A failed attach has stopped the display it was given.
                let _ = process.terminate(quick).await;
                Err(error)
            }
        }
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
        let Some(instance) = guard.take() else {
            return Ok(false);
        };
        instance.terminate(self.config.stop).await?;
        info!(sandbox = %sandbox, "browser stopped");
        Ok(true)
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
            let Some(instance) = guard.as_ref() else {
                continue;
            };
            let idle = instance.frame_subscribers() == 0
                && now.saturating_duration_since(instance.last_used()) >= self.config.idle_timeout;
            if !idle && instance.is_alive() {
                continue;
            }
            if let Some(instance) = guard.take() {
                if let Err(error) = instance.terminate(self.config.stop).await {
                    warn!(sandbox = %sandbox, %error, "idle browser could not be reaped");
                }
                info!(sandbox = %sandbox, "idle or dead browser stopped");
                stopped.push(sandbox);
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

    #[test]
    fn chromium_args_use_the_devtools_pipe_and_are_sandboxed_by_default() {
        let config = BrowserConfig::new("chromium".into(), "/data/browsers".into());
        let args: Vec<String> = config
            .chromium_args(Path::new("/data/s1/browser"))
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
        let args = explicit.chromium_args(Path::new("/p"));
        assert!(args.iter().any(|arg| arg == "--no-sandbox"));
        assert!(!args.iter().any(|arg| arg == "--headless=new"));
    }
}
