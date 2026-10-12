//! Sandbox shells: the user's own terminals in a sandbox, separate from every agent terminal
//! (SPEC-001 UI-015, UI-016, AC-UI-7, AC-UI-8, U-11; SPEC-002 §7.2).
//!
//! Contract:
//! - only users with write access to the sandbox create shells, and a shell is private to the
//!   user who opened it: nobody else (administrators included) can list, attach to, rename or
//!   close it, and it belongs to one sandbox (UI-016);
//! - a user may keep several shells per sandbox (tabs in the web UI), at most
//!   [`MAX_SHELLS_PER_SANDBOX`] live ones, each with a name ("Terminal 1" by default) that
//!   only its owner sees and can change; [`MAX_SHELL_VIEWERS`] viewers per shell;
//! - a shell is the user's login shell (`$SHELL`, else `/bin/bash`, else `/bin/sh`, unless
//!   [`App::set_shell_program`] configured one) started through the session host in the sandbox
//!   repository or a task worktree of that sandbox, as the user's sandbox identity
//!   ([`App::user_space`]: their own home, root in the sandbox, Git as their GitHub account); it
//!   gets no task token or hooks;
//! - shells outlive the server like agent sessions: their detached hosts keep running across a
//!   restart, and the next server process adopts them from their run directory
//!   (`shells/<id>/shell.json`: owner, sandbox, name and the sealed host token) at start
//!   ([`App::start_workspace_services`]); a run directory it cannot adopt is ended and removed;
//! - a shell ends when its process exits, when its owner closes it ([`App::shell_close`]) or
//!   after [`SHELL_IDLE_MS`] without viewers ([`App::reap_idle_shells`]);
//! - shells never read or write task state (HRN-002): the task terminal stays the agent's.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::{Hub, Subscription};
use crate::identity::UserSpace;
use crate::model::{SandboxRecord, Sealed};
use crate::ports::Clock;

/// Prefix of a Cockpit's Yulius MCP bearer (SPEC-037).
pub const COCKPIT_TOKEN_PREFIX: &str = "ck_";
/// What a Cockpit's bearer is derived for, hashed with its host token.
const COCKPIT_TOKEN_LABEL: &[u8] = b"yulius-cockpit-mcp\0";
use crate::ports::session::{LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunWriter};

/// Live shells allowed per user and sandbox.
pub const MAX_SHELLS_PER_SANDBOX: usize = 8;
/// Viewers allowed per shell.
pub const MAX_SHELL_VIEWERS: u64 = 16;
/// A shell without viewers for this long is ended (a day: a terminal outlives a closed laptop
/// and a night, like a local terminal tab, but not a forgotten week).
pub const SHELL_IDLE_MS: i64 = 24 * 60 * 60 * 1000;
/// Longest shell name, in characters.
pub const MAX_SHELL_NAME_CHARS: usize = 40;
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
/// bash's `PROMPT_COMMAND` in sandbox shells: OSC 7 (`ESC ] 7 ; file://host/cwd ESC \`) at
/// every prompt, the working directory the web terminal resolves printed paths against.
pub const OSC7_PROMPT_COMMAND: &str = r#"printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-}" "$PWD""#;
/// Shell run directories inspected at startup.
const MAX_ORPHANS: usize = 1024;
/// The shell's record in its run directory.
const SHELL_FILE: &str = "shell.json";
/// Version of [`ShellFile`].
const SHELL_FILE_VERSION: u32 = 1;
/// Time allowed to reattach to one surviving shell at startup.
const ADOPT_TIMEOUT: Duration = Duration::from_secs(5);

/// What runs in a shell: the user's login shell (a terminal), or their Cockpit, a plain Claude
/// Code session that is not a terminal (SPEC-021: never listed, renamed or closed as one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellKind {
    /// A terminal tab of the Shell panel.
    #[default]
    Terminal,
    /// The user's Cockpit ([`crate::cockpit`]).
    Cockpit,
}

/// Registry of live shells. The registry is process-local; the shells themselves survive a
/// server restart and are adopted again from their run directories.
#[derive(Default)]
pub struct ShellState {
    shells: Mutex<HashMap<String, Arc<ShellRuntime>>>,
    program: OnceLock<PathBuf>,
    /// The Cockpit's program instead of `claude` (tests).
    pub(crate) cockpit_program: OnceLock<PathBuf>,
    /// Serialises the Cockpit starts of one user in one sandbox (one Cockpit each); other users
    /// and sandboxes never wait for them.
    #[allow(clippy::type_complexity)]
    pub(crate) cockpit_start: Mutex<HashMap<(UserId, SandboxId), Arc<tokio::sync::Mutex<()>>>>,
    /// The conversation each user's Cockpit could not resume (it exited at once), by sandbox:
    /// the next start begins a fresh one instead of failing the same way.
    pub(crate) cockpit_unresumable: Mutex<HashMap<(UserId, SandboxId), String>>,
    /// The next creation number (tab order among shells created in the same millisecond).
    next_seq: AtomicU64,
}

/// One live shell.
pub struct ShellRuntime {
    id: String,
    pub(crate) kind: ShellKind,
    /// The conversation a Cockpit was started to resume, and when its process was spawned.
    pub(crate) resumed: OnceLock<(String, Timestamp)>,
    sandbox: SandboxId,
    creator: UserId,
    task: Option<TaskId>,
    /// The folder it started in (the sandbox repository or the task's worktree), as its
    /// processes see it.
    folder: PathBuf,
    name: Mutex<String>,
    created_at: Timestamp,
    /// Creation number, persisted with the shell (stable tab order).
    seq: u64,
    run_dir: PathBuf,
    token: [u8; 32],
    hub: Hub,
    writer: tokio::sync::Mutex<Option<Box<dyn RunWriter>>>,
    ended: AtomicBool,
    /// Its owner closed it: hidden and refused while it ends.
    closing: AtomicBool,
    ended_tx: watch::Sender<bool>,
    /// Clock time (ms) when the last viewer left; meaningful only without viewers.
    idle_since_ms: AtomicI64,
    /// The task that forwards the host's output; aborted when the server lets go of the shell.
    pump: Mutex<Option<JoinHandle<()>>>,
}

/// A shell as its owner sees it (`GET /api/sandboxes/{sid}/shells`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellInfo {
    /// Shell id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The task whose worktree it started in; `None` for the sandbox repository.
    pub task_id: Option<String>,
    /// Absolute path of the folder it started in, as its processes see it (file links in its
    /// output resolve against it, F 2026-10-10).
    pub folder: String,
    /// Creation time.
    pub created_at: Timestamp,
}

/// `shells/<id>/shell.json`: what a later server process needs to adopt a surviving shell.
/// Owner-only (0600) inside an owner-only run directory; the host token is sealed with the
/// instance master key, like an agent attempt's. (No `Debug`: records stay out of logs.)
#[derive(Serialize, Deserialize)]
struct ShellFile {
    version: u32,
    id: String,
    sandbox: String,
    creator: String,
    task: Option<String>,
    name: String,
    created_at: i64,
    #[serde(default)]
    seq: u64,
    /// Terminal or Cockpit (absent in records written before the Cockpit: a terminal).
    #[serde(default)]
    kind: ShellKind,
    /// Sealed host token, lowercase hex.
    token: String,
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

    /// Display name.
    #[must_use]
    pub fn name(&self) -> String {
        self.name.lock().map(|n| n.clone()).unwrap_or_default()
    }

    /// Whether the shell has ended.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// The Cockpit's bearer for the Yulius MCP server (SPEC-037 MGT-005): `ck_` and the SHA-256
    /// of a fixed label and the host token. The host token is persisted with the shell, so the
    /// bearer survives a server restart with its Cockpit; the hash reveals nothing about it.
    pub(crate) fn cockpit_token(&self, crypto: &dyn crate::ports::Crypto) -> String {
        let mut data = COCKPIT_TOKEN_LABEL.to_vec();
        data.extend_from_slice(&self.token);
        format!("{COCKPIT_TOKEN_PREFIX}{}", crypto.sha256_hex(&data))
    }

    /// Listed and attachable: not ended and not being closed.
    pub(crate) fn is_live(&self) -> bool {
        !self.has_ended() && !self.closing.load(Ordering::SeqCst)
    }

    pub(crate) fn info(&self) -> ShellInfo {
        ShellInfo {
            id: self.id.clone(),
            name: self.name(),
            task_id: self.task.as_ref().map(|t| t.as_str().to_owned()),
            folder: self.folder.to_string_lossy().into_owned(),
            created_at: self.created_at,
        }
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

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        })
        .collect()
}

/// A valid shell name: trimmed, 1..=[`MAX_SHELL_NAME_CHARS`] characters, no control characters.
fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_SHELL_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(format!(
            "a shell name has 1 to {MAX_SHELL_NAME_CHARS} characters and no control characters"
        )));
    }
    Ok(name.to_owned())
}

/// Writes `shell.json` through a temporary file, an fsync and a rename (SPEC-002 §4), 0600.
fn write_shell_file(run_dir: &Path, file: &ShellFile) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let bytes = serde_json::to_vec(file).map_err(std::io::Error::other)?;
    let tmp = run_dir.join(format!("{SHELL_FILE}.tmp"));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    out.write_all(&bytes)?;
    out.sync_all()?;
    std::fs::rename(&tmp, run_dir.join(SHELL_FILE))
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
            .filter(|shell| shell.is_live())
    }

    /// Every live Cockpit of the instance.
    pub(crate) fn live_cockpits(&self) -> Vec<Arc<ShellRuntime>> {
        self.shells
            .shells
            .lock()
            .map(|map| {
                map.values()
                    .filter(|shell| shell.kind == ShellKind::Cockpit && shell.is_live())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Live shells of a sandbox (every user's; diagnostics and tests).
    #[must_use]
    pub fn sandbox_shells(&self, sandbox: &SandboxId) -> Vec<Arc<ShellRuntime>> {
        self.shells
            .shells
            .lock()
            .map(|map| {
                map.values()
                    .filter(|shell| &shell.sandbox == sandbox && shell.is_live())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The caller's live shells in `sandbox`, oldest first.
    fn own_shells(&self, user: &UserId, sandbox: &SandboxId) -> Vec<Arc<ShellRuntime>> {
        let mut own: Vec<Arc<ShellRuntime>> = self
            .sandbox_shells(sandbox)
            .into_iter()
            .filter(|shell| &shell.creator == user && shell.kind == ShellKind::Terminal)
            .collect();
        own.sort_by(|a, b| (a.created_at, a.seq, &a.id).cmp(&(b.created_at, b.seq, &b.id)));
        own
    }

    /// The caller's live terminal `id` of `sandbox` (write access required); never their
    /// Cockpit, which cannot be renamed or closed (SPEC-021 CKP-004).
    ///
    /// # Errors
    /// `NotFound` for unknown, ended or closing shells, Cockpits, other users' shells and shells
    /// of another sandbox; access errors.
    pub(crate) async fn own_shell(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
    ) -> AppResult<Arc<ShellRuntime>> {
        self.own_shell_of(ctx, sandbox, id, Some(ShellKind::Terminal))
            .await
    }

    /// The caller's live shell `id` of `sandbox`, of `kind` when given (write access required).
    ///
    /// # Errors
    /// As [`Self::own_shell`].
    pub(crate) async fn own_shell_of(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
        kind: Option<ShellKind>,
    ) -> AppResult<Arc<ShellRuntime>> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        self.shell(id)
            .filter(|shell| {
                shell.creator == ctx.user.id
                    && shell.sandbox == sandbox_id
                    && kind.is_none_or(|kind| shell.kind == kind)
            })
            .ok_or(AppError::NotFound)
    }

    pub(crate) fn shell_env(
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
            // bash tells the terminal its working directory at every prompt (OSC 7), so paths
            // printed after a `cd` still link to the right files (F 2026-10-10). Other shells
            // ignore it; a user's own PROMPT_COMMAND replaces it.
            ("PROMPT_COMMAND".into(), OSC7_PROMPT_COMMAND.into()),
        ]);
        env
    }

    /// The caller's live shells in a sandbox, oldest first (write access; UI-016: only their
    /// own, never another user's or another sandbox's).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access.
    pub async fn shell_list(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<Vec<ShellInfo>> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        Ok(self
            .own_shells(&ctx.user.id, &sandbox_id)
            .iter()
            .map(|shell| shell.info())
            .collect())
    }

    /// Opens a sandbox shell for a write user in the sandbox repository or the worktree of
    /// `task` (a task of that sandbox), named `name` or "Terminal N" (the lowest number free
    /// among the caller's shells there).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access or for foreign tasks, `Invalid` for a bad
    /// name, `Conflict` when the folder does not exist yet, `Capacity` beyond
    /// [`MAX_SHELLS_PER_SANDBOX`], `Upstream` when the session host cannot start the shell.
    pub async fn shell_create(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
        task: Option<&str>,
        name: Option<&str>,
    ) -> AppResult<ShellInfo> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let name = name.map(clean_name).transpose()?;
        let (cwd, task_id) = match task {
            Some(task) => (
                self.sandbox_worktree(&record, task).await?,
                Some(TaskId::parse(task).map_err(|_| AppError::NotFound)?),
            ),
            None => {
                let repo = self.sandbox_repo(&record.id);
                if !repo.is_dir() {
                    return Err(AppError::Conflict(
                        "the sandbox repository is not cloned yet".into(),
                    ));
                }
                (repo, None)
            }
        };
        let id = self.new_shell_id();
        let runtime = {
            let mut map = self.shells.shells.lock().map_err(AppError::internal)?;
            let own: Vec<String> = map
                .values()
                .filter(|shell| {
                    shell.sandbox == record.id
                        && shell.creator == ctx.user.id
                        && shell.kind == ShellKind::Terminal
                        && shell.is_live()
                })
                .map(|shell| shell.name())
                .collect();
            if own.len() >= MAX_SHELLS_PER_SANDBOX {
                return Err(AppError::Capacity(format!(
                    "at most {MAX_SHELLS_PER_SANDBOX} terminals per sandbox; close one first"
                )));
            }
            let name = name.unwrap_or_else(|| {
                (1..)
                    .map(|n| format!("Terminal {n}"))
                    .find(|candidate| !own.contains(candidate))
                    .unwrap_or_else(|| "Terminal".into())
            });
            let runtime = self.new_shell_runtime(
                &id,
                ShellKind::Terminal,
                &record.id,
                &ctx.user.id,
                task_id,
                &cwd,
                name,
            );
            map.insert(id.clone(), Arc::clone(&runtime));
            runtime
        };
        let launched = match self.isolation_join(&record).await {
            Ok(joined) => {
                let (join, lease) = joined.unzip();
                match self.user_space(&ctx.user, &record, join) {
                    Ok(space) => {
                        let launched = self.launch_shell(&record, &space, &runtime, &cwd).await;
                        drop(lease);
                        match launched {
                            Ok(reader) => self.save_shell(&runtime).map(|()| reader),
                            Err(err) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            Err(err) => Err(err),
        };
        match launched {
            Ok(reader) => {
                self.start_pump(&runtime, reader);
                tracing::info!(shell = %id, sandbox = %record.slug, user = %ctx.user.id, "sandbox shell started");
                Ok(runtime.info())
            }
            Err(err) => {
                self.discard_shell(&runtime);
                Err(err)
            }
        }
    }

    /// Renames one of the caller's shells.
    ///
    /// # Errors
    /// `NotFound` for unknown, ended or closing shells, other users' shells and shells of
    /// another sandbox; access errors; `Invalid` for a bad name; `Internal` when the record
    /// cannot be written.
    pub async fn shell_rename(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
        name: &str,
    ) -> AppResult<ShellInfo> {
        let name = clean_name(name)?;
        let shell = self.own_shell(ctx, sandbox, id).await?;
        let previous = {
            let mut current = shell.name.lock().map_err(AppError::internal)?;
            std::mem::replace(&mut *current, name)
        };
        if let Err(err) = self.save_shell(&shell) {
            if let Ok(mut current) = shell.name.lock() {
                *current = previous;
            }
            return Err(err);
        }
        Ok(shell.info())
    }

    /// Closes one of the caller's shells: it disappears from the list at once and its process
    /// is ended in the background (Ctrl-C, SIGTERM, then SIGKILL; the host is terminated if it
    /// does not exit).
    ///
    /// # Errors
    /// `NotFound` for unknown, ended or closing shells, other users' shells and shells of
    /// another sandbox; access errors.
    pub async fn shell_close(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
        id: &str,
    ) -> AppResult<()> {
        let shell = self.own_shell(ctx, sandbox, id).await?;
        if shell.closing.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        // Viewers learn at once; the exit frame follows when the process is gone.
        shell.hub.notice("Closing this terminal…");
        let app = Arc::clone(self);
        tokio::spawn(async move { app.end_shells(&[shell]).await });
        Ok(())
    }

    /// Writes the shell's record for a later server process (see the module docs).
    pub(crate) fn save_shell(&self, runtime: &ShellRuntime) -> AppResult<()> {
        let sealed = self.deps.crypto.seal(&runtime.token)?;
        let file = ShellFile {
            version: SHELL_FILE_VERSION,
            id: runtime.id.clone(),
            sandbox: runtime.sandbox.as_str().to_owned(),
            creator: runtime.creator.as_str().to_owned(),
            task: runtime.task.as_ref().map(|t| t.as_str().to_owned()),
            name: runtime.name(),
            created_at: runtime.created_at.0,
            seq: runtime.seq,
            kind: runtime.kind,
            token: hex_token(&sealed.0),
        };
        write_shell_file(&runtime.run_dir, &file).map_err(AppError::internal)
    }

    /// A new random shell id (16 hex digits).
    pub(crate) fn new_shell_id(&self) -> String {
        let random = self.deps.crypto.random_bytes();
        hex_token(random.get(..8).unwrap_or(&random))
    }

    /// A shell runtime not started yet (the caller registers it, then spawns it).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_shell_runtime(
        &self,
        id: &str,
        kind: ShellKind,
        sandbox: &SandboxId,
        creator: &UserId,
        task: Option<TaskId>,
        cwd: &Path,
        name: String,
    ) -> Arc<ShellRuntime> {
        Arc::new(ShellRuntime {
            id: id.to_owned(),
            kind,
            resumed: OnceLock::new(),
            sandbox: sandbox.clone(),
            creator: creator.clone(),
            task,
            folder: std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf()),
            name: Mutex::new(name),
            created_at: self.now(),
            seq: self.shells.next_seq.fetch_add(1, Ordering::SeqCst),
            run_dir: shell_dir_root(&self.config.data_dir).join(id),
            token: self.deps.crypto.random_bytes(),
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            writer: tokio::sync::Mutex::new(None),
            ended: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            ended_tx: watch::channel(false).0,
            idle_since_ms: AtomicI64::new(self.now().0),
            pump: Mutex::new(None),
        })
    }

    /// Registers a started shell (a Cockpit; terminals register under the creation check).
    pub(crate) fn register_shell(&self, runtime: &Arc<ShellRuntime>) -> AppResult<()> {
        self.shells
            .shells
            .lock()
            .map_err(AppError::internal)?
            .insert(runtime.id.clone(), Arc::clone(runtime));
        Ok(())
    }

    /// Forgets a shell that failed to start and removes whatever it left.
    pub(crate) fn discard_shell(&self, runtime: &ShellRuntime) {
        runtime.ended.store(true, Ordering::SeqCst);
        if let Ok(mut map) = self.shells.shells.lock() {
            map.remove(&runtime.id);
        }
        let _ = self.deps.session_host.terminate(&runtime.run_dir);
        let _ = std::fs::remove_dir_all(&runtime.run_dir);
    }

    async fn launch_shell(
        &self,
        record: &SandboxRecord,
        space: &UserSpace,
        runtime: &ShellRuntime,
        cwd: &Path,
    ) -> AppResult<Box<dyn RunReader>> {
        let program = self.shell_program();
        let plan = LaunchPlan {
            argv: vec![program.to_string_lossy().into_owned(), "-l".into()],
            cwd: cwd.to_path_buf(),
            env: self.shell_env(record, space, &program),
            secret_env: Vec::new(),
            files: Vec::new(),
            containment: space.containment.clone(),
        };
        self.spawn_shell(runtime, &plan).await
    }

    /// Starts `plan` in the shell's own run directory and attaches to it.
    pub(crate) async fn spawn_shell(
        &self,
        runtime: &ShellRuntime,
        plan: &LaunchPlan,
    ) -> AppResult<Box<dyn RunReader>> {
        std::fs::create_dir_all(&runtime.run_dir).map_err(AppError::internal)?;
        let _ = std::fs::set_permissions(&runtime.run_dir, std::fs::Permissions::from_mode(0o700));
        let host = &self.deps.session_host;
        host.spawn(
            &runtime.run_dir,
            &runtime.id,
            &runtime.id,
            plan,
            &runtime.token,
            DEFAULT_SIZE,
        )
        .await?;
        self.attach_shell(runtime).await
    }

    /// Connects to the shell's host and loads its screen into the hub.
    async fn attach_shell(&self, runtime: &ShellRuntime) -> AppResult<Box<dyn RunReader>> {
        let attached = self
            .deps
            .session_host
            .attach(&runtime.run_dir, &runtime.token)
            .await?;
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

    pub(crate) fn start_pump(
        self: &Arc<Self>,
        runtime: &Arc<ShellRuntime>,
        reader: Box<dyn RunReader>,
    ) {
        let app = Arc::clone(self);
        let shell = Arc::clone(runtime);
        let task = tokio::spawn(async move { app.pump_shell(shell, reader).await });
        if let Ok(mut pump) = runtime.pump.lock() {
            *pump = Some(task);
        }
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
                        if let Ok(again) = self.attach_shell(&runtime).await {
                            reader = again;
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
        self.note_cockpit_exit(runtime, exit.and_then(|e| e.code));
        runtime.hub.exit("closed", exit.and_then(|e| e.code));
        runtime.ended_tx.send_replace(true);
        if let Ok(mut map) = self.shells.shells.lock() {
            map.remove(&runtime.id);
        }
        let _ = std::fs::remove_dir_all(&runtime.run_dir);
        self.remove_shell_attachments(&runtime.sandbox, &runtime.id);
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
    /// shell's sandbox (UI-016). `size` (cols, rows), the viewer's terminal, is applied before
    /// the viewer's snapshot is taken.
    ///
    /// # Errors
    /// `NotFound` for unknown, ended or closing shells, other users' shells and invisible
    /// sandboxes, `Forbidden` for read users, `Capacity` beyond [`MAX_SHELL_VIEWERS`].
    pub async fn shell_open(
        &self,
        ctx: &AuthContext,
        shell_id: &str,
        size: Option<(u16, u16)>,
    ) -> AppResult<ShellAttach> {
        let runtime = self
            .shell(shell_id)
            .filter(|shell| shell.creator == ctx.user.id)
            .ok_or(AppError::NotFound)?;
        self.require(&ctx.user, &runtime.sandbox, Action::Work)
            .await?;
        if runtime.hub.viewer_count() >= MAX_SHELL_VIEWERS {
            return Err(AppError::Capacity("too many viewers on this shell".into()));
        }
        if let Some((cols, rows)) = size {
            // A failed resize leaves the shell at its size; the viewer still attaches.
            let _ = Self::resize_shell(&runtime, cols, rows).await;
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
            && !attach.runtime.closing.load(Ordering::SeqCst)
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

    /// Viewer resize (the last resizing viewer controls the size); the same size again is a
    /// no-op, so a reconnect does not make the shell redraw.
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
        Self::resize_shell(&attach.runtime, cols, rows).await
    }

    async fn resize_shell(runtime: &ShellRuntime, cols: u16, rows: u16) -> AppResult<()> {
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(5, 500));
        if runtime.hub.size() == (cols, rows) {
            return Ok(());
        }
        match runtime.writer.lock().await.as_mut() {
            Some(writer) => writer.resize(cols, rows).await?,
            None => return Err(AppError::Conflict("the shell has ended".into())),
        }
        runtime.hub.resize(cols, rows);
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

    /// Adopts the shells a previous server process left running (their detached hosts survive
    /// a restart): each run directory with a valid record whose sandbox still exists and whose
    /// host accepts the sealed token is registered again with its screen; every other run
    /// directory is ended and removed. Returns (adopted, removed).
    pub async fn adopt_shells(self: &Arc<Self>) -> (usize, usize) {
        let root = shell_dir_root(&self.config.data_dir);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return (0, 0);
        };
        let live: Vec<String> = self
            .shells
            .shells
            .lock()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let mut attempts = Vec::new();
        for entry in entries.flatten().take(MAX_ORPHANS) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if live.contains(&name) || !path.is_dir() {
                continue;
            }
            let app = Arc::clone(self);
            attempts.push(tokio::spawn(async move {
                let adopted = matches!(
                    tokio::time::timeout(ADOPT_TIMEOUT, app.adopt_shell(&name, &path)).await,
                    Ok(Ok(()))
                );
                if !adopted {
                    if let RunInspection::Recorded {
                        host_alive: true, ..
                    } = app.deps.session_host.inspect(&path)
                    {
                        let _ = app.deps.session_host.terminate(&path);
                    }
                    let _ = std::fs::remove_dir_all(&path);
                }
                adopted
            }));
        }
        let (mut adopted, mut removed) = (0, 0);
        for attempt in attempts {
            if attempt.await.unwrap_or(false) {
                adopted += 1;
            } else {
                removed += 1;
            }
        }
        (adopted, removed)
    }

    /// The folder an adopted shell started in: its task's worktree, else the repository.
    async fn shell_folder(&self, sandbox: &SandboxId, task: Option<&TaskId>) -> PathBuf {
        let worktree = match task {
            Some(task) => self
                .deps
                .store
                .task(task)
                .await
                .ok()
                .flatten()
                .and_then(|t| t.worktree_path)
                .map(PathBuf::from),
            None => None,
        };
        let folder = worktree.unwrap_or_else(|| self.sandbox_repo(sandbox));
        std::fs::canonicalize(&folder).unwrap_or(folder)
    }

    async fn adopt_shell(self: &Arc<Self>, dir_name: &str, run_dir: &Path) -> AppResult<()> {
        let bytes = std::fs::read(run_dir.join(SHELL_FILE)).map_err(AppError::internal)?;
        let file: ShellFile = serde_json::from_slice(&bytes).map_err(AppError::internal)?;
        if file.version != SHELL_FILE_VERSION || file.id != dir_name {
            return Err(AppError::Invalid("unknown shell record".into()));
        }
        let sandbox = SandboxId::parse(&file.sandbox).map_err(|_| AppError::NotFound)?;
        self.deps
            .store
            .sandbox(&sandbox)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let sealed = hex_decode(&file.token).ok_or(AppError::NotFound)?;
        let token: [u8; 32] = self
            .deps
            .crypto
            .open(&Sealed(sealed))?
            .try_into()
            .map_err(|_| AppError::NotFound)?;
        let task = file
            .task
            .as_deref()
            .map(TaskId::parse)
            .transpose()
            .map_err(|_| AppError::NotFound)?;
        let folder = self.shell_folder(&sandbox, task.as_ref()).await;
        let runtime = Arc::new(ShellRuntime {
            id: file.id.clone(),
            kind: file.kind,
            resumed: OnceLock::new(),
            sandbox,
            creator: UserId::parse(&file.creator).map_err(|_| AppError::NotFound)?,
            task,
            folder,
            name: Mutex::new(clean_name(&file.name).unwrap_or_else(|_| "Terminal".into())),
            created_at: Timestamp(file.created_at),
            seq: file.seq,
            run_dir: run_dir.to_path_buf(),
            token,
            hub: Hub::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1),
            writer: tokio::sync::Mutex::new(None),
            ended: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            ended_tx: watch::channel(false).0,
            idle_since_ms: AtomicI64::new(self.now().0),
            pump: Mutex::new(None),
        });
        let reader = self.attach_shell(&runtime).await?;
        self.shells
            .next_seq
            .fetch_max(file.seq.saturating_add(1), Ordering::SeqCst);
        if let Ok(mut map) = self.shells.shells.lock() {
            map.insert(runtime.id.clone(), Arc::clone(&runtime));
        }
        self.start_pump(&runtime, reader);
        tracing::info!(shell = %runtime.id, sandbox = %runtime.sandbox, "adopted a running sandbox shell");
        Ok(())
    }

    /// Adopts the shells of a previous server process ([`Self::adopt_shells`]), then starts the
    /// periodic reaper of idle shells and browsers; call once at startup, before serving
    /// requests. The reaper stops when the handle is dropped or the app is gone.
    pub async fn start_workspace_services(self: &Arc<Self>) -> WorkspaceReaper {
        let (adopted, removed) = self.adopt_shells().await;
        self.prune_shell_attachments();
        if adopted + removed > 0 {
            tracing::info!(adopted, removed, "shells of the previous server process");
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

    /// Server shutdown: lets go of every shell — they keep running in their detached hosts for
    /// the next server process, like agent sessions — and stops every browser (browsers do not
    /// outlive the server; their processes are reaped).
    pub async fn stop_workspace_services(&self) {
        let shells: Vec<Arc<ShellRuntime>> = self
            .shells
            .shells
            .lock()
            .map(|mut map| map.drain().map(|(_, shell)| shell).collect())
            .unwrap_or_default();
        for shell in &shells {
            if let Ok(mut pump) = shell.pump.lock()
                && let Some(task) = pump.take()
            {
                task.abort();
            }
            *shell.writer.lock().await = None;
        }
        let browsers = self.deps.browser.shutdown_all().await;
        if !browsers.is_empty() {
            tracing::error!(?browsers, "some browser processes could not be reaped");
        }
    }

    /// Ends every shell of this server process and waits for them (tests and maintenance;
    /// a normal shutdown keeps them running, [`Self::stop_workspace_services`]).
    pub async fn end_all_shells(&self) {
        let shells: Vec<Arc<ShellRuntime>> = self
            .shells
            .shells
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default();
        self.end_shells(&shells).await;
    }
}
