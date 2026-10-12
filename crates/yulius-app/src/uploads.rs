//! Uploads (SPEC-001 EDT-006, UI-048; docs/api.md "Browser, editor, shell"): files dropped onto
//! the editor go into the sandbox repository or a task worktree; files dropped or pasted onto a
//! terminal become attachments that the agent (or the user's own shell) opens by path.
//!
//! Contract:
//! - one file per request, streamed through a bounded buffer and never held in memory: at most
//!   `AppConfig::upload_max_bytes` (a declared length above it is refused before any byte is
//!   read, and the received bytes are counted); at most [`MAX_CONCURRENT_UPLOADS`] uploads at once
//!   (`Capacity` beyond); the interface layer fails a body that sends nothing for
//!   [`UPLOAD_IDLE`];
//! - the bytes go to a temporary file (`.yulius-upload-<random>`, created exclusively, 0644) in
//!   the destination folder and are synced; then the file takes its name atomically: linked under
//!   a new name (an existing name is a `Conflict` and is never replaced), renamed over an existing
//!   regular file when the caller confirmed the overwrite (its permissions kept; a folder or a
//!   link is never replaced), or, for attachments, linked under the first free name (`name`,
//!   `name-2.ext`, …). The folder is resolved again before that step: a folder replaced by a link
//!   meanwhile is a `Conflict`;
//! - an upload that does not finish (an error, a cancelled request, a client that went away)
//!   removes its temporary file ([`Upload`]'s `Drop`);
//! - editor uploads need write access and follow the files API (crate::files): `root` is the
//!   repository or a task worktree; `path` is relative, normal components only, no `.git`; every
//!   existing component (links resolved) stays inside the folder and out of `.git`; missing
//!   parent folders are created inside it (they stay when the upload then fails); each component
//!   is 1 to 255 bytes without control characters;
//! - attachments live in `sandboxes/<sid>/attachments/tasks/<task>/` (write access to the task's
//!   sandbox, i.e. whoever may type into its terminal) and `…/attachments/shells/<shell>/` (the
//!   shell's owner only). They are inside the sandbox directory, which containment binds at the
//!   same path, so a contained agent or shell opens the absolute path it is given, and outside
//!   every worktree, so they never appear in `git status`, the delivery gates or a pull request.
//!   Like every file of the sandbox they are writable from inside it: a read-only mount of a
//!   folder the sandbox can rename would let a sandbox process swap in a link and have bubblewrap
//!   mount whatever it points at. Every folder on the way is checked to be a real directory, never
//!   a link (a contained shell, root in its sandbox, could plant one); as with the files API, the
//!   checks are by path, so a swap racing the few milliseconds between check and use is not
//!   excluded (an `openat`-based resolution would close it). Names
//!   keep letters, digits, `.`, `-`, `_` and `+` (anything else becomes `-`; no leading dot or
//!   dash), so a path needs no quoting in a terminal; at most [`MAX_ATTACHMENTS_PER_FOLDER`] per
//!   folder. Files dropped on the new-task composer wait in `…/attachments/drafts/<user>/` and are
//!   removed after a week; a task's folder is removed with the task (TSK-009), a shell's when the shell ends and
//!   at start-up for shells that are gone;
//! - every upload is audited (`file.uploaded`, `attachment.uploaded`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use yulius_contracts::api::files::UploadedFile;
use yulius_domain::ids::{SandboxId, TaskId};

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::files::{inside, io_error, relative_components};

/// Default largest upload (25 MiB; `YULIUS_UPLOAD_MAX_MB`).
pub const DEFAULT_UPLOAD_MAX_BYTES: u64 = 25 * 1024 * 1024;
/// Uploads in progress at once on this instance.
pub const MAX_CONCURRENT_UPLOADS: usize = 8;
/// Longest pause in an upload's body before it fails.
pub const UPLOAD_IDLE: Duration = Duration::from_secs(60);
/// Attachments kept per task or shell.
pub const MAX_ATTACHMENTS_PER_FOLDER: usize = 500;
/// Longest file or folder name, in bytes (the file system's limit).
const MAX_NAME_BYTES: usize = 255;
/// Longest attachment name, in characters (the extension is kept).
const MAX_ATTACHMENT_NAME_CHARS: usize = 100;
/// Names tried for an attachment before giving up (`name`, `name-2`, …).
const MAX_FREE_NAME_ATTEMPTS: u32 = 1000;
/// Folder of every attachment in a sandbox directory.
const ATTACHMENTS_DIR: &str = "attachments";
/// Shell attachment folders inspected at start-up.
const MAX_PRUNED_ENTRIES: usize = 10_000;

/// How long a file dropped on the composer waits for its task before it is removed.
const DRAFT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Uploads in progress (the concurrency bound).
pub struct UploadState {
    slots: Arc<Semaphore>,
}

impl Default for UploadState {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS)),
        }
    }
}

/// How the finished file takes its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// A new name only; an existing one is a conflict.
    Create,
    /// Replaces an existing regular file (or creates the name).
    Overwrite,
    /// The first free name derived from the requested one (attachments).
    FreeName,
}

/// One upload in progress: write the body with [`Upload::write`], then hand it to
/// [`App::finish_upload`]. Dropping it unfinished removes its temporary file.
pub struct Upload {
    /// The destination folder, resolved (canonical) when the upload began.
    dir: PathBuf,
    /// The requested file name.
    name: String,
    /// The folder as reported to the caller: relative to the root (editor) or absolute as the
    /// agent sees it (attachments).
    shown_dir: String,
    attachment: bool,
    temp: PathBuf,
    file: Option<tokio::fs::File>,
    placement: Placement,
    received: u64,
    limit: u64,
    hasher: Sha256,
    actor: String,
    audit_target: String,
    _slot: OwnedSemaphorePermit,
}

impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload")
            .field("name", &self.name)
            .field("received", &self.received)
            .finish_non_exhaustive()
    }
}

impl Upload {
    /// Appends a chunk of the body.
    ///
    /// # Errors
    /// `Invalid` beyond the size limit; `Capacity` when the disk is full; I/O errors.
    pub async fn write(&mut self, chunk: &[u8]) -> AppResult<()> {
        let len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if self.received.saturating_add(len) > self.limit {
            return Err(too_large(self.limit));
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| AppError::internal("the upload was already finished"))?;
        file.write_all(chunk)
            .await
            .map_err(|err| upload_io_error(&err))?;
        self.hasher.update(chunk);
        self.received += len;
        Ok(())
    }

    /// Bytes received so far.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        // Linked or renamed files no longer need it; an unfinished upload leaves nothing behind.
        let _ = std::fs::remove_file(&self.temp);
    }
}

fn too_large(limit: u64) -> AppError {
    AppError::Invalid(format!(
        "the file is larger than the upload limit of {}",
        human_size(limit)
    ))
}

/// `25 MB`, `512 KB`, `100 bytes` (binary units, as `YULIUS_UPLOAD_MAX_MB`).
fn human_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MB", bytes / MIB)
    } else if bytes >= MIB {
        format!("{}.{} MB", bytes / MIB, (bytes % MIB) * 10 / MIB)
    } else if bytes >= 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

fn upload_io_error(err: &std::io::Error) -> AppError {
    if err.kind() == std::io::ErrorKind::StorageFull {
        AppError::Capacity("the server's disk is full".into())
    } else {
        io_error(err)
    }
}

/// Checks every component of an editor upload's path: 1 to 255 bytes, no control characters.
fn check_components(relative: &str) -> AppResult<()> {
    for part in relative.split('/') {
        if part.is_empty() || part.len() > MAX_NAME_BYTES {
            return Err(AppError::Invalid(format!(
                "file and folder names have 1 to {MAX_NAME_BYTES} bytes"
            )));
        }
        if part.chars().any(char::is_control) {
            return Err(AppError::Invalid(
                "file names cannot contain control characters".into(),
            ));
        }
    }
    Ok(())
}

/// Invisible formatting characters that could disguise a name (bidirectional overrides,
/// zero-width characters, the byte-order mark).
fn is_disguising(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// A safe attachment name: the last component of `raw`; ASCII letters, digits, `.`, `_`, `+`
/// and printable non-ASCII letters are kept, anything else becomes `-` (one per run, none before
/// a dot); no leading dot or dash; at most 100 characters with the extension kept; `attachment`
/// when nothing is left. Such a path needs no quoting in a shell.
#[must_use]
pub fn attachment_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut out = String::new();
    for c in last.chars() {
        let keep = c.is_ascii_alphanumeric()
            || matches!(c, '.' | '_' | '+')
            || (!c.is_ascii() && !c.is_whitespace() && !c.is_control() && !is_disguising(c));
        if keep {
            // `report-.pdf` reads better as `report.pdf`.
            if c == '.' && out.ends_with('-') {
                out.pop();
            }
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_start_matches(['.', '-']).trim_end_matches('-');
    let mut name: String = trimmed.to_owned();
    if name.chars().count() > MAX_ATTACHMENT_NAME_CHARS {
        let (stem, ext) = split_extension(&name);
        let ext: String = ext.chars().take(16).collect();
        let keep = MAX_ATTACHMENT_NAME_CHARS.saturating_sub(ext.chars().count());
        let stem: String = stem.chars().take(keep).collect();
        name = format!("{}{ext}", stem.trim_end_matches(['.', '-']));
    }
    while name.len() > MAX_NAME_BYTES - 8 {
        name.pop();
    }
    if name.is_empty() || name.chars().all(|c| c == '.') {
        "attachment".into()
    } else {
        name
    }
}

/// `("photo", ".png")`; a name without a dot (or only a leading one) has no extension.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// The `n`-th candidate for a free name: `photo.png`, `photo-2.png`, `photo-3.png`, …
fn candidate(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    format!("{stem}-{n}{ext}")
}

/// Creates (or checks) `parts` below `base` as real directories: a link or a file on the way is
/// refused, so nothing is ever written through a link a sandbox process planted.
fn ensure_plain_dirs(base: &Path, parts: &[&str]) -> AppResult<PathBuf> {
    let mut current = base.to_path_buf();
    for part in parts {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => {
                return Err(AppError::Forbidden(
                    "the attachments folder is not a plain folder".into(),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        let meta = std::fs::symlink_metadata(&current).map_err(|e| io_error(&e))?;
                        if !meta.file_type().is_dir() {
                            return Err(AppError::Forbidden(
                                "the attachments folder is not a plain folder".into(),
                            ));
                        }
                    }
                    Err(err) => return Err(io_error(&err)),
                }
            }
            Err(err) => return Err(io_error(&err)),
        }
    }
    Ok(current)
}

/// Whether `parts` below `base` are all real directories (removal never follows a link).
fn plain_dirs_exist(base: &Path, parts: &[&str]) -> bool {
    let mut current = base.to_path_buf();
    parts.iter().all(|part| {
        current.push(part);
        std::fs::symlink_metadata(&current).is_ok_and(|meta| meta.file_type().is_dir())
    })
}

/// Creates the missing folders of an editor upload below the (canonical) `root`; existing ones
/// are resolved and must stay inside the root, out of `.git`, and be folders.
fn ensure_dirs(root: &Path, dirs: &[&str]) -> AppResult<PathBuf> {
    let mut current = root.to_path_buf();
    for part in dirs {
        let next = current.join(part);
        match std::fs::symlink_metadata(&next) {
            Ok(_) => {
                let resolved = std::fs::canonicalize(&next).map_err(|e| io_error(&e))?;
                if !inside(root, &resolved) {
                    return Err(AppError::Forbidden(
                        "the path leads outside the folder".into(),
                    ));
                }
                if !resolved.is_dir() {
                    return Err(AppError::Conflict(format!(
                        "{part} is a file, not a folder"
                    )));
                }
                current = resolved;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&next) {
                    Ok(()) => current = next,
                    // Created meanwhile (another upload of the same folder): check it as above.
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        let resolved = std::fs::canonicalize(&next).map_err(|e| io_error(&e))?;
                        if !inside(root, &resolved) || !resolved.is_dir() {
                            return Err(AppError::Conflict(format!(
                                "{part} changed during the upload"
                            )));
                        }
                        current = resolved;
                    }
                    Err(err) => return Err(io_error(&err)),
                }
            }
            Err(err) => return Err(io_error(&err)),
        }
    }
    Ok(current)
}

/// Links `temp` as `dest` unless `dest` exists. Returns whether it was placed. Where the file
/// system has no hard links, it checks and renames instead (not atomic against a racing writer).
async fn link_new(temp: &Path, dest: &Path) -> std::io::Result<bool> {
    match tokio::fs::hard_link(temp, dest).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => match tokio::fs::symlink_metadata(dest).await {
            Ok(_) => Ok(false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::rename(temp, dest).await?;
                Ok(true)
            }
            Err(err) => Err(err),
        },
    }
}

/// Makes a rename or link durable (best effort: not every file system syncs directories).
async fn sync_dir(dir: &Path) {
    if let Ok(handle) = tokio::fs::File::open(dir).await {
        let _ = handle.sync_all().await;
    }
}

/// Who an attachment belongs to.
enum Owner<'a> {
    Task(&'a TaskId),
    Shell(&'a str),
    /// Files dropped on the new-task composer, before any task exists (one folder per user).
    Draft(&'a str),
}

impl Owner<'_> {
    fn parts(&self) -> [&str; 3] {
        match self {
            Self::Task(id) => [ATTACHMENTS_DIR, "tasks", id.as_str()],
            Self::Shell(id) => [ATTACHMENTS_DIR, "shells", id],
            Self::Draft(user) => [ATTACHMENTS_DIR, "drafts", user],
        }
    }

    fn audit_target(&self) -> String {
        match self {
            Self::Task(id) => format!("task:{}", id.as_str()),
            Self::Shell(id) => format!("shell:{id}"),
            Self::Draft(user) => format!("draft:{user}"),
        }
    }
}

impl App {
    fn upload_actor(ctx: &AuthContext) -> String {
        ctx.user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into())
    }

    fn check_declared(&self, declared: Option<u64>) -> AppResult<()> {
        match declared {
            Some(len) if len > self.config.upload_max_bytes => {
                Err(too_large(self.config.upload_max_bytes))
            }
            _ => Ok(()),
        }
    }

    fn upload_slot(&self) -> AppResult<OwnedSemaphorePermit> {
        Arc::clone(&self.uploads.slots)
            .try_acquire_owned()
            .map_err(|_| {
                AppError::Capacity("too many uploads at once; try again in a moment".into())
            })
    }

    /// The sandbox directory as processes see it (absolute; containment binds it at this path).
    fn sandbox_dir_absolute(&self, sandbox: &SandboxId) -> PathBuf {
        let dir = self.sandbox_dir(sandbox);
        std::path::absolute(&dir).unwrap_or(dir)
    }

    /// `audit` is the actor and the audit target.
    async fn new_upload(
        &self,
        slot: OwnedSemaphorePermit,
        dir: PathBuf,
        (name, shown_dir): (String, String),
        placement: Placement,
        (actor, audit_target): (String, String),
    ) -> AppResult<Upload> {
        let random = self.deps.crypto.random_bytes();
        let temp = dir.join(format!(
            ".yulius-upload-{}",
            hex_token(random.get(..8).unwrap_or(&random))
        ));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temp)
            .await
            .map_err(|err| upload_io_error(&err))?;
        Ok(Upload {
            dir,
            name,
            shown_dir,
            attachment: placement == Placement::FreeName,
            temp,
            file: Some(file),
            placement,
            received: 0,
            limit: self.config.upload_max_bytes,
            hasher: Sha256::new(),
            actor,
            audit_target,
            _slot: slot,
        })
    }

    /// Starts an editor upload of `path` below `root` (see the module docs). `overwrite` replaces
    /// an existing regular file; `declared` is the request's `Content-Length`.
    ///
    /// # Errors
    /// Access errors as the files API; `Invalid` for bad names or a declared size above the
    /// limit; `Forbidden` outside the folder or in `.git`; `Conflict` when the name exists (or is
    /// a folder or link) and was not to be replaced; `Capacity` beyond the concurrent uploads.
    pub async fn begin_file_upload(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
        overwrite: bool,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        // The path's own rules first, so a bad name is reported before access is looked up.
        relative_components(path)?;
        let target = self.file_target(ctx, sandbox, root, path).await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid("a file path is required".into()));
        }
        check_components(&target.relative)?;
        self.check_declared(declared)?;
        let parts: Vec<&str> = target.relative.split('/').collect();
        let (name, dirs) = parts
            .split_last()
            .ok_or_else(|| AppError::Invalid("a file path is required".into()))?;
        // A slot first: nothing is created for an upload that cannot run now.
        let slot = self.upload_slot()?;
        let dir = ensure_dirs(&target.root, dirs)?;
        // Refuse an existing name before the body is sent; the final step checks again.
        if let Ok(meta) = std::fs::symlink_metadata(dir.join(name)) {
            if meta.file_type().is_symlink() {
                return Err(AppError::Conflict(format!(
                    "{name} is a link; it is not replaced"
                )));
            }
            if meta.is_dir() {
                return Err(AppError::Conflict(format!("a folder named {name} exists")));
            }
            if !overwrite {
                return Err(AppError::Conflict(format!("{name} exists")));
            }
        }
        let shown_dir = dirs.join("/");
        let root_label = root.unwrap_or("sandbox").to_owned();
        self.new_upload(
            slot,
            dir,
            ((*name).to_owned(), shown_dir),
            if overwrite {
                Placement::Overwrite
            } else {
                Placement::Create
            },
            (
                Self::upload_actor(ctx),
                format!("{sandbox}:{root_label}:{}", target.relative),
            ),
        )
        .await
    }

    /// Starts an attachment for the task terminal of `task` (write access to its sandbox).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without access; `Invalid` above the size limit; `Conflict` when
    /// the task has [`MAX_ATTACHMENTS_PER_FOLDER`] attachments; `Capacity` beyond the concurrent
    /// uploads.
    pub async fn begin_task_attachment(
        &self,
        ctx: &AuthContext,
        task: &str,
        name: &str,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        let record = self
            .load_task(ctx, task, yulius_domain::permission::Action::Work)
            .await?;
        if ctx.user.disabled_at.is_some() {
            return Err(AppError::Forbidden("this account is disabled".into()));
        }
        self.begin_attachment(
            ctx,
            &record.sandbox_id,
            &Owner::Task(&record.id),
            name,
            declared,
        )
        .await
    }

    /// Starts an attachment for one of the caller's shells, their Cockpit included (only its
    /// owner, write access).
    ///
    /// # Errors
    /// `NotFound` for unknown, ended or other users' shells; otherwise as
    /// [`App::begin_task_attachment`].
    pub async fn begin_shell_attachment(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        shell: &str,
        name: &str,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        let runtime = self.own_shell_of(ctx, sandbox, shell, None).await?;
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let id = runtime.info().id;
        self.begin_attachment(ctx, &sandbox_id, &Owner::Shell(&id), name, declared)
            .await
    }

    /// Starts an attachment for the new-task composer of `sandbox` (write access to it): the
    /// file waits in the caller's drafts folder, outside every worktree, until the prompt that
    /// names its path starts a task. Drafts older than a week are removed first.
    ///
    /// # Errors
    /// As [`App::begin_task_attachment`], with `NotFound`/`Forbidden` for the sandbox.
    pub async fn begin_draft_attachment(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(
            &ctx.user,
            &sandbox_id,
            yulius_domain::permission::Action::Work,
        )
        .await?;
        if ctx.user.disabled_at.is_some() {
            return Err(AppError::Forbidden("this account is disabled".into()));
        }
        let user = ctx.user.id.as_str();
        self.prune_old_drafts(&sandbox_id, user);
        self.begin_attachment(ctx, &sandbox_id, &Owner::Draft(user), name, declared)
            .await
    }

    /// Removes the files of a user's drafts folder that are older than [`DRAFT_MAX_AGE`].
    fn prune_old_drafts(&self, sandbox: &SandboxId, user: &str) {
        let Ok(dir) = std::fs::canonicalize(self.sandbox_dir(sandbox)) else {
            return;
        };
        let owner = Owner::Draft(user);
        let parts = owner.parts();
        if !plain_dirs_exist(&dir, &parts) {
            return;
        }
        let folder = parts.iter().fold(dir, |path, part| path.join(part));
        let Ok(entries) = std::fs::read_dir(&folder) else {
            return;
        };
        for entry in entries.flatten().take(MAX_PRUNED_ENTRIES) {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > DRAFT_MAX_AGE);
            if old && entry.file_type().is_ok_and(|t| t.is_file()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    async fn begin_attachment(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxId,
        owner: &Owner<'_>,
        name: &str,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        self.check_declared(declared)?;
        let name = attachment_name(name);
        let slot = self.upload_slot()?;
        let sandbox_dir = std::fs::canonicalize(self.sandbox_dir(sandbox))
            .map_err(|_| AppError::Conflict("the sandbox has no folder yet".into()))?;
        let dir = ensure_plain_dirs(&sandbox_dir, &owner.parts())?;
        let count = std::fs::read_dir(&dir)
            .map_err(|e| io_error(&e))?
            .take(MAX_ATTACHMENTS_PER_FOLDER + 1)
            .count();
        if count >= MAX_ATTACHMENTS_PER_FOLDER {
            return Err(AppError::Conflict(format!(
                "at most {MAX_ATTACHMENTS_PER_FOLDER} attachments are kept per terminal or draft"
            )));
        }
        let shown_dir = owner
            .parts()
            .iter()
            .fold(self.sandbox_dir_absolute(sandbox), |path, part| {
                path.join(part)
            })
            .to_string_lossy()
            .into_owned();
        self.new_upload(
            slot,
            dir,
            (name, shown_dir),
            Placement::FreeName,
            (Self::upload_actor(ctx), owner.audit_target()),
        )
        .await
    }

    /// Completes an upload: syncs it, gives it its name atomically (see the module docs) and
    /// audits it.
    ///
    /// # Errors
    /// `Conflict` when the name exists (and was not to be replaced), is a folder or a link, or
    /// the folder changed during the upload; I/O errors.
    pub async fn finish_upload(&self, mut upload: Upload) -> AppResult<UploadedFile> {
        let mut file = upload
            .file
            .take()
            .ok_or_else(|| AppError::internal("the upload was already finished"))?;
        file.flush().await.map_err(|e| upload_io_error(&e))?;
        file.sync_all().await.map_err(|e| upload_io_error(&e))?;
        drop(file);
        let still = tokio::fs::canonicalize(&upload.dir).await.ok();
        if still.as_deref() != Some(upload.dir.as_path()) {
            return Err(AppError::Conflict(
                "the folder changed during the upload; try again".into(),
            ));
        }
        let (name, replaced) = match upload.placement {
            Placement::Create => {
                let dest = upload.dir.join(&upload.name);
                if !link_new(&upload.temp, &dest)
                    .await
                    .map_err(|e| upload_io_error(&e))?
                {
                    return Err(AppError::Conflict(format!("{} exists", upload.name)));
                }
                (upload.name.clone(), false)
            }
            Placement::Overwrite => {
                let dest = upload.dir.join(&upload.name);
                let replaced = match tokio::fs::symlink_metadata(&dest).await {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(AppError::Conflict(format!(
                            "{} is a link; it is not replaced",
                            upload.name
                        )));
                    }
                    Ok(meta) if meta.is_dir() => {
                        return Err(AppError::Conflict(format!(
                            "a folder named {} exists",
                            upload.name
                        )));
                    }
                    Ok(meta) => {
                        let _ = tokio::fs::set_permissions(&upload.temp, meta.permissions()).await;
                        true
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                    Err(err) => return Err(io_error(&err)),
                };
                tokio::fs::rename(&upload.temp, &dest)
                    .await
                    .map_err(|e| upload_io_error(&e))?;
                (upload.name.clone(), replaced)
            }
            Placement::FreeName => {
                let mut placed = None;
                for n in 1..=MAX_FREE_NAME_ATTEMPTS {
                    let name = candidate(&upload.name, n);
                    if link_new(&upload.temp, &upload.dir.join(&name))
                        .await
                        .map_err(|e| upload_io_error(&e))?
                    {
                        placed = Some(name);
                        break;
                    }
                }
                let name = placed.ok_or_else(|| {
                    AppError::Conflict(format!("no free name for {}", upload.name))
                })?;
                (name, false)
            }
        };
        let _ = tokio::fs::remove_file(&upload.temp).await;
        sync_dir(&upload.dir).await;
        let sha256 = hex_token(upload.hasher.clone().finalize().as_slice());
        let path = if upload.shown_dir.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", upload.shown_dir)
        };
        let size = upload.received;
        let (action, target, detail) = if upload.attachment {
            (
                "attachment.uploaded",
                upload.audit_target.clone(),
                format!("{name} ({size} bytes)"),
            )
        } else {
            (
                "file.uploaded",
                upload.audit_target.clone(),
                format!("{size} bytes{}", if replaced { ", replaced" } else { "" }),
            )
        };
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                &upload.actor,
                action,
                Some(&target),
                Some(&detail),
            )
            .await;
        tracing::info!(action, %target, size, replaced, "upload stored");
        Ok(UploadedFile {
            path,
            name,
            size,
            sha256,
            replaced,
        })
    }

    fn remove_attachments(&self, sandbox: &SandboxId, owner: &Owner<'_>) {
        let Ok(dir) = std::fs::canonicalize(self.sandbox_dir(sandbox)) else {
            return;
        };
        let parts = owner.parts();
        if plain_dirs_exist(&dir, &parts) {
            let folder = parts.iter().fold(dir, |path, part| path.join(part));
            if let Err(err) = std::fs::remove_dir_all(&folder) {
                tracing::warn!(%sandbox, error = %err, "could not remove attachments");
            }
        }
    }

    /// Removes a deleted task's attachments (TSK-009).
    pub(crate) fn remove_task_attachments(&self, sandbox: &SandboxId, task: &TaskId) {
        self.remove_attachments(sandbox, &Owner::Task(task));
    }

    /// Removes an ended shell's attachments.
    pub(crate) fn remove_shell_attachments(&self, sandbox: &SandboxId, shell: &str) {
        self.remove_attachments(sandbox, &Owner::Shell(shell));
    }

    /// Removes the attachment folders of shells that no longer run (they ended while no server
    /// was there to see it). Returns how many were removed. Bounded by [`MAX_PRUNED_ENTRIES`].
    pub(crate) fn prune_shell_attachments(&self) -> usize {
        let sandboxes = self.config.data_dir.join("sandboxes");
        let Ok(entries) = std::fs::read_dir(&sandboxes) else {
            return 0;
        };
        let mut removed = 0;
        for sandbox in entries.flatten().take(MAX_PRUNED_ENTRIES) {
            let Ok(id) = SandboxId::parse(&sandbox.file_name().to_string_lossy()) else {
                continue;
            };
            let base = sandbox.path();
            if !plain_dirs_exist(&base, &[ATTACHMENTS_DIR, "shells"]) {
                continue;
            }
            let live: Vec<String> = self
                .sandbox_shells(&id)
                .iter()
                .map(|shell| shell.info().id)
                .collect();
            let Ok(shells) = std::fs::read_dir(base.join(ATTACHMENTS_DIR).join("shells")) else {
                continue;
            };
            for shell in shells.flatten().take(MAX_PRUNED_ENTRIES) {
                let name = shell.file_name().to_string_lossy().into_owned();
                if !live.contains(&name) {
                    self.remove_shell_attachments(&id, &name);
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_need_no_quoting() {
        assert_eq!(
            attachment_name("Screen Shot 2026-10-10 at 10.00.png"),
            "Screen-Shot-2026-10-10-at-10.00.png"
        );
        assert_eq!(attachment_name("report (final).pdf"), "report-final.pdf");
        assert_eq!(attachment_name("a -- b.txt"), "a-b.txt");
        assert_eq!(attachment_name("../../etc/passwd"), "passwd");
        assert_eq!(attachment_name("C:\\Users\\me\\notes.txt"), "notes.txt");
        assert_eq!(attachment_name(".env"), "env");
        assert_eq!(attachment_name("-rf"), "rf");
        assert_eq!(attachment_name("$(rm -rf ~);`x`.sh"), "rm-rf-x.sh");
        assert_eq!(attachment_name("zdjęcie.jpg"), "zdjęcie.jpg");
        assert_eq!(attachment_name("evil\u{202E}gpj.exe"), "evil-gpj.exe");
        assert_eq!(attachment_name(""), "attachment");
        assert_eq!(attachment_name("..."), "attachment");
        assert_eq!(attachment_name("a\nb\tc"), "a-b-c");
        let long = format!("{}.png", "x".repeat(300));
        let short = attachment_name(&long);
        assert!(short.ends_with(".png") && short.chars().count() <= MAX_ATTACHMENT_NAME_CHARS);
    }

    #[test]
    fn free_names_keep_the_extension() {
        assert_eq!(candidate("photo.png", 1), "photo.png");
        assert_eq!(candidate("photo.png", 2), "photo-2.png");
        assert_eq!(candidate("archive.tar.gz", 3), "archive.tar-3.gz");
        assert_eq!(candidate("Makefile", 2), "Makefile-2");
    }

    #[test]
    fn components_are_bounded_and_printable() {
        assert!(check_components("src/assets/logo.svg").is_ok());
        assert!(check_components("a\nb.txt").is_err());
        assert!(check_components(&"x".repeat(256)).is_err());
        assert!(check_components(&"x".repeat(255)).is_ok());
        assert!(check_components("a//b").is_err());
    }

    #[test]
    fn sizes_read_like_the_setting() {
        assert_eq!(human_size(25 * 1024 * 1024), "25 MB");
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(10), "10 bytes");
    }

    #[test]
    fn plain_dirs_refuse_links() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().canonicalize()?;
        let made = ensure_plain_dirs(&base, &["attachments", "tasks", "t1"])?;
        assert!(made.is_dir() && plain_dirs_exist(&base, &["attachments", "tasks", "t1"]));
        let outside = base.join("elsewhere");
        std::fs::create_dir(&outside)?;
        std::fs::create_dir(base.join("other"))?;
        std::os::unix::fs::symlink(&outside, base.join("other").join("attachments"))?;
        assert!(ensure_plain_dirs(&base.join("other"), &["attachments", "tasks"]).is_err());
        assert!(!plain_dirs_exist(&base.join("other"), &["attachments"]));
        assert!(
            !outside.join("tasks").exists(),
            "nothing was created through the link"
        );
        Ok(())
    }
}
