//! Lazily started, one-per-sandbox Chromium processes (SPEC-001 BRW-002).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::{info, warn};
use yulius_domain::ids::SandboxId;

use crate::cdp::CdpConfig;
use crate::discovery::resolve_executable;
use crate::error::BrowserError;
use crate::instance::{BrowserInstance, InstanceConfig, ScreencastConfig};
use crate::process::{StopTimeouts, Supervised, passed_env};

/// File Chromium writes into the profile when started with `--remote-debugging-port=0`.
pub const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";
/// Poll interval while waiting for [`DEVTOOLS_PORT_FILE`].
const PORT_FILE_POLL: Duration = Duration::from_millis(50);

/// Browser adapter configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserConfig {
    /// Chromium executable: a path, or a bare name searched on `PATH`
    /// (see [`crate::discover_chromium`]).
    pub chromium_exe: PathBuf,
    /// Root for default per-sandbox profiles ([`BrowserManager::default_profile_dir`]).
    pub profile_root: PathBuf,
    /// Run with `--headless=new` (always in production; `false` only for local debugging).
    pub headless: bool,
    /// Initial viewport (width, height) in CSS pixels.
    pub default_viewport: (u32, u32),
    /// Stop a browser after this long without activity and without viewers.
    pub idle_timeout: Duration,
    /// Maximum wait for `DevToolsActivePort` after spawning.
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
}

impl BrowserConfig {
    /// Defaults: headless, 1280×800, 30 min idle timeout, 30 s startup, 5+5+10 s stop
    /// escalation, Chromium sandbox on, 32 tabs, 15 s load wait.
    #[must_use]
    pub fn new(chromium_exe: PathBuf, profile_root: PathBuf) -> Self {
        let instance = InstanceConfig::default();
        Self {
            chromium_exe,
            profile_root,
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
            "--remote-debugging-port=0".into(),
            "--remote-debugging-address=127.0.0.1".into(),
            user_data_dir,
        ];
        if self.headless {
            args.push("--headless=new".into());
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
        args.push(format!("--window-size={width},{height}").into());
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

/// Parses `DevToolsActivePort` (`<port>\n/devtools/browser/<id>\n`) into the WebSocket URL.
fn parse_port_file(text: &str) -> Result<String, String> {
    let mut lines = text.lines();
    let port = lines
        .next()
        .and_then(|line| line.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| "first line is not a TCP port".to_owned())?;
    let path = lines
        .next()
        .map(str::trim)
        .filter(|path| {
            path.starts_with("/devtools/browser/") && path.bytes().all(|b| b.is_ascii_graphic())
        })
        .ok_or_else(|| "second line is not a /devtools/browser/ path".to_owned())?;
    Ok(format!("ws://127.0.0.1:{port}{path}"))
}

async fn wait_for_port_file(
    port_file: &Path,
    process: &Supervised,
    timeout: Duration,
) -> Result<String, BrowserError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_problem = String::new();
    loop {
        if let Some(status) = process.exit_status() {
            return Err(BrowserError::ExitedEarly {
                status,
                stderr: process.stderr_tail(),
            });
        }
        match tokio::fs::read_to_string(port_file).await {
            Ok(text) => match parse_port_file(&text) {
                Ok(url) => return Ok(url),
                // Possibly a partial write; retry until the deadline.
                Err(problem) => last_problem = format!(": {DEVTOOLS_PORT_FILE} {problem}"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => last_problem = format!(": reading {DEVTOOLS_PORT_FILE}: {error}"),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(BrowserError::StartupTimeout {
                timeout,
                detail: last_problem,
            });
        }
        tokio::time::sleep(PORT_FILE_POLL).await;
    }
}

async fn prepare_profile(profile_dir: &Path) -> Result<PathBuf, BrowserError> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::create_dir_all(profile_dir)
        .await
        .map_err(BrowserError::Profile)?;
    tokio::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(BrowserError::Profile)?;
    let port_file = profile_dir.join(DEVTOOLS_PORT_FILE);
    match tokio::fs::remove_file(&port_file).await {
        Ok(()) => Ok(port_file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(port_file),
        Err(error) => Err(BrowserError::Profile(error)),
    }
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
        let port_file = prepare_profile(profile_dir).await?;
        let args = self.config.chromium_args(profile_dir);
        let process = Supervised::spawn(&exe, &args, &passed_env()).map_err(BrowserError::Spawn)?;
        let quick = StopTimeouts {
            graceful: Duration::ZERO,
            ..self.config.stop
        };
        let ws_url =
            match wait_for_port_file(&port_file, &process, self.config.startup_timeout).await {
                Ok(url) => url,
                Err(error) => {
                    let _ = process.terminate(quick).await;
                    return Err(error);
                }
            };
        match BrowserInstance::attach(sandbox.clone(), &ws_url, self.config.instance_config()).await
        {
            Ok(instance) => {
                info!(sandbox = %sandbox, pid = process.pid(), "browser started");
                instance.set_process(process);
                Ok(instance)
            }
            Err(error) => {
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
    fn port_file_parsing_requires_port_and_browser_path() {
        assert_eq!(
            parse_port_file("40123\n/devtools/browser/abc-123\n"),
            Ok("ws://127.0.0.1:40123/devtools/browser/abc-123".into())
        );
        assert!(parse_port_file("").is_err());
        assert!(parse_port_file("40123\n").is_err());
        assert!(parse_port_file("0\n/devtools/browser/x\n").is_err());
        assert!(parse_port_file("70000\n/devtools/browser/x\n").is_err());
        assert!(parse_port_file("40123\n/devtools/page/x\n").is_err());
        assert!(parse_port_file("40123\n/devtools/browser/x y\n").is_err());
    }

    #[test]
    fn chromium_args_are_loopback_isolated_and_sandboxed_by_default() {
        let config = BrowserConfig::new("chromium".into(), "/data/browsers".into());
        let args: Vec<String> = config
            .chromium_args(Path::new("/data/s1/browser"))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
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
