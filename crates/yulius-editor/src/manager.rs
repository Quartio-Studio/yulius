//! On-demand openvscode-server per key — one per (user, sandbox) in Yulius — optionally inside
//! a containment wrapper (SPEC-002 ADR-10; SPEC-001 EDT-001..004).

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::{info, warn};
use yulius_domain::ids::SandboxId;

use crate::health::probe;
use crate::process::{StopTimeouts, Supervised, passed_env, resolve_program};
use crate::token::{ConnectionToken, write_token_file};

/// File name of the connection token inside `<data_root>/<sandbox>/`.
pub const TOKEN_FILE: &str = "connection-token";
/// Settings of a new VS Code profile (written once; the user's later changes win): Yulius'
/// dark look, no walkthrough or AI chat side bar, and the sandbox trusted (it is the user's
/// own workspace; the sandbox containment is the boundary).
pub const DEFAULT_SETTINGS: &str = r#"{
  "workbench.colorTheme": "Default Dark Modern",
  "workbench.startupEditor": "none",
  "workbench.secondarySideBar.defaultVisibility": "hidden",
  "workbench.welcomePage.walkthroughs.openOnInstall": false,
  "workbench.tips.enabled": false,
  "chat.disableAIFeatures": true,
  "security.workspace.trust.enabled": false,
  "telemetry.telemetryLevel": "off",
  "update.mode": "none",
  "extensions.ignoreRecommendations": true,
  "git.openRepositoryInParentFolders": "always",
  "editor.fontSize": 13,
  "terminal.integrated.fontSize": 13
}
"#;
/// Spawn attempts on different ports when the editor exits during startup (port races).
const START_ATTEMPTS: usize = 3;
/// Interval between health probes during startup.
const HEALTH_POLL: Duration = Duration::from_millis(100);
/// Largest accepted base path.
const BASE_PATH_MAX: usize = 256;

/// Editor adapter failure.
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    /// openvscode-server is not installed (an external prerequisite on the VM).
    #[error(
        "openvscode-server executable not found ({searched}); install openvscode-server and configure its path"
    )]
    ExecutableNotFound {
        /// The configured name or path.
        searched: String,
    },
    /// The base path is not of the form `/segment/segment…` with safe characters.
    #[error("invalid editor base path {0:?}")]
    InvalidBasePath(String),
    /// The workspace directory is missing or not a directory.
    #[error("workspace directory is not an existing directory: {0}")]
    InvalidWorkspace(PathBuf),
    /// Data directory or token file preparation failed.
    #[error("failed to prepare editor data: {0}")]
    Prepare(#[source] std::io::Error),
    /// The process could not be spawned.
    #[error("failed to start openvscode-server: {0}")]
    Spawn(#[source] std::io::Error),
    /// Every port of the range is in use.
    #[error("no free port in {start}..={end}")]
    NoFreePort {
        /// First port of the range.
        start: u16,
        /// Last port of the range.
        end: u16,
    },
    /// The editor did not pass its health check in time.
    #[error("openvscode-server was not healthy within {timeout:?}: {detail}")]
    StartupTimeout {
        /// Applied timeout.
        timeout: Duration,
        /// Last health probe outcome.
        detail: String,
    },
    /// The editor exited during startup on every attempt.
    #[error("openvscode-server exited during startup ({status}); stderr tail: {stderr}")]
    ExitedEarly {
        /// Exit status text.
        status: String,
        /// Last bytes of stderr.
        stderr: String,
    },
    /// The process survived SIGKILL.
    #[error("openvscode-server (pid {pid}) did not exit after SIGKILL")]
    NotReaped {
        /// Process id.
        pid: u32,
    },
}

/// Editor adapter configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorConfig {
    /// openvscode-server executable: a path, or a bare name searched on `PATH`.
    pub exe: PathBuf,
    /// Root for per-sandbox editor data (`<data_root>/<sandbox>/{user-data,extensions}` and
    /// the token file).
    pub data_root: PathBuf,
    /// Loopback ports the editors may listen on.
    pub port_range: RangeInclusive<u16>,
    /// Stop an editor after this long without [`EditorManager::touch`] or `ensure`.
    pub idle_timeout: Duration,
    /// Maximum wait for the first successful health check.
    pub startup_timeout: Duration,
    /// Timeout of one health probe.
    pub probe_timeout: Duration,
    /// Stop escalation (the graceful phase is skipped: the editor has no shutdown request).
    pub stop: StopTimeouts,
}

impl EditorConfig {
    /// Defaults: 30 min idle timeout, 30 s startup, 2 s probes, 5 s SIGTERM + 10 s SIGKILL.
    #[must_use]
    pub fn new(exe: PathBuf, data_root: PathBuf, port_range: RangeInclusive<u16>) -> Self {
        Self {
            exe,
            data_root,
            port_range,
            idle_timeout: Duration::from_secs(30 * 60),
            startup_timeout: Duration::from_secs(30),
            probe_timeout: Duration::from_secs(2),
            stop: StopTimeouts {
                graceful: Duration::ZERO,
                ..StopTimeouts::default()
            },
        }
    }
}

/// Everything needed to start one editor. Paths named `*_arg` are as seen by the editor
/// process (inside containment they differ from host paths); the others are host paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorSpec {
    /// Slot key: at most one editor runs per key (Yulius: `<sandbox>/<user>`).
    pub key: String,
    /// Host path of the connection token file (written 0600 before the start, reused later).
    pub token_file: PathBuf,
    /// `--connection-token-file` as seen by the editor.
    pub token_file_arg: PathBuf,
    /// Host paths of the user data and extensions directories (created 0700; new profiles get
    /// [`DEFAULT_SETTINGS`]).
    pub user_data_dir: PathBuf,
    /// See [`EditorSpec::user_data_dir`].
    pub extensions_dir: PathBuf,
    /// `--user-data-dir` as seen by the editor.
    pub user_data_arg: PathBuf,
    /// `--extensions-dir` as seen by the editor.
    pub extensions_arg: PathBuf,
    /// Folder opened by default (an existing host directory, bound at the same path).
    pub workspace_dir: PathBuf,
    /// Base path the editor serves under (`/s/<sandbox>/editor`).
    pub base_path: String,
    /// Command prefix that wraps the editor (e.g. a bubblewrap argv ending in `--`); empty
    /// runs it directly.
    pub wrapper: Vec<OsString>,
    /// Environment on top of the allowlisted host variables (later entries win).
    pub env: Vec<(OsString, OsString)>,
}

impl EditorSpec {
    /// The single-tenant layout: `<data_root>/<sandbox>/{user-data,extensions,connection-token}`,
    /// no wrapper.
    #[must_use]
    pub fn for_sandbox(
        config: &EditorConfig,
        sandbox: &SandboxId,
        workspace_dir: &Path,
        base_path: &str,
    ) -> Self {
        let root = config.data_root.join(sandbox.as_str());
        Self {
            key: sandbox.as_str().to_owned(),
            token_file: root.join(TOKEN_FILE),
            token_file_arg: root.join(TOKEN_FILE),
            user_data_dir: root.join("user-data"),
            extensions_dir: root.join("extensions"),
            user_data_arg: root.join("user-data"),
            extensions_arg: root.join("extensions"),
            workspace_dir: workspace_dir.to_path_buf(),
            base_path: base_path.to_owned(),
            wrapper: Vec::new(),
            env: Vec::new(),
        }
    }
}

/// How to reach a running editor. The reverse proxy forwards `/s/<sandbox>/editor/*` to
/// `http://127.0.0.1:<port>` unchanged (the editor serves under its base path) and adds
/// `Cookie: <ConnectionToken::cookie_value>` to every request and WebSocket upgrade, after
/// removing any `vscode-tkn` cookie sent by the client. The token never reaches the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorInstance {
    key: String,
    port: u16,
    connection_token: ConnectionToken,
    base_path: String,
    workspace_dir: PathBuf,
    pid: u32,
}

impl EditorInstance {
    /// The slot key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Loopback port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Connection token required by the editor.
    #[must_use]
    pub fn connection_token(&self) -> &ConnectionToken {
        &self.connection_token
    }

    /// Base path the editor serves under (e.g. `/s/<sandbox>/editor`).
    #[must_use]
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    /// Folder opened by default (another folder can be opened with `?folder=<path>`).
    #[must_use]
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `http://127.0.0.1:<port>` for the reverse proxy.
    #[must_use]
    pub fn upstream_origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// `/s/<sandbox>/editor`, the base path used by the public route (docs/api.md).
#[must_use]
pub fn default_base_path(sandbox: &SandboxId) -> String {
    format!("/s/{}/editor", sandbox.as_str())
}

fn validate_base_path(base_path: &str) -> Result<(), EditorError> {
    let valid = base_path.len() <= BASE_PATH_MAX
        && base_path.starts_with('/')
        && base_path.len() > 1
        && !base_path.ends_with('/')
        && base_path
            .split('/')
            .skip(1)
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
        && base_path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(EditorError::InvalidBasePath(base_path.to_owned()))
    }
}

struct Running {
    instance: EditorInstance,
    process: Supervised,
    last_used: Mutex<Instant>,
}

type Slot = Arc<tokio::sync::Mutex<Option<Running>>>;

/// Owns at most one openvscode-server per key.
#[derive(Debug)]
pub struct EditorManager {
    config: EditorConfig,
    slots: Mutex<HashMap<String, Slot>>,
    reserved_ports: Mutex<HashSet<u16>>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// FNV-1a, to spread sandboxes over the port range deterministically.
fn spread(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

async fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::create_dir_all(path).await?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await
}

/// Writes [`DEFAULT_SETTINGS`] as `<user_data>/User/settings.json` unless it exists.
async fn write_default_settings(user_data: &Path) -> std::io::Result<()> {
    let dir = user_data.join("User");
    create_private_dir(&dir).await?;
    let settings = dir.join("settings.json");
    if tokio::fs::try_exists(&settings).await? {
        return Ok(());
    }
    tokio::fs::write(&settings, DEFAULT_SETTINGS).await
}

impl EditorManager {
    /// A manager with no running editors.
    #[must_use]
    pub fn new(config: EditorConfig) -> Self {
        Self {
            config,
            slots: Mutex::new(HashMap::new()),
            reserved_ports: Mutex::new(HashSet::new()),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &EditorConfig {
        &self.config
    }

    fn slot(&self, key: &str) -> Slot {
        Arc::clone(lock(&self.slots).entry(key.to_owned()).or_default())
    }

    /// Returns the sandbox's running editor or starts one serving `workspace_dir` under
    /// `base_path` with the single-tenant layout ([`EditorSpec::for_sandbox`]).
    ///
    /// # Errors
    /// As [`EditorManager::ensure_spec`].
    pub async fn ensure(
        &self,
        sandbox: &SandboxId,
        workspace_dir: &Path,
        base_path: &str,
    ) -> Result<EditorInstance, EditorError> {
        let spec = EditorSpec::for_sandbox(&self.config, sandbox, workspace_dir, base_path);
        self.ensure_spec(&spec).await
    }

    /// Returns the running editor of `spec.key` or starts one. A running editor is reused as
    /// is; a crashed one is reaped and replaced (reusing its persisted token).
    ///
    /// # Errors
    /// Invalid inputs, missing executable, no free port, spawn failure, early exit or a failed
    /// health check; a failed start leaves no process behind.
    pub async fn ensure_spec(&self, spec: &EditorSpec) -> Result<EditorInstance, EditorError> {
        validate_base_path(&spec.base_path)?;
        let workspace_dir = &spec.workspace_dir;
        let is_dir = tokio::fs::metadata(workspace_dir)
            .await
            .is_ok_and(|meta| meta.is_dir());
        if !workspace_dir.is_absolute() || !is_dir {
            return Err(EditorError::InvalidWorkspace(workspace_dir.clone()));
        }
        let slot = self.slot(&spec.key);
        let mut guard = slot.lock().await;
        if let Some(running) = guard.as_ref()
            && !running.process.has_exited()
        {
            *lock(&running.last_used) = Instant::now();
            return Ok(running.instance.clone());
        }
        if let Some(dead) = guard.take() {
            warn!(editor = %spec.key, "editor exited; restarting");
            if let Err(error) = self.release(&dead).await {
                warn!(editor = %spec.key, %error, "could not reap the previous editor");
            }
        }
        let running = self.launch(spec).await?;
        let instance = running.instance.clone();
        *guard = Some(running);
        Ok(instance)
    }

    /// The sandbox's editor (single-tenant key) if it is running; never starts one.
    pub async fn get(&self, sandbox: &SandboxId) -> Option<EditorInstance> {
        self.get_key(sandbox.as_str()).await
    }

    /// The editor of `key` if it is running; never starts one.
    pub async fn get_key(&self, key: &str) -> Option<EditorInstance> {
        let slot = lock(&self.slots).get(key).cloned()?;
        let guard = slot.lock().await;
        guard
            .as_ref()
            .filter(|running| !running.process.has_exited())
            .map(|running| running.instance.clone())
    }

    /// Records activity for idle tracking. The proxy calls it per request and periodically
    /// while an editor WebSocket is open. A slot busy starting or stopping is skipped.
    pub fn touch(&self, sandbox: &SandboxId) {
        self.touch_key(sandbox.as_str());
    }

    /// [`EditorManager::touch`] by key.
    pub fn touch_key(&self, key: &str) {
        let Some(slot) = lock(&self.slots).get(key).cloned() else {
            return;
        };
        if let Ok(guard) = slot.try_lock()
            && let Some(running) = guard.as_ref()
        {
            *lock(&running.last_used) = Instant::now();
        }
    }

    fn pick_port(&self, key: &str, skip: &HashSet<u16>) -> Option<u16> {
        let (start, end) = (
            *self.config.port_range.start(),
            *self.config.port_range.end(),
        );
        if start > end {
            return None;
        }
        let count = u64::from(end - start) + 1;
        let offset = spread(key) % count;
        let reserved = lock(&self.reserved_ports).clone();
        (0..count)
            .filter_map(|step| u16::try_from(u64::from(start) + (offset + step) % count).ok())
            .filter(|port| !reserved.contains(port) && !skip.contains(port))
            .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
    }

    async fn launch(&self, spec: &EditorSpec) -> Result<Running, EditorError> {
        let exe = resolve_program(&self.config.exe, std::env::var_os("PATH").as_deref())
            .ok_or_else(|| EditorError::ExecutableNotFound {
                searched: self.config.exe.display().to_string(),
            })?;
        let token_dir = spec.token_file.parent().ok_or_else(|| {
            EditorError::Prepare(std::io::Error::other("token file has no parent"))
        })?;
        for dir in [token_dir, &spec.user_data_dir, &spec.extensions_dir] {
            create_private_dir(dir)
                .await
                .map_err(EditorError::Prepare)?;
        }
        write_default_settings(&spec.user_data_dir)
            .await
            .map_err(EditorError::Prepare)?;
        // The token outlives restarts so an open workbench can reconnect after an idle stop.
        let token = match ConnectionToken::read(&spec.token_file) {
            Some(token) => token,
            None => {
                let token = ConnectionToken::generate()
                    .await
                    .map_err(EditorError::Prepare)?;
                write_token_file(&spec.token_file, &token).map_err(EditorError::Prepare)?;
                token
            }
        };
        let (program, prefix): (PathBuf, Vec<OsString>) = match spec.wrapper.split_first() {
            Some((program, rest)) => (
                PathBuf::from(program),
                rest.iter()
                    .cloned()
                    .chain(std::iter::once(exe.clone().into_os_string()))
                    .collect(),
            ),
            None => (exe.clone(), Vec::new()),
        };
        let mut env = passed_env();
        env.extend(spec.env.iter().cloned());

        let mut tried = HashSet::new();
        let mut last_exit = None;
        for _ in 0..START_ATTEMPTS {
            let port = self
                .pick_port(&spec.key, &tried)
                .ok_or(EditorError::NoFreePort {
                    start: *self.config.port_range.start(),
                    end: *self.config.port_range.end(),
                })?;
            tried.insert(port);
            let mut args = prefix.clone();
            args.extend([
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                port.to_string().into(),
                "--connection-token-file".into(),
                spec.token_file_arg.clone().into_os_string(),
                "--server-base-path".into(),
                spec.base_path.clone().into(),
                "--user-data-dir".into(),
                spec.user_data_arg.clone().into_os_string(),
                "--extensions-dir".into(),
                spec.extensions_arg.clone().into_os_string(),
                "--default-folder".into(),
                spec.workspace_dir.clone().into_os_string(),
            ]);
            let process = Supervised::spawn(&program, &args, &env, &spec.workspace_dir)
                .map_err(EditorError::Spawn)?;
            lock(&self.reserved_ports).insert(port);
            match self
                .wait_healthy(&process, port, &spec.base_path, &token)
                .await
            {
                Ok(()) => {
                    info!(editor = %spec.key, port, pid = process.pid(), contained = !spec.wrapper.is_empty(), "editor started");
                    let instance = EditorInstance {
                        key: spec.key.clone(),
                        port,
                        connection_token: token,
                        base_path: spec.base_path.clone(),
                        workspace_dir: spec.workspace_dir.clone(),
                        pid: process.pid(),
                    };
                    return Ok(Running {
                        instance,
                        process,
                        last_used: Mutex::new(Instant::now()),
                    });
                }
                Err(error) => {
                    let _ = process.terminate(self.config.stop).await;
                    lock(&self.reserved_ports).remove(&port);
                    match error {
                        // Possibly lost a race for the port; try another one.
                        exited @ EditorError::ExitedEarly { .. } => last_exit = Some(exited),
                        other => return Err(other),
                    }
                }
            }
        }
        Err(last_exit.unwrap_or(EditorError::NoFreePort {
            start: *self.config.port_range.start(),
            end: *self.config.port_range.end(),
        }))
    }

    async fn wait_healthy(
        &self,
        process: &Supervised,
        port: u16,
        base_path: &str,
        token: &ConnectionToken,
    ) -> Result<(), EditorError> {
        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        let path = format!("{base_path}/");
        let cookie = token.cookie_value();
        loop {
            if let Some(status) = process.exit_status() {
                return Err(EditorError::ExitedEarly {
                    status,
                    stderr: process.stderr_tail(),
                });
            }
            let detail = match probe(port, &path, Some(&cookie), self.config.probe_timeout).await {
                // 2xx/3xx with the token proves the listener is this editor, not a stranger
                // that grabbed the port.
                Ok(status) if (200..400).contains(&status) => return Ok(()),
                Ok(status) => format!("HTTP {status} for GET {path}"),
                Err(problem) => problem,
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(EditorError::StartupTimeout {
                    timeout: self.config.startup_timeout,
                    detail,
                });
            }
            tokio::time::sleep(HEALTH_POLL).await;
        }
    }

    async fn release(&self, running: &Running) -> Result<(), EditorError> {
        let outcome = running.process.terminate(self.config.stop).await;
        lock(&self.reserved_ports).remove(&running.instance.port);
        outcome.map(|_| ()).map_err(|_| EditorError::NotReaped {
            pid: running.instance.pid,
        })
    }

    /// Stops the sandbox's editor (SIGTERM, then SIGKILL of its process group) and reaps it.
    /// Returns whether one was running.
    ///
    /// # Errors
    /// [`EditorError::NotReaped`] when the process survived SIGKILL.
    pub async fn stop(&self, sandbox: &SandboxId) -> Result<bool, EditorError> {
        self.stop_key(sandbox.as_str()).await
    }

    /// [`EditorManager::stop`] by key.
    ///
    /// # Errors
    /// [`EditorError::NotReaped`] when the process survived SIGKILL.
    pub async fn stop_key(&self, key: &str) -> Result<bool, EditorError> {
        let Some(slot) = lock(&self.slots).get(key).cloned() else {
            return Ok(false);
        };
        let mut guard = slot.lock().await;
        let Some(running) = guard.take() else {
            return Ok(false);
        };
        self.release(&running).await?;
        info!(editor = %key, "editor stopped");
        Ok(true)
    }

    /// Stops every editor; returns the keys whose process could not be reaped.
    pub async fn stop_all(&self) -> Vec<String> {
        let keys: Vec<String> = lock(&self.slots).keys().cloned().collect();
        let mut failed = Vec::new();
        for key in keys {
            if self.stop_key(&key).await.is_err() {
                failed.push(key);
            }
        }
        failed
    }

    /// Stops editors idle since before `now - idle_timeout` and reaps crashed ones; busy
    /// slots are skipped. Returns the stopped keys.
    pub async fn reap_idle(&self, now: Instant) -> Vec<String> {
        let slots: Vec<(String, Slot)> = lock(&self.slots)
            .iter()
            .map(|(key, slot)| (key.clone(), Arc::clone(slot)))
            .collect();
        let mut stopped = Vec::new();
        for (key, slot) in slots {
            let Ok(mut guard) = slot.try_lock() else {
                continue;
            };
            let Some(running) = guard.as_ref() else {
                continue;
            };
            let idle = now.saturating_duration_since(*lock(&running.last_used))
                >= self.config.idle_timeout;
            if !idle && !running.process.has_exited() {
                continue;
            }
            if let Some(running) = guard.take() {
                if let Err(error) = self.release(&running).await {
                    warn!(editor = %key, %error, "idle editor could not be reaped");
                }
                info!(editor = %key, "idle or dead editor stopped");
                stopped.push(key);
            }
        }
        stopped
    }

    /// Runs [`Self::reap_idle`] every `period` (at least 1 s) until the handle is dropped.
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
    fn base_paths_are_validated() {
        for valid in ["/s/sbx-1/editor", "/editor", "/a.b/c_d"] {
            assert!(validate_base_path(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "", "/", "editor", "/s/x/", "/s//x", "/s/../x", "/s/./x", "/s/x y", "/s/x?y",
        ] {
            assert!(validate_base_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_base_path(&format!("/{}", "a".repeat(BASE_PATH_MAX))).is_err());
    }

    #[test]
    fn default_settings_are_valid_json_with_a_dark_theme() -> Result<(), Box<dyn std::error::Error>>
    {
        let settings: serde_json::Value = serde_json::from_str(DEFAULT_SETTINGS)?;
        assert_eq!(settings["workbench.colorTheme"], "Default Dark Modern");
        assert_eq!(settings["security.workspace.trust.enabled"], false);
        Ok(())
    }

    #[test]
    fn default_base_path_matches_the_public_route() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            default_base_path(&SandboxId::parse("sbx_1")?),
            "/s/sbx_1/editor"
        );
        Ok(())
    }
}
