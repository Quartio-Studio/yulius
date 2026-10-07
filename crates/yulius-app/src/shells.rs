//! Sandbox shells: a general login shell per request in the sandbox, separate from every agent
//! terminal (SPEC-001 UI-015, AC-UI-7, U-11; SPEC-002 §7.2).
//!
//! Contract:
//! - only users with write access to the sandbox create shells, and a shell is private to the
//!   user who opened it: nobody else (administrators included) can attach to it (UI-016);
//! - a shell is the user's login shell (`$SHELL`, else `/bin/bash`, else `/bin/sh`, unless
//!   [`App::set_shell_program`] configured one) started through the session host in the sandbox
//!   repository or a task worktree of that sandbox, as the user's sandbox identity
//!   ([`App::user_space`]: their own home, root in the sandbox, Git as their GitHub account); it
//!   gets no task token or hooks;
//! - at most [`MAX_SHELLS_PER_SANDBOX`] live shells per user and sandbox and
//!   [`MAX_SHELL_VIEWERS`] viewers per shell;
//! - a shell ends when its process exits, after [`SHELL_IDLE_MS`] without viewers
//!   ([`App::reap_idle_shells`]) or on server shutdown ([`App::stop_workspace_services`]);
//! - shells never read or write task state (HRN-002): the task terminal stays the agent's.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::{Hub, Subscription};
use crate::identity::UserSpace;
use crate::model::SandboxRecord;
use crate::ports::Clock;
use crate::ports::session::{LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunWriter};

/// Live shells allowed per user and sandbox.
pub const MAX_SHELLS_PER_SANDBOX: usize = 4;
/// Viewers allowed per shell.
pub const MAX_SHELL_VIEWERS: u64 = 16;
/// A shell without viewers for this long is ended.
pub const SHELL_IDLE_MS: i64 = 30 * 60 * 1000;
/// Largest accepted input frame.
pub const MAX_SHELL_INPUT_BYTES: usize = 64 * 1024;
/// Interval of the idle reaper for shells and browsers.
pub const REAP_INTERVAL: Duration = Duration::from_secs(60);
/// Grace per escalation step of a shutdown request (Ctrl-C, then SIGTERM, then SIGKILL;
/// interactive shells ignore SIGTERM, so ending a shell takes about two steps).
const SHUTDOWN_GRACE_MS: u32 = 500;
/// Wait for the exit of a shell that was asked to stop before it is terminated.
const EXIT_WAIT: Duration = Duration::from_secs(10);
/// Reattach attempts after an unexpected host disconnect.
const REATTACH_ATTEMPTS: u32 = 3;
/// Initial PTY size before the first viewer reports its size.
const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// Orphaned shell directories inspected at startup.
const MAX_ORPHANS: usize = 1024;

/// Registry of live shells (process-local; shells do not survive a server restart).
#[derive(Default)]
pub struct ShellState {
    shells: Mutex<HashMap<String, Arc<ShellRuntime>>>,
    program: OnceLock<PathBuf>,
}

/// One live shell.
pub struct ShellRuntime {
    id: String,
    sandbox: SandboxId,
    creator: UserId,
    run_dir: PathBuf,
    token: [u8; 32],
    hub: Hub,
    writer: tokio::sync::Mutex<Option<Box<dyn RunWriter>>>,
    ended: AtomicBool,
    ended_tx: watch::Sender<bool>,
    /// Clock time (ms) when the last viewer left; meaningful only without viewers.
    idle_since_ms: AtomicI64,
}

impl ShellRuntime {
    /// Shell id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Owning sandbox.
    #[must_use]
    pub fn sandbox(&self) -> &SandboxId {
        &self.sandbox
    }

    /// The user who opened the shell.
    #[must_use]
    pub fn creator(&self) -> &UserId {
        &self.creator
    }

    /// Whether the shell has ended.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Visible screen text (diagnostics and tests).
    #[must_use]
    pub fn screen_text(&self) -> String {
        self.hub.screen_text()
    }

    /// Connected viewers.
    #[must_use]
    pub fn viewer_count(&self) -> u64 {
        self.hub.viewer_count()
    }
}

/// An attached shell viewer; dropping it detaches (and starts the idle timer when it was the
/// last viewer).
pub struct ShellAttach {
    /// Shell id.
    pub shell_id: String,
    /// Snapshot + live frames.
    pub subscription: Subscription,
    runtime: Arc<ShellRuntime>,
    clock: Arc<dyn Clock>,
}

impl ShellAttach {
    /// PTY size (cols, rows).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        self.runtime.hub.size()
    }

    /// Whether the shell has ended.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.runtime.has_ended()
    }
}

impl Drop for ShellAttach {
    fn drop(&mut self) {
        self.runtime.hub.unsubscribe();
        if self.runtime.hub.viewer_count() == 0 {
            self.runtime
                .idle_since_ms
                .store(self.clock.now().0, Ordering::SeqCst);
        }
    }
}

/// Background reaper of idle shells and browsers; aborted on drop.
pub struct WorkspaceReaper {
    task: JoinHandle<()>,
}

impl Drop for WorkspaceReaper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn shell_dir_root(data_dir: &Path) -> PathBuf {
    data_dir.join("shells")
}

impl App {
    /// Configures the login shell for sandbox shells (composition and tests); the first call
    /// wins. Returns whether it was applied.
    pub fn set_shell_program(&self, program: PathBuf) -> bool {
        self.shells.program.set(program).is_ok()
    }

    /// The configured shell, else `$SHELL` when it is an absolute executable, else
    /// `/bin/bash`, else `/bin/sh`.
    #[must_use]
    pub fn shell_program(&self) -> PathBuf {
        if let Some(program) = self.shells.program.get() {
            return program.clone();
        }
        if let Some(shell) = std::env::var_os("SHELL").map(PathBuf::from)
            && shell.is_absolute()
            && is_executable(&shell)
        {
            return shell;
        }
        ["/bin/bash", "/bin/sh"]
            .into_iter()
            .map(PathBuf::from)
            .find(|candidate| is_executable(candidate))
            .unwrap_or_else(|| PathBuf::from("/bin/sh"))
    }

    fn shell(&self, id: &str) -> Option<Arc<ShellRuntime>> {
        self.shells
            .shells
            .lock()
            .ok()
            .and_then(|map| map.get(id).cloned())
            .filter(|shell| !shell.has_ended())
    }

    /// Live shells of a sandbox.
    #[must_use]
    pub fn sandbox_shells(&self, sandbox: &SandboxId) -> Vec<Arc<ShellRuntime>> {
        self.shells
            .shells
            .lock()
            .map(|map| {
                map.values()
                    .filter(|shell| &shell.sandbox == sandbox && !shell.has_ended())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn shell_env(
        &self,
        sandbox: &SandboxRecord,
        space: &UserSpace,
        program: &Path,
    ) -> Vec<(String, String)> {
        let mut env = space.env.clone();
        env.extend([
            ("SHELL".into(), program.to_string_lossy().into_owned()),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("YULIUS_PORT_BASE".into(), sandbox.port_base.to_string()),
            ("YULIUS_PORT_COUNT".into(), sandbox.port_count.to_string()),
            ("PORT".into(), sandbox.port_base.to_string()),
            ("YULIUS_SANDBOX".into(), sandbox.slug.clone()),
        ]);
        env
    }

    /// Opens a sandbox shell for a write user in the sandbox repository or the worktree of
    /// `task` (a task of that sandbox) and returns its id.
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access or for foreign tasks, `Conflict` when the
    /// folder does not exist yet, `Capacity` beyond [`MAX_SHELLS_PER_SANDBOX`], `Upstream` when
    /// the session host cannot start the shell.
    pub async fn shell_create(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
        task: Option<&str>,
    ) -> AppResult<String> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let cwd = match task {
            Some(task) => self.sandbox_worktree(&record, task).await?,
            None => {
                let repo = self.sandbox_repo(&record.id);
                if !repo.is_dir() {
                    return Err(AppError::Conflict(
                        "the sandbox repository is not cloned yet".into(),
                    ));
                }
                repo
            }
        };
        let random = self.deps.crypto.random_bytes();
        let id = hex_token(random.get(..8).unwrap_or(&random));
        let runtime = Arc::new(ShellRuntime {
            id: id.clone(),
            sandbox: record.id.clone(),
            creator: ctx.user.id.clone(),
            run_dir: shell_dir_root(&self.config.data_dir).join(&id),
            token: self.deps.crypto.random_bytes(),
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            writer: tokio::sync::Mutex::new(None),
            ended: AtomicBool::new(false),
            ended_tx: watch::channel(false).0,
            idle_since_ms: AtomicI64::new(self.now().0),
        });
        {
            let mut map = self.shells.shells.lock().map_err(AppError::internal)?;
            let live = map
                .values()
                .filter(|shell| {
                    shell.sandbox == record.id && shell.creator == ctx.user.id && !shell.has_ended()
                })
                .count();
            if live >= MAX_SHELLS_PER_SANDBOX {
                return Err(AppError::Capacity(format!(
                    "at most {MAX_SHELLS_PER_SANDBOX} shells per sandbox; close one first"
                )));
            }
            map.insert(id.clone(), Arc::clone(&runtime));
        }
        let space = self.user_space(&ctx.user, &record)?;
        match self.launch_shell(&record, &space, &runtime, &cwd).await {
            Ok(reader) => {
                let app = Arc::clone(self);
                let shell = Arc::clone(&runtime);
                tokio::spawn(async move { app.pump_shell(shell, reader).await });
                tracing::info!(shell = %id, sandbox = %record.slug, user = %ctx.user.id, "sandbox shell started");
                Ok(id)
            }
            Err(err) => {
                runtime.ended.store(true, Ordering::SeqCst);
                if let Ok(mut map) = self.shells.shells.lock() {
                    map.remove(&id);
                }
                let _ = self.deps.session_host.terminate(&runtime.run_dir);
                let _ = std::fs::remove_dir_all(&runtime.run_dir);
                Err(err)
            }
        }
    }

    async fn launch_shell(
        &self,
        record: &SandboxRecord,
        space: &UserSpace,
        runtime: &ShellRuntime,
        cwd: &Path,
    ) -> AppResult<Box<dyn RunReader>> {
        std::fs::create_dir_all(&runtime.run_dir).map_err(AppError::internal)?;
        let _ = std::fs::set_permissions(&runtime.run_dir, std::fs::Permissions::from_mode(0o700));
        let program = self.shell_program();
        let plan = LaunchPlan {
            argv: vec![program.to_string_lossy().into_owned(), "-l".into()],
            cwd: cwd.to_path_buf(),
            env: self.shell_env(record, space, &program),
            secret_env: Vec::new(),
            files: Vec::new(),
            containment: space.containment.clone(),
        };
        let host = &self.deps.session_host;
        host.spawn(
            &runtime.run_dir,
            &runtime.id,
            &runtime.id,
            &plan,
            &runtime.token,
            DEFAULT_SIZE,
        )
        .await?;
        let attached = host.attach(&runtime.run_dir, &runtime.token).await?;
        let welcome = attached.welcome;
        runtime.hub.reset(
            welcome.seq,
            welcome.snapshot.as_deref().unwrap_or_default(),
            welcome.cols,
            welcome.rows,
        );
        *runtime.writer.lock().await = Some(attached.writer);
        Ok(attached.reader)
    }

    async fn pump_shell(
        self: Arc<Self>,
        runtime: Arc<ShellRuntime>,
        mut reader: Box<dyn RunReader>,
    ) {
        let mut reattaches = 0;
        let exit = loop {
            match reader.next().await {
                Ok(Some(RunEvent::Output { seq, bytes })) => runtime.hub.output(seq, &bytes),
                Ok(Some(RunEvent::Snapshot { seq, bytes })) => {
                    let (cols, rows) = runtime.hub.size();
                    runtime.hub.reset(seq, &bytes, cols, rows);
                }
                Ok(Some(RunEvent::Exit(exit))) => break Some(exit),
                Ok(None) | Err(_) => {
                    if reattaches < REATTACH_ATTEMPTS && !runtime.has_ended() {
                        reattaches += 1;
                        tokio::time::sleep(Duration::from_millis(200 * u64::from(reattaches)))
                            .await;
                        if let Ok(again) = self
                            .deps
                            .session_host
                            .attach(&runtime.run_dir, &runtime.token)
                            .await
                        {
                            runtime.hub.reset(
                                again.welcome.seq,
                                again.welcome.snapshot.as_deref().unwrap_or_default(),
                                again.welcome.cols,
                                again.welcome.rows,
                            );
                            *runtime.writer.lock().await = Some(again.writer);
                            reader = again.reader;
                        }
                        continue;
                    }
                    break match self.deps.session_host.inspect(&runtime.run_dir) {
                        RunInspection::Exited(exit) => Some(exit),
                        RunInspection::Recorded {
                            host_alive: true, ..
                        } => {
                            let _ = self.deps.session_host.terminate(&runtime.run_dir);
                            None
                        }
                        _ => None,
                    };
                }
            }
        };
        self.finish_shell(&runtime, exit).await;
    }

    /// Marks a shell ended exactly once: tells viewers, forgets it and removes its run
    /// directory.
    async fn finish_shell(&self, runtime: &ShellRuntime, exit: Option<RunExit>) {
        if runtime.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        *runtime.writer.lock().await = None;
        runtime.hub.exit("closed", exit.and_then(|e| e.code));
        runtime.ended_tx.send_replace(true);
        if let Ok(mut map) = self.shells.shells.lock() {
            map.remove(&runtime.id);
        }
        let _ = std::fs::remove_dir_all(&runtime.run_dir);
        tracing::info!(shell = %runtime.id, sandbox = %runtime.sandbox, code = ?exit.and_then(|e| e.code), "sandbox shell ended");
    }

    /// Asks a shell to exit; the host escalates to signals after a grace period.
    async fn request_shell_exit(runtime: &ShellRuntime) {
        if let Some(writer) = runtime.writer.lock().await.as_mut() {
            let _ = writer.shutdown(SHUTDOWN_GRACE_MS).await;
        }
    }

    /// Waits until `deadline` for a shell's exit, then terminates its host and records the end
    /// (a timeout is not proof of exit, so the host is killed rather than forgotten).
    async fn await_shell_exit(&self, runtime: &ShellRuntime, deadline: tokio::time::Instant) {
        let mut ended = runtime.ended_tx.subscribe();
        let exited = tokio::time::timeout_at(deadline, ended.wait_for(|ended| *ended))
            .await
            .is_ok_and(|result| result.is_ok());
        if !exited {
            tracing::warn!(shell = %runtime.id, "shell did not exit after shutdown; terminating its host");
            let _ = self.deps.session_host.terminate(&runtime.run_dir);
            self.finish_shell(runtime, None).await;
        }
    }

    /// Ends shells: every shell is asked first, then all are awaited against one deadline.
    async fn end_shells(&self, shells: &[Arc<ShellRuntime>]) {
        for shell in shells {
            Self::request_shell_exit(shell).await;
        }
        let deadline = tokio::time::Instant::now() + EXIT_WAIT;
        for shell in shells {
            self.await_shell_exit(shell, deadline).await;
        }
    }

    /// Attaches a viewer to a shell: only its creator, while they keep write access to the
    /// shell's sandbox (UI-016).
    ///
    /// # Errors
    /// `NotFound` for unknown or ended shells, other users' shells and invisible sandboxes,
    /// `Forbidden` for read users, `Capacity` beyond [`MAX_SHELL_VIEWERS`].
    pub async fn shell_open(&self, ctx: &AuthContext, shell_id: &str) -> AppResult<ShellAttach> {
        let runtime = self
            .shell(shell_id)
            .filter(|shell| shell.creator == ctx.user.id)
            .ok_or(AppError::NotFound)?;
        self.require(&ctx.user, &runtime.sandbox, Action::Work)
            .await?;
        if runtime.hub.viewer_count() >= MAX_SHELL_VIEWERS {
            return Err(AppError::Capacity("too many viewers on this shell".into()));
        }
        let subscription = runtime.hub.subscribe();
        Ok(ShellAttach {
            shell_id: runtime.id.clone(),
            subscription,
            runtime,
            clock: Arc::clone(&self.deps.clock),
        })
    }

    /// Whether the viewer may still use the shell (re-checked on input and periodically).
    pub async fn shell_still_allowed(&self, ctx: &AuthContext, attach: &ShellAttach) -> bool {
        ctx.user.disabled_at.is_none()
            && self
                .require(&ctx.user, &attach.runtime.sandbox, Action::Work)
                .await
                .is_ok()
    }

    /// Viewer keyboard input.
    ///
    /// # Errors
    /// `Forbidden` after access was revoked, `Invalid` for oversized frames, `Conflict` once
    /// the shell ended.
    pub async fn shell_input(
        &self,
        ctx: &AuthContext,
        attach: &ShellAttach,
        data: &[u8],
    ) -> AppResult<()> {
        if data.len() > MAX_SHELL_INPUT_BYTES {
            return Err(AppError::Invalid("input frame too large".into()));
        }
        if !self.shell_still_allowed(ctx, attach).await {
            return Err(AppError::Forbidden("write access is required".into()));
        }
        match attach.runtime.writer.lock().await.as_mut() {
            Some(writer) => writer.input(data).await,
            None => Err(AppError::Conflict("the shell has ended".into())),
        }
    }

    /// Viewer resize (the last resizing viewer controls the size).
    ///
    /// # Errors
    /// As [`Self::shell_input`].
    pub async fn shell_resize(
        &self,
        ctx: &AuthContext,
        attach: &ShellAttach,
        cols: u16,
        rows: u16,
    ) -> AppResult<()> {
        if !self.shell_still_allowed(ctx, attach).await {
            return Err(AppError::Forbidden("write access is required".into()));
        }
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(5, 500));
        match attach.runtime.writer.lock().await.as_mut() {
            Some(writer) => writer.resize(cols, rows).await?,
            None => return Err(AppError::Conflict("the shell has ended".into())),
        }
        attach.runtime.hub.resize(cols, rows);
        Ok(())
    }

    /// Ends shells that have had no viewer for [`SHELL_IDLE_MS`] (by the injected clock);
    /// returns how many were ended.
    pub async fn reap_idle_shells(&self) -> usize {
        let now = self.now().0;
        let idle: Vec<Arc<ShellRuntime>> = self
            .shells
            .shells
            .lock()
            .map(|map| {
                map.values()
                    .filter(|shell| {
                        !shell.has_ended()
                            && shell.hub.viewer_count() == 0
                            && now.saturating_sub(shell.idle_since_ms.load(Ordering::SeqCst))
                                >= SHELL_IDLE_MS
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for shell in &idle {
            tracing::info!(shell = %shell.id, "ending a shell without viewers");
        }
        self.end_shells(&idle).await;
        idle.len()
    }

    /// Terminates shell hosts left behind by a previous server process and removes their
    /// directories (shells are not resumed after a restart).
    pub fn reap_orphan_shells(&self) -> usize {
        let root = shell_dir_root(&self.config.data_dir);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        let live: Vec<String> = self
            .shells
            .shells
            .lock()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let mut reaped = 0;
        for entry in entries.flatten().take(MAX_ORPHANS) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if live.contains(&name) || !path.is_dir() {
                continue;
            }
            if let RunInspection::Recorded {
                host_alive: true, ..
            } = self.deps.session_host.inspect(&path)
            {
                let _ = self.deps.session_host.terminate(&path);
            }
            let _ = std::fs::remove_dir_all(&path);
            reaped += 1;
        }
        reaped
    }

    /// Removes shells orphaned by a previous server process, then starts the periodic reaper of
    /// idle shells and browsers; call once at startup, before serving requests. The
    /// reaper stops when the handle is dropped or the app is gone.
    #[must_use]
    pub fn start_workspace_services(self: &Arc<Self>) -> WorkspaceReaper {
        // Before any shell of this process exists, so no live shell directory is touched.
        let orphans = self.reap_orphan_shells();
        if orphans > 0 {
            tracing::info!(orphans, "removed shells of a previous server process");
        }
        let app: Weak<Self> = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(REAP_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(app) = app.upgrade() else {
                    return;
                };
                app.reap_idle_shells().await;
                for sandbox in app.deps.browser.reap_idle().await {
                    tracing::info!(sandbox = %sandbox, "stopped an idle sandbox browser");
                }
            }
        });
        WorkspaceReaper { task }
    }

    /// Server shutdown: ends every shell and stops every browser (their processes
    /// are reaped; unlike agent sessions they are not meant to outlive the server).
    pub async fn stop_workspace_services(&self) {
        let shells: Vec<Arc<ShellRuntime>> = self
            .shells
            .shells
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default();
        self.end_shells(&shells).await;
        let browsers = self.deps.browser.shutdown_all().await;
        if !browsers.is_empty() {
            tracing::error!(?browsers, "some browser processes could not be reaped");
        }
    }
}
