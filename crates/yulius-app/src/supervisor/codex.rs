//! Codex specifics of the supervisor: its input hold, homes, run end and sign-in write-back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId};
use yulius_domain::task::TaskStatus;

use super::typing::write_private_file;
use super::{CONTINUE_TASK_PROMPT, RunStart, SESSION_WRITER_BUSY, TaskRuntime};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{LaunchKind, Secret, TaskRecord};

/// How many times a Codex refresh is stored again after another run's older refresh of the same
/// sign-in won the swap (SPEC-503 CDX-006).
const CODEX_REFRESH_SWAPS: usize = 4;

/// A Codex paste can remain in the composer if no Enter followed it. The run
/// directory records the pending paste before it goes to the PTY.
pub(super) struct CodexInput {
    marker: PathBuf,
    pub(super) state: CodexPaste,
    /// Size to recover if the server disappears during the temporary resize. Once
    /// a run has used a probe, deliberate viewer resizes supersede its saved size.
    pub(super) repaint_size: Option<(u16, u16)>,
}

#[derive(PartialEq, Eq)]
pub(super) enum CodexPaste {
    Idle,
    InFlight,
    Recovering,
}

impl CodexInput {
    pub(super) fn load(run_dir: &str) -> Self {
        let marker = Path::new(run_dir).join("codex-submit-held");
        let state = if marker.try_exists().unwrap_or(true) {
            CodexPaste::Recovering
        } else {
            CodexPaste::Idle
        };
        let repaint_size = Self::read_repaint_size(&marker.with_file_name("codex-repaint-size"));
        Self {
            marker,
            state,
            repaint_size,
        }
    }

    fn read_repaint_size(path: &Path) -> Option<(u16, u16)> {
        use std::io::Read as _;
        let mut file = std::fs::File::open(path).ok()?;
        let mut bytes = [0; 4];
        if file.metadata().ok()?.len() != 4 {
            return None;
        }
        file.read_exact(&mut bytes).ok()?;
        let [c1, c2, r1, r2] = bytes;
        let size = (u16::from_be_bytes([c1, c2]), u16::from_be_bytes([r1, r2]));
        let dimensions =
            yulius_contracts::ptyhost::MIN_DIMENSION..=yulius_contracts::ptyhost::MAX_DIMENSION;
        (dimensions.contains(&size.0) && dimensions.contains(&size.1)).then_some(size)
    }

    fn remember_size(&mut self, size: (u16, u16)) -> AppResult<()> {
        if self.repaint_size == Some(size) {
            return Ok(());
        }
        let path = self.marker.with_file_name("codex-repaint-size");
        let temporary = path.with_extension("tmp");
        let [c1, c2] = size.0.to_be_bytes();
        let [r1, r2] = size.1.to_be_bytes();
        write_private_file(&temporary, &[c1, c2, r1, r2])
            .and_then(|()| std::fs::rename(temporary, path))
            .map_err(|err| {
                tracing::warn!(error = %err, "could not preserve Codex terminal geometry");
                AppError::Conflict(CODEX_UNREADABLE.into())
            })?;
        self.repaint_size = Some(size);
        Ok(())
    }

    /// A viewer's resize is authoritative (SPEC-623 FIN-002): it keeps a probed run's saved
    /// size current, and a record it cannot save is dropped rather than refusing the resize,
    /// so a reattachment keeps the host's (the viewer's) size instead of a stale one.
    pub(super) fn follow_viewer(&mut self, size: (u16, u16)) {
        if self.repaint_size.is_none() || self.remember_size(size).is_ok() {
            return;
        }
        self.repaint_size = None;
        let path = self.marker.with_file_name("codex-repaint-size");
        if let Err(err) = std::fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %err, "could not drop the stale Codex terminal geometry");
        }
    }

    pub(super) fn record(&mut self) -> AppResult<()> {
        use std::os::unix::fs::OpenOptionsExt as _;

        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.marker)
            .map_err(|err| {
                tracing::warn!(marker = %self.marker.display(), error = %err, "could not record the pending Codex paste");
                AppError::Conflict("could not record the Codex paste before sending it".into())
            })?;
        self.state = CodexPaste::InFlight;
        Ok(())
    }

    pub(super) fn release(&mut self) -> AppResult<()> {
        self.state = CodexPaste::Recovering;
        if let Err(err) = std::fs::remove_file(&self.marker)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(marker = %self.marker.display(), error = %err, "could not clear the withheld Codex submit");
            return Err(AppError::Conflict(
                "the empty Codex composer was seen, but its paste hold could not be cleared".into(),
            ));
        }
        self.state = CodexPaste::Idle;
        Ok(())
    }
}

/// Said on every path that found the screen unreadable; only Yulius's messages repaint first.
pub(crate) const CODEX_UNREADABLE: &str = "Codex's terminal could not confirm a safe input screen";

pub(crate) const CODEX_PICKER: &str = "Codex's rate-limit model picker is open. Choose Keep current model or dismiss the picker in the terminal.";

/// Where a Codex home's `sessions` points: the sandbox's shared sessions.
const CODEX_SESSIONS_LINK: &str = "../sessions";

/// Makes the Codex home `dir`'s `sessions` the link to the sandbox's shared sessions, or checks
/// that it is: anything else (a folder, another link) would put rollouts where Yulius and the
/// other accounts' runs never look, so the start is refused (SPEC-503 CDX-003).
fn ensure_sessions_link(dir: &std::os::fd::OwnedFd) -> AppResult<()> {
    match nix::fcntl::readlinkat(dir, "sessions") {
        Ok(target) if target == std::ffi::OsStr::new(CODEX_SESSIONS_LINK) => Ok(()),
        Ok(_) | Err(nix::errno::Errno::EINVAL) => Err(AppError::internal(
            "the Codex home's sessions is not Yulius's link to the sandbox's sessions",
        )),
        Err(nix::errno::Errno::ENOENT) => {
            nix::unistd::symlinkat(CODEX_SESSIONS_LINK, dir, "sessions")
                .map_err(crate::files::errno_error)
        }
        Err(err) => Err(crate::files::errno_error(err)),
    }
}

/// The Codex home's `auth.json` a launch wrote, taken back when the launch ends before its run is
/// live (SPEC-503 CDX-006): a failed spawn, a store error, or a subscription removed just after it
/// was chosen (SPEC-447) would otherwise leave it for later tasks of the sandbox to read. Dropped
/// armed, it runs the write-back rule on its own task (the launch lock may be held); disarmed once
/// the run is live, whose end does it instead.
pub(super) struct CodexLeftover {
    cleanup: Option<(Arc<App>, TaskId, AccountId)>,
}

impl CodexLeftover {
    pub(super) fn new(app: &Arc<App>, task: &TaskId, account: &AccountId) -> Self {
        Self {
            cleanup: Some((app.clone(), task.clone(), account.clone())),
        }
    }

    pub(super) fn disarm(&mut self) {
        self.cleanup = None;
    }
}

impl Drop for CodexLeftover {
    fn drop(&mut self) {
        let Some((app, task, account)) = self.cleanup.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                app.write_back_codex_auth(&task, &account).await;
            });
        }
    }
}

/// What a Codex write-back left ([`App::keep_codex_refresh`]): the credential the account holds,
/// and the bytes of a run's `auth.json` that lost to another credential.
#[derive(Default)]
pub(super) struct KeptCodexAuth {
    pub(super) holds: Option<Secret>,
    lost: Option<Vec<u8>>,
}

/// Removes the `auth.json` of `account`'s Codex home below `root`, opened without following a
/// link; a missing one is fine.
fn remove_codex_auth(root: &Path, account: &AccountId) -> AppResult<()> {
    let dir = match crate::files::open_dirs_below(
        root,
        &[
            std::ffi::OsStr::new("codex"),
            std::ffi::OsStr::new(account.as_str()),
        ],
        false,
    ) {
        Ok(dir) => dir,
        Err(nix::errno::Errno::ENOENT) => return Ok(()),
        Err(err) => return Err(crate::files::errno_error(err)),
    };
    crate::files::remove_at(&dir, std::ffi::OsStr::new("auth.json"))
}

/// Writes the plan's files of the Codex home `home` through its handle `dir` (mode 0600) and takes
/// them out of the plan: the agent could have turned a path below the sandbox into a link.
pub(super) fn write_codex_files(
    dir: &std::os::fd::OwnedFd,
    home: &Path,
    files: &mut Vec<crate::ports::session::PlannedFile>,
) -> AppResult<()> {
    let (own, rest): (Vec<_>, Vec<_>) = std::mem::take(files)
        .into_iter()
        .partition(|f| f.path.parent() == Some(home));
    *files = rest;
    for file in own {
        let Some(name) = file.path.file_name() else {
            continue;
        };
        let temporary = format!(".{}.yulius-tmp", name.to_string_lossy());
        let _ = nix::unistd::unlinkat(
            dir,
            temporary.as_str(),
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        );
        crate::files::replace_in(dir, name, &temporary, |out| {
            use std::io::Write as _;
            use std::os::unix::fs::PermissionsExt as _;
            out.set_permissions(std::fs::Permissions::from_mode(file.mode))?;
            out.write_all(&file.contents)
        })?;
    }
    Ok(())
}

/// The `auth.json` a Codex run left in its home, read without following a link and bounded;
/// `None` when there is none.
fn read_codex_auth(root: &Path, account: &AccountId) -> AppResult<Option<Vec<u8>>> {
    let dir = match crate::files::open_dirs_below(
        root,
        &[
            std::ffi::OsStr::new("codex"),
            std::ffi::OsStr::new(account.as_str()),
        ],
        false,
    ) {
        Ok(dir) => dir,
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(err) => return Err(crate::files::errno_error(err)),
    };
    match crate::files::read_bounded_at(
        &dir,
        std::ffi::OsStr::new("auth.json"),
        yulius_domain::harness::CODEX_AUTH_MAX_BYTES,
    ) {
        Ok(crate::files::Bounded::Bytes(mut bytes)) => Ok(Some(std::mem::take(&mut *bytes))),
        Ok(crate::files::Bounded::NotPlain | crate::files::Bounded::TooLarge) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(crate::files::io_error(&err)),
    }
}

impl TaskRuntime {
    /// Only the live, redacted screen of the observed attempt can admit input.
    pub(crate) fn codex_input_screen(&self, attempt: &AttemptId) -> AppResult<Option<bool>> {
        if self.screen_withheld() || self.screen_attempt().as_ref() != Some(attempt) {
            return Err(AppError::Conflict(
                "the live Codex screen is unavailable".into(),
            ));
        }
        Ok(self.hub.codex_rate_limit_picker_shown())
    }

    pub(crate) fn codex_ready_for_retry(&self, attempt: &AttemptId) -> bool {
        matches!(self.codex_input_screen(attempt), Ok(Some(false)))
            && self.run.try_lock().is_ok_and(|slot| {
                slot.as_ref().is_some_and(|run| {
                    run.attempt_id == *attempt
                        && run.codex_input.as_ref().is_some_and(|hold| {
                            hold.state == CodexPaste::Idle
                                || self.hub.prompt_box_shown_empty(Harness::Codex)
                        })
                })
            })
    }

    pub(super) fn check_codex_input_screen(&self, attempt: &AttemptId) -> AppResult<()> {
        match self.codex_input_screen(attempt)? {
            Some(false) => Ok(()),
            Some(true) => Err(AppError::Conflict(CODEX_PICKER.into())),
            None => Err(AppError::Conflict(CODEX_UNREADABLE.into())),
        }
    }

    /// A cancelled send may stop waiting, but the resize pair still restores the
    /// viewer's geometry. The worker owns only the repaint, never the message.
    pub(crate) async fn repaint_codex_input(
        self: &Arc<Self>,
        attempt: &AttemptId,
    ) -> AppResult<()> {
        if self.codex_input_screen(attempt)?.is_some() {
            return Ok(());
        }
        let rt = Arc::clone(self);
        let attempt = attempt.clone();
        tokio::spawn(async move {
            let mut slot = rt
                .run
                .try_lock()
                .map_err(|_| AppError::Conflict(SESSION_WRITER_BUSY.into()))?;
            let run = slot
                .as_mut()
                .filter(|run| run.attempt_id == attempt)
                .ok_or_else(|| {
                    AppError::Conflict("the observed session is no longer attached".into())
                })?;
            if rt.codex_input_screen(&attempt)?.is_some() {
                return Ok(());
            }
            let (cols, rows) = rt.hub.size();
            let input = run
                .codex_input
                .as_mut()
                .ok_or_else(|| AppError::Conflict("the observed session is not Codex".into()))?;
            input.remember_size((cols, rows))?;
            let other = if cols < 1000 { cols + 1 } else { cols - 1 };
            for width in [other, cols] {
                let watch = rt.hub.watch();
                let since = rt.hub.output_seq();
                rt.hub.resize(width, rows);
                if let Err(err) = run.writer.resize(width, rows).await {
                    rt.hub.resize(cols, rows);
                    tracing::warn!(%attempt, error = %err, "Codex repaint resize failed");
                    return Err(AppError::Conflict(CODEX_UNREADABLE.into()));
                }
                rt.hub
                    .settle(watch, since, Duration::from_millis(750))
                    .await;
            }
            Ok(())
        })
        .await
        .map_err(AppError::internal)?
    }
}

impl App {
    /// The sandbox's Codex sessions, shared by its Codex homes (SPEC-503 CDX-003).
    pub(crate) fn codex_sessions(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("codex").join("sessions")
    }

    /// The Codex home of `account` in `sandbox`, `<sandbox>/codex/<account>`, opened without
    /// following a link (the agent owns the sandbox directory, #657), with `sessions` linking to
    /// the sandbox's shared sessions so a resumed session finds its rollout whichever account
    /// runs it (CDX-003).
    pub(super) fn codex_home(
        &self,
        sandbox: &SandboxId,
        account: &AccountId,
    ) -> AppResult<(PathBuf, std::os::fd::OwnedFd)> {
        let root = self.sandbox_dir(sandbox);
        std::fs::create_dir_all(&root).map_err(AppError::internal)?;
        let linked = |err: nix::errno::Errno| match err {
            nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => AppError::internal(
                "the sandbox's Codex folder is a link or not a folder: Yulius follows no links",
            ),
            other => crate::files::errno_error(other),
        };
        let codex = std::ffi::OsStr::new("codex");
        crate::files::open_dirs_below(&root, &[codex, std::ffi::OsStr::new("sessions")], true)
            .map_err(linked)?;
        let dir = crate::files::open_dirs_below(
            &root,
            &[codex, std::ffi::OsStr::new(account.as_str())],
            true,
        )
        .map_err(linked)?;
        let home = root.join("codex").join(account.as_str());
        ensure_sessions_link(&dir)?;
        Ok((home, dir))
    }

    /// The rollout of the Codex session `session_id` in the sandbox's sessions, or where none is
    /// yet (Codex writes it at the session's first turn).
    pub(crate) fn codex_rollout(&self, sandbox: &SandboxId, session_id: &str) -> PathBuf {
        let sessions = self.codex_sessions(sandbox);
        match self.deps.harness.find_codex_rollout(&sessions, session_id) {
            Ok(Some(path)) => path,
            Ok(None) | Err(_) => Self::missing_codex_rollout(&sessions, session_id),
        }
    }

    /// Path recorded while a Codex session has no discoverable rollout (SPEC-538).
    pub(crate) fn missing_codex_rollout(sessions: &Path, session_id: &str) -> PathBuf {
        sessions.join(format!("missing-{session_id}.jsonl"))
    }

    /// After a Codex run ended or was lost (SPEC-503 CDX-006): its home's `auth.json` replaces the
    /// account's credential when it is a genuine refresh of the same sign-in, at once
    /// ([`Self::keep_codex_refresh`]); the file lies where the agent writes, so a forged one is not
    /// taken, and the account keeps its state. Its removal waits for the launch lock, so it runs
    /// on a task of its own ([`Self::remove_codex_auth_after`]): neither the run's end nor a caller
    /// holding that lock waits for it.
    pub(super) async fn codex_run_ended(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
    ) {
        let Ok(Some(attempt)) = self.deps.store.attempt(attempt_id).await else {
            return;
        };
        let Some(account) = attempt
            .account_id
            .filter(|_| attempt.harness == Harness::Codex)
        else {
            return;
        };
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let root = self.sandbox_dir(&task.sandbox_id);
        let kept = self.keep_codex_refresh(&root, &account).await;
        let app = Arc::clone(self);
        let task_id = task_id.clone();
        tokio::spawn(async move {
            app.remove_codex_auth_after(
                &task_id,
                &task.sandbox_id,
                &root,
                &account,
                Some(&attempt.id),
                kept,
            )
            .await;
        });
    }

    /// The write-back of a launch that ended before its run was live (`CodexLeftover`): as
    /// [`Self::codex_run_ended`], with no run of its own to leave out.
    async fn write_back_codex_auth(&self, task_id: &TaskId, account_id: &AccountId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let root = self.sandbox_dir(&task.sandbox_id);
        let kept = self.keep_codex_refresh(&root, account_id).await;
        self.remove_codex_auth_after(task_id, &task.sandbox_id, &root, account_id, None, kept)
            .await;
    }

    /// Once no run of the account is left in the sandbox, its `auth.json` is removed: the
    /// credential lives in the vault (`kept`: what it holds after the write-back, and the bytes
    /// that lost to another credential), not in a folder later tasks can read. `attempt` is the run that ended, left out of the runs still
    /// using the file. Reaches [`crate::race::Point::CodexWriteBackDone`] when done.
    async fn remove_codex_auth_after(
        &self,
        task_id: &TaskId,
        sandbox: &SandboxId,
        root: &Path,
        account_id: &AccountId,
        attempt: Option<&AttemptId>,
        kept: KeptCodexAuth,
    ) {
        self.remove_codex_auth_unless_used(Some(task_id), sandbox, root, account_id, attempt, kept)
            .await;
        crate::race::reached(
            crate::race::Point::CodexWriteBackDone,
            Path::new(task_id.as_str()),
        );
    }

    async fn remove_codex_auth_unless_used(
        &self,
        task_id: Option<&TaskId>,
        sandbox: &SandboxId,
        root: &Path,
        account_id: &AccountId,
        attempt: Option<&AttemptId>,
        kept: KeptCodexAuth,
    ) {
        // Under the launch lock: a run of the account starting now in this sandbox has written
        // its `auth.json` and is recorded live once the lock is free, so its file is never taken.
        let _launch = self.supervisor.launch.lock().await;
        let others = match self.deps.store.live_attempts().await {
            Ok(live) => {
                let mut others = false;
                for other in live.iter().filter(|a| {
                    Some(&a.id) != attempt
                        && a.harness == Harness::Codex
                        && a.account_id.as_ref() == Some(account_id)
                }) {
                    // A task that cannot be read may be in this sandbox: its run keeps the file.
                    match self.deps.store.task(&other.task_id).await {
                        Ok(Some(t)) if &t.sandbox_id != sandbox => {}
                        Ok(None) => {}
                        Ok(Some(_)) | Err(_) => others = true,
                    }
                }
                others
            }
            Err(_) => true,
        };
        if others {
            return;
        }
        // An account gone or signed out meanwhile has no tokens to keep: whatever is there goes.
        let gone = match self.deps.store.account(account_id).await {
            Ok(Some(account)) => account.credential.is_none(),
            Ok(None) => true,
            Err(_) => false,
        };
        // Otherwise only the vault's own copy goes: a file that did not reach the vault (a
        // refresh that could not be stored, or one not taken) may hold the only copy of rotated
        // tokens.
        match read_codex_auth(root, account_id) {
            // The vault's own copy goes, and so do tokens another credential won over: they are
            // never taken.
            Ok(Some(file))
                if gone
                    || kept
                        .holds
                        .as_ref()
                        .is_some_and(|h| h.expose().as_bytes() == file)
                    || kept.lost.as_ref().is_some_and(|lost| *lost == file) =>
            {
                if let Err(err) = remove_codex_auth(root, account_id) {
                    tracing::warn!(task = task_id.map(TaskId::as_str), account = %account_id, error = %err, "the run's Codex credentials could not be removed");
                }
            }
            Ok(Some(_)) => {
                tracing::warn!(task = task_id.map(TaskId::as_str), account = %account_id, "the run's Codex credentials did not reach the vault; left in place");
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(task = task_id.map(TaskId::as_str), account = %account_id, error = %err, "the run's Codex credentials could not be read");
            }
        }
    }

    /// Stores the `auth.json` in the Codex home of `account` below `root` when it replaces the
    /// account's credential ([`HarnessPort::codex_auth_replaces`]). Says what the account holds
    /// afterwards (`holds`; `None` when its credential is gone: signed out, nothing is stored
    /// back) and, when another credential won (the account held another or none by the time of
    /// the write: a sign-in made meanwhile, a removal, another run's newer refresh), the file's bytes
    /// that lost, which are never taken and need not stay.
    pub(super) async fn keep_codex_refresh(
        &self,
        root: &Path,
        account_id: &AccountId,
    ) -> KeptCodexAuth {
        let holds = |stored: Vec<u8>| KeptCodexAuth {
            holds: String::from_utf8(stored).ok().map(Secret::new),
            lost: None,
        };
        let Some(account) = self.deps.store.account(account_id).await.ok().flatten() else {
            return KeptCodexAuth::default();
        };
        let Some(stored) = account
            .credential
            .as_ref()
            .and_then(|sealed| self.deps.crypto.open(sealed).ok())
        else {
            return KeptCodexAuth::default();
        };
        let fresh = match read_codex_auth(root, account_id) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return holds(stored),
            Err(err) => {
                tracing::warn!(account = %account_id, error = %err, "the Codex home's credentials could not be read");
                return holds(stored);
            }
        };
        if !self
            .deps
            .harness
            .codex_auth_replaces(&fresh, &stored, self.now())
        {
            return holds(stored);
        }
        crate::race::reached(
            crate::race::Point::CodexRefreshTaken,
            Path::new(account_id.as_str()),
        );
        let sealed = match self.deps.crypto.seal(&fresh) {
            Ok(sealed) => sealed,
            Err(err) => {
                tracing::error!(error = %err, "could not seal refreshed Codex credentials");
                return holds(stored);
            }
        };
        // Only the credential changes, and only while it is the one this refresh is of: the
        // status, error and email stay (a status read above is never written back), and a new
        // sign-in made meanwhile wins. Another run's refresh of the same sign-in stored first
        // does not win when this one is newer: the swap is tried again against it.
        let Some(mut previous) = account.credential else {
            return holds(stored);
        };
        for _ in 0..CODEX_REFRESH_SWAPS {
            match self
                .deps
                .store
                .refresh_account_credential(account_id, &previous, &sealed, self.now())
                .await
            {
                Ok(true) => {
                    tracing::info!(account = %account_id, "refreshed Codex credentials kept");
                    return holds(fresh);
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::error!(error = %err, "could not store refreshed Codex credentials");
                    return holds(stored);
                }
            }
            // What the account holds now must be known before the file is called lost: a read
            // that fails or a credential that does not open leaves the file, which may hold the
            // only copy of rotated tokens.
            let now_sealed = match self.deps.store.account(account_id).await {
                Ok(account) => account.and_then(|account| account.credential),
                Err(err) => {
                    tracing::error!(account = %account_id, error = %err, "the account could not be read after a lost Codex refresh swap; the file stays");
                    return KeptCodexAuth::default();
                }
            };
            let now_held = match &now_sealed {
                Some(sealed) => match self.deps.crypto.open(sealed) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        tracing::error!(account = %account_id, error = %err, "the account's credential could not be opened after a lost Codex refresh swap; the file stays");
                        return KeptCodexAuth::default();
                    }
                },
                None => None,
            };
            match (now_sealed, now_held) {
                (Some(again), Some(held))
                    if self
                        .deps
                        .harness
                        .codex_auth_replaces(&fresh, &held, self.now()) =>
                {
                    previous = again;
                }
                (_, held) => {
                    tracing::info!(account = %account_id, "the account holds another credential or none now: the Codex refresh is not kept");
                    return KeptCodexAuth {
                        holds: held
                            .and_then(|bytes| String::from_utf8(bytes).ok().map(Secret::new)),
                        lost: Some(fresh),
                    };
                }
            }
        }
        // Still contended after every try: what the account holds is not known for sure. At a
        // run's end or a start's sweep the file stays (the next one takes it again); a launch
        // writes the credential it read before over it, as when the account cannot be read.
        tracing::warn!(account = %account_id, "the Codex refresh could not be stored: the account's credential kept changing");
        KeptCodexAuth::default()
    }

    /// Confirms which Codex conversation a native resume will open. SessionStart may have
    /// saved the attempt's ID before an outage interrupted the lineage write.
    pub(super) async fn codex_resume_session(
        &self,
        task: &TaskRecord,
    ) -> AppResult<Option<String>> {
        let store = &self.deps.store;
        let missing = || {
            AppError::Conflict(format!(
                "This {} Codex task has no recorded rollout to resume.",
                task.status.as_str()
            ))
        };
        let run = if task.status == TaskStatus::Parked {
            let id = task.resume_attempt_id.as_ref().ok_or_else(missing)?;
            let run = store.attempt(id).await?;
            if run
                .as_ref()
                .is_some_and(crate::parking::removed_before_spawn)
            {
                let runs = store.attempts(&task.id).await?;
                runs.into_iter()
                    .rev()
                    .skip_while(|run| run.id != *id)
                    .find(|run| !crate::parking::removed_before_spawn(run))
            } else {
                run
            }
        } else {
            store.attempts(&task.id).await?.pop()
        };
        let sessions = store.provider_sessions(&task.id).await?;
        let latest = sessions
            .last()
            .filter(|entry| entry.harness == Harness::Codex);
        let Some(run) = run else {
            return match (task.status, sessions.last()) {
                (TaskStatus::Parked, _) => Err(missing()),
                (_, None) => Ok(None),
                (_, Some(entry)) if entry.harness == Harness::Codex => {
                    Ok(Some(entry.session_id.clone()))
                }
                _ => Err(missing()),
            };
        };
        if run.harness != Harness::Codex {
            return Err(missing());
        }
        if let Some(id) = run.provider_session_id {
            if latest.is_none_or(|entry| entry.session_id != id) {
                self.record_lineage(task, Harness::Codex, &id, "native")
                    .await?;
                // The store preserves another task's claim on a rollout. Confirm
                // that the launch will actually read this attempt's identity.
                if !store
                    .provider_sessions(&task.id)
                    .await?
                    .last()
                    .is_some_and(|entry| entry.harness == Harness::Codex && entry.session_id == id)
                {
                    return Err(missing());
                }
            }
            return Ok(Some(id));
        }
        // A failed fresh spawn opened no session. A previous attempt's lineage is still on
        // record, but the failed launch must retry the original request.
        let fresh_spawn_failed = run.launch_kind == LaunchKind::New
            && run.exit_reason.as_deref() == Some("spawn_failed");
        if task.status != TaskStatus::Parked && (sessions.is_empty() || fresh_spawn_failed) {
            return Ok(None);
        }
        let entry = latest
            .filter(|entry| {
                run.ended_at
                    .is_some_and(|end| (run.started_at..=end).contains(&entry.last_seen))
            })
            .ok_or_else(missing)?;
        Ok(Some(entry.session_id.clone()))
    }

    /// SessionStart precedes the first rollout write. Only a saved rollout can receive the
    /// continuation. An absent rollout starts the original request, including after admission.
    pub(crate) async fn codex_restart(&self, task: &TaskRecord) -> AppResult<RunStart> {
        if let Some(id) = self.codex_resume_session(task).await?
            && self
                .deps
                .harness
                .find_codex_rollout(&self.codex_sessions(&task.sandbox_id), &id)?
                .is_some()
        {
            Ok(RunStart::Resume {
                prompt: Some(CONTINUE_TASK_PROMPT.into()),
            })
        } else {
            Ok(RunStart::Fresh { prompt: None })
        }
    }

    /// Keeps a genuine refresh from, and removes, each Codex home's `auth.json` that no live run
    /// of its subscription in its sandbox uses (a server stopped before its run's write-back
    /// removed it).
    pub(super) async fn sweep_codex_homes(&self) {
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes {
            let root = self.sandbox_dir(&sandbox.id);
            let Ok(homes) = std::fs::read_dir(root.join("codex")) else {
                continue;
            };
            for home in homes.flatten() {
                let Some(account) = home
                    .file_name()
                    .to_str()
                    .and_then(|name| AccountId::parse(name).ok())
                else {
                    continue;
                };
                if !matches!(read_codex_auth(&root, &account), Ok(Some(_))) {
                    continue;
                }
                let kept = self.keep_codex_refresh(&root, &account).await;
                self.remove_codex_auth_unless_used(None, &sandbox.id, &root, &account, None, kept)
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-613 CID-001: the saved Codex size is two big-endian u16s, columns then rows,
    /// within the host's dimensions; anything else is no record.
    #[test]
    fn the_codex_size_record_round_trips_and_rejects_malformed_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let run = dir.path().to_str().ok_or("path")?;
        let record = dir.path().join("codex-repaint-size");
        let mut input = super::CodexInput::load(run);
        assert_eq!(input.repaint_size, None, "an older run has no record");
        input.remember_size((120, 40))?;
        assert_eq!(std::fs::read(&record)?, [0, 120, 0, 40]);
        assert_eq!(super::CodexInput::load(run).repaint_size, Some((120, 40)));
        let max = yulius_contracts::ptyhost::MAX_DIMENSION;
        for bytes in [
            vec![0, 120, 0],
            vec![0, 120, 0, 40, 0],
            vec![0, 0, 0, 40],
            [(max + 1).to_be_bytes(), 40u16.to_be_bytes()].concat(),
        ] {
            std::fs::write(&record, &bytes)?;
            assert_eq!(super::CodexInput::load(run).repaint_size, None, "{bytes:?}");
        }
        Ok(())
    }

    /// SPEC-623 FIN-002: a viewer's size updates a probed run's record, a run never probed
    /// keeps none, and a record that cannot be saved is dropped.
    #[test]
    fn a_viewer_size_follows_the_codex_record_or_drops_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let run = dir.path().to_str().ok_or("path")?;
        let record = dir.path().join("codex-repaint-size");
        let mut input = super::CodexInput::load(run);
        input.follow_viewer((90, 30));
        assert!(!record.exists(), "no probe, no record");
        input.remember_size((120, 40))?;
        input.follow_viewer((100, 30));
        assert_eq!(std::fs::read(&record)?, [0, 100, 0, 30]);
        // The temporary file's name is taken, so the record can no longer be replaced.
        std::fs::create_dir(record.with_extension("tmp"))?;
        input.follow_viewer((80, 24));
        assert_eq!(input.repaint_size, None);
        assert!(!record.exists(), "the stale record is dropped");
        Ok(())
    }

    /// SPEC-503 CDX-006: the run's `auth.json` is read without following a link the agent put
    /// there, and a home that is a link is refused; a missing one is nothing to read.
    #[test]
    fn codex_credentials_are_read_without_following_links() -> Result<(), Box<dyn std::error::Error>>
    {
        use super::{AccountId, read_codex_auth, remove_codex_auth};
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let account = AccountId::parse("acc-1")?;
        assert_eq!(read_codex_auth(root, &account)?, None);
        let home = root.join("codex/acc-1");
        std::fs::create_dir_all(&home)?;
        assert_eq!(read_codex_auth(root, &account)?, None);
        std::fs::write(home.join("auth.json"), b"{}")?;
        assert_eq!(read_codex_auth(root, &account)?, Some(b"{}".to_vec()));
        std::fs::remove_file(home.join("auth.json"))?;
        std::fs::write(root.join("secret.json"), b"host secret")?;
        std::os::unix::fs::symlink(root.join("secret.json"), home.join("auth.json"))?;
        assert!(
            read_codex_auth(root, &account).is_err(),
            "a linked auth.json"
        );
        remove_codex_auth(root, &account)?;
        assert!(
            root.join("secret.json").exists(),
            "the link's target is untouched"
        );
        assert!(std::fs::symlink_metadata(home.join("auth.json")).is_err());
        std::fs::remove_dir(&home)?;
        std::os::unix::fs::symlink(root, &home)?;
        std::fs::write(root.join("auth.json"), b"{}")?;
        assert!(read_codex_auth(root, &account).is_err(), "a linked home");
        Ok(())
    }

    /// SPEC-503 CDX-003: a Codex home's `sessions` is made the link to the shared sessions, kept
    /// when it is, and anything else refuses the start.
    #[test]
    fn a_codex_homes_sessions_is_the_shared_link() -> Result<(), Box<dyn std::error::Error>> {
        use super::ensure_sessions_link;
        let dir = tempfile::tempdir()?;
        let open = |path: &std::path::Path| {
            nix::fcntl::open(
                path,
                nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_DIRECTORY,
                nix::sys::stat::Mode::empty(),
            )
        };
        let home = dir.path().join("a");
        std::fs::create_dir(&home)?;
        ensure_sessions_link(&open(&home)?)?;
        assert_eq!(
            std::fs::read_link(home.join("sessions"))?,
            std::path::PathBuf::from("../sessions")
        );
        ensure_sessions_link(&open(&home)?)?;
        let folder = dir.path().join("b");
        std::fs::create_dir_all(folder.join("sessions"))?;
        assert!(ensure_sessions_link(&open(&folder)?).is_err(), "a folder");
        let other = dir.path().join("c");
        std::fs::create_dir(&other)?;
        std::os::unix::fs::symlink("/etc", other.join("sessions"))?;
        assert!(
            ensure_sessions_link(&open(&other)?).is_err(),
            "another link"
        );
        Ok(())
    }
}
