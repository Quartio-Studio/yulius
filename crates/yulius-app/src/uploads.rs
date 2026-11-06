//! Uploads (SPEC-001 EDT-006, UI-048; docs/api.md "Browser, editor, shell"): files dropped onto
//! the editor go into the sandbox repository or a task worktree; files dropped or pasted onto a
//! terminal become attachments that the agent (or the user's own shell) opens by path.
//!
//! Contract:
//! - one file per request, streamed through a bounded buffer and never held in memory: at most
//!   `AppConfig::upload_max_bytes` (a declared length above it is refused before any byte is
//!   read, and the received bytes are counted); a declared length that would bring the data disk
//!   to its critical level (SPEC-276) is refused the same way (SPEC-325); at most
//!   [`MAX_CONCURRENT_UPLOADS`] uploads at once
//!   (`Capacity` beyond); the interface layer fails a body that sends nothing for
//!   [`UPLOAD_IDLE`];
//! - the bytes go to a temporary file (`.yulius-upload-<random>`, created exclusively, 0644) in
//!   the destination folder and are synced; then the file takes its name atomically: linked under
//!   a new name (an existing name is a `Conflict` and is never replaced), renamed over an existing
//!   regular file when the caller confirmed the overwrite (its permissions kept; a folder or a
//!   link is never replaced), or, for attachments, linked under the first free name (`name`,
//!   `name-2.ext`, …). The folder is opened once, when the upload begins, and the temporary file
//!   is created, linked and renamed only through that handle (`openat`, `linkat`, `renameat`;
//!   ADR-0174): a folder swapped for a link afterwards is never followed (#657);
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
//!   mount whatever it points at. Every folder on the way is opened from the sandbox directory
//!   with `O_NOFOLLOW`, never through a link (a contained shell, root in its sandbox, could plant
//!   one), and the upload keeps the last one's handle. Names
//!   keep letters, digits, `.`, `-`, `_` and `+` (anything else becomes `-`; no leading dot or
//!   dash), so a path needs no quoting in a terminal; at most [`MAX_ATTACHMENTS_PER_FOLDER`] per
//!   folder. Files dropped on the new-task composer wait in `…/attachments/drafts/<user>/` and are
//!   removed after a week; a deleted task's folder stays while another task's request names it and
//!   otherwise waits a week in the trash (crate::attachment_trash, SPEC-396), a shell's goes when the shell ends and
//!   at start-up for shells that are gone. A draft or a shell's folder that a task's request or
//!   an automation's prompt names stays (SPEC-413, ADR-0259). An image pasted into a person's
//!   Global Superagent is kept in its own folder, `users/<id>/superagent/attachments/`, inside its
//!   root (images only; those older than a week go at the next upload; SPEC-433, ADR-0277);
//! - every upload is audited (`file.uploaded`, `attachment.uploaded`).

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fchmod, fstatat};
use nix::unistd::{UnlinkatFlags, linkat, unlinkat};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use yulius_contracts::api::files::UploadedFile;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::sweep::{CRITICAL_PERCENT, Level};

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::files::{
    errno_error, inside, io_error, kind_at, names_in, open_dir_at, open_dirs_below, open_inside,
    real_path, relative_components, remove_tree,
};

/// Default largest upload (1 GiB, a screen recording: SPEC-325; `YULIUS_UPLOAD_MAX_MB`).
pub const DEFAULT_UPLOAD_MAX_BYTES: u64 = 1024 * 1024 * 1024;
/// Uploads in progress at once on this instance.
pub const MAX_CONCURRENT_UPLOADS: usize = 8;
/// Agent states sent, restored, saved or downloaded at once (SPEC-271 OPS-008).
pub const MAX_CONCURRENT_STATES: usize = 2;
/// Longest pause in an upload's body before it fails.
pub const UPLOAD_IDLE: Duration = Duration::from_secs(60);
/// Attachments kept per task or shell.
pub const MAX_ATTACHMENTS_PER_FOLDER: usize = 500;
/// Longest file or folder name, in bytes (the file system's limit).
const MAX_NAME_BYTES: usize = 255;
/// Longest attachment name, in characters (the extension is kept).
const MAX_ATTACHMENT_NAME_CHARS: usize = 100;
/// Names tried for an attachment before giving up (`name`, `name-2`, …).
pub(crate) const MAX_FREE_NAME_ATTEMPTS: u32 = 1000;
/// Folder of every attachment in a sandbox directory.
pub(crate) const ATTACHMENTS_DIR: &str = "attachments";
/// Shell attachment folders inspected at start-up.
pub(crate) const MAX_PRUNED_ENTRIES: usize = 10_000;

/// How long a file dropped on the composer waits for its task before it is removed; an image
/// pasted into the Global Superagent stays as long (SPEC-433 GIM-003).
const DRAFT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Extensions Claude Code (2.1.311) reads as an image when their path is pasted: all the Global
/// Superagent, without file tools, can take (SPEC-433 GIM-002). Widened together with the page's
/// `GLOBAL_IMAGE` (`filedrop.js`) and the mock's `/api/superagent/attachments`.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Uploads in progress (the concurrency bound).
pub struct UploadState {
    slots: Arc<Semaphore>,
    /// Reference fetches running at once (SPEC-267): each may write a repository's history to
    /// the data disk before its size is known.
    pub(crate) references: Arc<Semaphore>,
    /// Agent states moving at once (SPEC-271 OPS-008): each holds a packed state of up to
    /// 64 MiB in memory.
    pub(crate) states: Arc<Semaphore>,
}

impl Default for UploadState {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS)),
            references: Arc::new(Semaphore::new(
                crate::reference_repo::MAX_CONCURRENT_FETCHES,
            )),
            states: Arc::new(Semaphore::new(MAX_CONCURRENT_STATES)),
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
    /// The destination folder's handle: every file operation goes through it.
    folder: OwnedFd,
    /// The requested file name.
    name: String,
    /// The folder as reported to the caller: relative to the root (editor) or absolute as the
    /// agent sees it (attachments).
    shown_dir: String,
    attachment: bool,
    /// The temporary file's name in `folder`.
    temp: String,
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
        let _ = unlinkat(&self.folder, self.temp.as_str(), UnlinkatFlags::NoRemoveDir);
    }
}

fn too_large(limit: u64) -> AppError {
    AppError::Invalid(format!(
        "the file is larger than the upload limit of {}",
        human_size(limit)
    ))
}

/// `1 GB`, `25 MB`, `512 KB`, `100 bytes` (binary units, as `YULIUS_UPLOAD_MAX_MB`).
pub(crate) fn human_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GB", bytes / GIB)
    } else if bytes >= GIB {
        format!("{}.{} GB", bytes / GIB, (bytes % GIB) * 10 / GIB)
    } else if bytes >= MIB && bytes.is_multiple_of(MIB) {
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
pub(crate) fn candidate(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    format!("{stem}-{n}{ext}")
}

/// Opens `parts` below `base` as real directories, creating the missing ones: a link or a file
/// on the way is refused, so nothing is ever written through a link a sandbox process planted.
/// Returns the folder's path and its handle.
pub(crate) fn ensure_plain_dirs(base: &Path, parts: &[&str]) -> AppResult<(PathBuf, OwnedFd)> {
    let names: Vec<&OsStr> = parts.iter().map(OsStr::new).collect();
    let fd = open_dirs_below(base, &names, true).map_err(|err| match err {
        Errno::ELOOP | Errno::ENOTDIR => {
            AppError::Forbidden("the attachments folder is not a plain folder".into())
        }
        other => errno_error(other),
    })?;
    let path = parts
        .iter()
        .fold(base.to_path_buf(), |path, part| path.join(part));
    Ok((path, fd))
}

/// Whether `parts` below `base` are all real directories (removal never follows a link).
pub(crate) fn plain_dirs_exist(base: &Path, parts: &[&str]) -> bool {
    let mut current = base.to_path_buf();
    parts.iter().all(|part| {
        current.push(part);
        std::fs::symlink_metadata(&current).is_ok_and(|meta| meta.file_type().is_dir())
    })
}

/// Opens the folder of an editor upload below the (canonical) `root`, creating the missing ones
/// through their parent's handle; existing ones are resolved (a link inside the root is followed)
/// and must stay inside the root, out of `.git`, and be folders. Every handle's real path is
/// checked, so a folder swapped for a link meanwhile is never used. Returns the folder's real
/// path and its handle.
fn ensure_dirs(root: &Path, dirs: &[&str]) -> AppResult<(PathBuf, OwnedFd)> {
    let folder = OFlag::O_RDONLY | OFlag::O_DIRECTORY;
    let mut fd = open_inside(root, root, folder)?;
    let mut current = root.to_path_buf();
    for part in dirs {
        let next = current.join(part);
        fd = match std::fs::symlink_metadata(&next) {
            Ok(_) => {
                let resolved = std::fs::canonicalize(&next).map_err(|e| io_error(&e))?;
                if !inside(root, &resolved) {
                    return Err(AppError::Forbidden(crate::files::OUTSIDE.into()));
                }
                if !resolved.is_dir() {
                    return Err(AppError::Conflict(format!(
                        "{part} is a file, not a folder"
                    )));
                }
                open_inside(root, &resolved, folder)?
            }
            // Created through the parent's handle; one created meanwhile (another upload of the
            // same folder) is opened the same way, never through a link.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                open_dir_at(&fd, OsStr::new(part), true).map_err(|err| match err {
                    Errno::ELOOP | Errno::ENOTDIR => {
                        AppError::Conflict(format!("{part} changed during the upload"))
                    }
                    other => errno_error(other),
                })?
            }
            Err(err) => return Err(io_error(&err)),
        };
        current = real_path(&fd)?;
        if !inside(root, &current) {
            return Err(AppError::Forbidden(crate::files::OUTSIDE.into()));
        }
    }
    Ok((current, fd))
}

/// Links `temp` as `name` in `folder` unless `name` exists. Returns whether it was placed. Where
/// the link fails (a file system without hard links), it renames with `RENAME_NOREPLACE` instead,
/// as atomic as the link (SPEC-402).
fn link_new(folder: &OwnedFd, temp: &str, name: &str) -> std::io::Result<bool> {
    link_new_in((folder, temp), folder, name)
}

/// [`link_new`] from the folder `from` into the folder `to` (a link is never followed).
pub(crate) fn link_new_in(
    (from, source): (&OwnedFd, &str),
    to: &OwnedFd,
    name: &str,
) -> std::io::Result<bool> {
    match linkat(from, source, to, name, AtFlags::empty()) {
        Ok(()) => Ok(true),
        Err(Errno::EEXIST) => Ok(false),
        Err(link) => {
            fallback_placing();
            rename_new((from, source), to, name, link)
        }
    }
}

/// Renames `source` of `from` to `name` of `to` unless `name` exists, in one step
/// (`RENAME_NOREPLACE`): whether it was placed. A file system without the flag fails the
/// placement; nothing checks and then renames (#808).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn rename_new(
    (from, source): (&OwnedFd, &str),
    to: &OwnedFd,
    name: &str,
    _link: Errno,
) -> std::io::Result<bool> {
    use nix::fcntl::{RenameFlags, renameat2};
    match renameat2(from, source, to, name, RenameFlags::RENAME_NOREPLACE) {
        Ok(()) => Ok(true),
        Err(Errno::EEXIST) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Without an atomic no-replace rename (development hosts), the link's error stands.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn rename_new(
    _source: (&OwnedFd, &str),
    _to: &OwnedFd,
    _name: &str,
    link: Errno,
) -> std::io::Result<bool> {
    Err(link.into())
}

#[cfg(test)]
thread_local! {
    /// Test-only pause point: runs once where [`link_new_in`]'s fallback places the file.
    static FALLBACK_PLACING: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// [`link_new_in`]'s fallback is about to place the file (a pause point for this thread's test).
fn fallback_placing() {
    #[cfg(test)]
    if let Some(hook) = FALLBACK_PLACING.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

/// The type of the entry `name` in `folder` (a link is not followed), `None` when there is none.
fn kind_in(folder: &OwnedFd, name: &str) -> AppResult<Option<SFlag>> {
    match fstatat(folder, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT,
        )),
        Err(Errno::ENOENT) => Ok(None),
        Err(err) => Err(errno_error(err)),
    }
}

/// Gives the synced temporary file `temp` of `folder` its name as `placement` says, through the
/// folder's handle only; returns the name and whether it replaced a file. `file` is the temporary
/// file's own handle (its mode is set on it, never by path).
fn place(
    folder: &OwnedFd,
    temp: &str,
    wanted: &str,
    placement: Placement,
    file: std::fs::File,
) -> AppResult<(String, bool)> {
    let placed = match placement {
        Placement::Create => {
            if !link_new(folder, temp, wanted).map_err(|e| upload_io_error(&e))? {
                return Err(AppError::Conflict(format!("{wanted} exists")));
            }
            (wanted.to_owned(), false)
        }
        Placement::Overwrite => {
            let replaced = match fstatat(folder, wanted, AtFlags::AT_SYMLINK_NOFOLLOW) {
                Ok(stat) => {
                    let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
                    if kind == SFlag::S_IFLNK {
                        return Err(AppError::Conflict(format!(
                            "{wanted} is a link; it is not replaced"
                        )));
                    }
                    if kind == SFlag::S_IFDIR {
                        return Err(AppError::Conflict(format!(
                            "a folder named {wanted} exists"
                        )));
                    }
                    let _ = fchmod(&file, Mode::from_bits_truncate(stat.st_mode & 0o7777));
                    true
                }
                Err(Errno::ENOENT) => false,
                Err(err) => return Err(errno_error(err)),
            };
            renameat(folder, temp, folder, wanted).map_err(|e| upload_io_error(&e.into()))?;
            (wanted.to_owned(), replaced)
        }
        Placement::FreeName => {
            let mut placed = None;
            for n in 1..=MAX_FREE_NAME_ATTEMPTS {
                let name = candidate(wanted, n);
                if link_new(folder, temp, &name).map_err(|e| upload_io_error(&e))? {
                    placed = Some(name);
                    break;
                }
            }
            let name =
                placed.ok_or_else(|| AppError::Conflict(format!("no free name for {wanted}")))?;
            (name, false)
        }
    };
    drop(file);
    let _ = unlinkat(folder, temp, UnlinkatFlags::NoRemoveDir);
    sync_dir(folder);
    Ok(placed)
}

/// Entries of `folder` (`.` and `..` left out), counted through its handle up to `limit`.
pub(crate) fn count_entries(folder: &OwnedFd, limit: usize) -> AppResult<usize> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    let mut listing =
        nix::dir::Dir::openat(folder, ".", flags, Mode::empty()).map_err(errno_error)?;
    Ok(listing
        .iter()
        .filter_map(Result::ok)
        .filter(|entry| !matches!(entry.file_name().to_bytes(), b"." | b".."))
        .take(limit)
        .count())
}

/// Whether `stat` is a regular file older than [`DRAFT_MAX_AGE`] (a draft the prune may take).
fn old_draft(kind: SFlag, stat: &nix::sys::stat::FileStat) -> bool {
    let modified =
        std::time::UNIX_EPOCH + Duration::from_secs(u64::try_from(stat.st_mtime).unwrap_or(0));
    kind == SFlag::S_IFREG && modified.elapsed().is_ok_and(|age| age > DRAFT_MAX_AGE)
}

/// The entries of a drafts folder other than drafts older than a week, at most `limit`, among the
/// first [`MAX_PRUNED_ENTRIES`] listed (past that the count is partial). Blocks.
fn count_waiting_drafts(folder: &OwnedFd, limit: usize) -> AppResult<usize> {
    Ok(names_in(folder, MAX_PRUNED_ENTRIES)
        .map_err(errno_error)?
        .iter()
        .filter(|name| !kind_at(folder, name).is_ok_and(|(kind, stat)| old_draft(kind, &stat)))
        .take(limit)
        .count())
}

/// Removes the regular files of `folder` older than a week, among the first
/// [`MAX_PRUNED_ENTRIES`] listed, through its handle (an image pasted into the Global Superagent,
/// which Claude Code read when it was pasted; SPEC-433 GIM-003). Blocks; best effort.
fn remove_old_files(folder: &OwnedFd) {
    let Ok(names) = names_in(folder, MAX_PRUNED_ENTRIES) else {
        return;
    };
    for name in names {
        if kind_at(folder, &name).is_ok_and(|(kind, stat)| old_draft(kind, &stat)) {
            let _ = unlinkat(folder, name.as_os_str(), UnlinkatFlags::NoRemoveDir);
        }
    }
}

/// Makes a rename or link durable (best effort: not every file system syncs directories).
fn sync_dir(folder: &OwnedFd) {
    let _ = nix::unistd::fsync(folder);
}

/// Who an attachment belongs to.
enum Owner<'a> {
    Task(&'a TaskId),
    Shell(&'a str),
    /// Files dropped on the new-task composer, before any task exists (one folder per user).
    Draft(&'a str),
    /// Images pasted into a person's Global Superagent: in its own folder, below the users'
    /// folder rather than a sandbox's (SPEC-433).
    Global(&'a str),
}

impl Owner<'_> {
    /// The folders below the base: a sandbox's folder, or for [`Owner::Global`] the users'.
    fn parts(&self) -> [&str; 3] {
        match self {
            Self::Task(id) => [ATTACHMENTS_DIR, "tasks", id.as_str()],
            Self::Shell(id) => [ATTACHMENTS_DIR, "shells", id],
            Self::Draft(user) => [ATTACHMENTS_DIR, "drafts", user],
            // `App::global_superagent_dir`'s `attachments`.
            Self::Global(user) => [user, "superagent", ATTACHMENTS_DIR],
        }
    }

    fn audit_target(&self) -> String {
        match self {
            Self::Task(id) => format!("task:{}", id.as_str()),
            Self::Shell(id) => format!("shell:{id}"),
            Self::Draft(user) => format!("draft:{user}"),
            Self::Global(user) => format!("superagent:{user}"),
        }
    }
}

impl App {
    fn upload_actor(ctx: &AuthContext) -> String {
        ctx.user.label()
    }

    /// Refuses a declared length above the limit, or one that would bring the data disk to its
    /// critical level (SPEC-325 UPL-002; an unknown disk refuses nothing).
    async fn check_declared(&self, declared: Option<u64>) -> AppResult<()> {
        let Some(len) = declared else {
            return Ok(());
        };
        if len > self.config.upload_max_bytes {
            return Err(too_large(self.config.upload_max_bytes));
        }
        if let Some(room) = self.disk_room_if_short(len).await {
            return Err(AppError::Invalid(format!(
                "the server's disk has no room for this file: only {} can be added before \
                 it is {CRITICAL_PERCENT} % full",
                human_size(room)
            )));
        }
        Ok(())
    }

    /// When adding `len` bytes would bring the data disk to its critical level (SPEC-276), what
    /// can still be added before it (not all the free space); `None` when they fit or the disk is
    /// unknown (uploads, SPEC-325 UPL-002; reference fetches, SPEC-349 RFB-002).
    pub(crate) async fn disk_room_if_short(&self, len: u64) -> Option<u64> {
        let (total, used) = self.data_disk().await?;
        if Level::of(used.saturating_add(len), total) != Level::Critical {
            return None;
        }
        let room = u128::from(total) * u128::from(CRITICAL_PERCENT) / 100;
        Some(u64::try_from(room).unwrap_or(u64::MAX).saturating_sub(used))
    }

    pub(crate) fn upload_slot(&self) -> AppResult<OwnedSemaphorePermit> {
        Arc::clone(&self.uploads.slots)
            .try_acquire_owned()
            .map_err(|_| {
                AppError::Capacity("too many uploads at once; try again in a moment".into())
            })
    }

    /// The sandbox directory as processes see it (absolute; containment binds it at this path).
    pub(crate) fn sandbox_dir_absolute(&self, sandbox: &SandboxId) -> PathBuf {
        let dir = self.sandbox_dir(sandbox);
        std::path::absolute(&dir).unwrap_or(dir)
    }

    /// `audit` is the actor and the audit target.
    async fn new_upload(
        &self,
        slot: OwnedSemaphorePermit,
        (dir, folder): (PathBuf, OwnedFd),
        (name, shown_dir): (String, String),
        placement: Placement,
        (actor, audit_target): (String, String),
    ) -> AppResult<Upload> {
        crate::race::reached(crate::race::Point::UploadFolder, &dir);
        let random = self.deps.crypto.random_bytes();
        let temp = format!(
            ".yulius-upload-{}",
            hex_token(random.get(..8).unwrap_or(&random))
        );
        let fd = openat(
            &folder,
            temp.as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o644),
        )
        .map_err(|err| upload_io_error(&err.into()))?;
        let file = tokio::fs::File::from_std(std::fs::File::from(fd));
        Ok(Upload {
            dir,
            folder,
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
        let target = self.file_target(ctx, sandbox, root, path, true).await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid("a file path is required".into()));
        }
        check_components(&target.relative)?;
        self.check_declared(declared).await?;
        let parts: Vec<&str> = target.relative.split('/').collect();
        let (name, dirs) = parts
            .split_last()
            .ok_or_else(|| AppError::Invalid("a file path is required".into()))?;
        // A slot first: nothing is created for an upload that cannot run now.
        let slot = self.upload_slot()?;
        let dir = ensure_dirs(&target.root, dirs)?;
        // Refuse an existing name before the body is sent; the final step checks again.
        if let Some(kind) = kind_in(&dir.1, name)? {
            if kind == SFlag::S_IFLNK {
                return Err(AppError::Conflict(format!(
                    "{name} is a link; it is not replaced"
                )));
            }
            if kind == SFlag::S_IFDIR {
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
            Self::upload_actor(ctx),
            &record.sandbox_id,
            &Owner::Task(&record.id),
            name,
            (declared, false),
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
        self.begin_attachment(
            Self::upload_actor(ctx),
            &sandbox_id,
            &Owner::Shell(&id),
            name,
            (declared, false),
        )
        .await
    }

    /// Starts an image pasted (or dropped) into the caller's running Global Superagent, kept in
    /// its own folder, inside its root at the same path; images older than a week there are
    /// removed first (SPEC-433).
    ///
    /// # Errors
    /// `NotFound` while the caller's Global Superagent is not running; `Forbidden` for a disabled
    /// account; `Invalid` for a name that is not an image Claude Code reads, before the body is
    /// read, and otherwise as [`App::begin_task_attachment`].
    pub async fn begin_global_superagent_attachment(
        &self,
        ctx: &AuthContext,
        name: &str,
        declared: Option<u64>,
    ) -> AppResult<Upload> {
        if self.running_global_superagent(&ctx.user.id).is_none() {
            return Err(AppError::NotFound);
        }
        if ctx.user.disabled_at.is_some() {
            return Err(AppError::Forbidden("this account is disabled".into()));
        }
        let stored = attachment_name(name);
        let ext = split_extension(&stored)
            .1
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            return Err(AppError::Invalid(
                "the Global Superagent takes images only (PNG, JPEG, GIF or WebP): it has no \
                 file tools to open other files"
                    .into(),
            ));
        }
        let users = std::fs::canonicalize(self.users_dir())
            .map_err(|_| AppError::Conflict("the Global Superagent has no folder yet".into()))?;
        self.begin_attachment_in(
            Self::upload_actor(ctx),
            (&users, &users),
            &Owner::Global(ctx.user.id.as_str()),
            name,
            (declared, false),
        )
        .await
    }

    /// Starts an attachment for the new-task composer of `sandbox` (write access to it): the
    /// file waits in the caller's drafts folder, outside every worktree, until the prompt that
    /// names its path starts a task. Drafts older than a week that no request names are removed
    /// first (SPEC-413).
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
        self.prune_old_drafts(&sandbox_id, user).await;
        self.begin_attachment(
            Self::upload_actor(ctx),
            &sandbox_id,
            &Owner::Draft(user),
            name,
            (declared, false),
        )
        .await
    }

    /// Removes the files of a user's drafts folder that are older than [`DRAFT_MAX_AGE`] and that
    /// no task's request or automation's prompt names (SPEC-413 ANR-002); keeps them all when
    /// those cannot be read.
    async fn prune_old_drafts(&self, sandbox: &SandboxId, user: &str) {
        let Ok(dir) = std::fs::canonicalize(self.sandbox_dir(sandbox)) else {
            return;
        };
        let owner = Owner::Draft(user);
        let parts = owner.parts();
        crate::race::reached(
            crate::race::Point::DraftsPrune,
            &parts.iter().fold(dir.clone(), |path, part| path.join(part)),
        );
        // Listed and pruned through the folder's handle, reached without following links (#660).
        let names = parts.map(OsStr::new);
        let Ok(folder) = open_dirs_below(&dir, &names, false) else {
            return;
        };
        let Ok(entries) = names_in(&folder, MAX_PRUNED_ENTRIES) else {
            return;
        };
        let old: Vec<_> = entries
            .into_iter()
            .filter(|name| kind_at(&folder, name).is_ok_and(|(kind, stat)| old_draft(kind, &stat)))
            .collect();
        if old.is_empty() {
            return;
        }
        let namers = match self.namers(sandbox).await {
            Ok(namers) => namers,
            Err(err) => {
                tracing::warn!(%sandbox, error = %err, "old drafts kept: tasks unreadable");
                return;
            }
        };
        for name in old {
            // A name that is not UTF-8 cannot be in a request.
            let named = name.to_str().is_some_and(|name| {
                namers.name(&format!("{ATTACHMENTS_DIR}/drafts/{user}/{name}"))
            });
            if !named {
                let _ = unlinkat(&folder, name.as_os_str(), UnlinkatFlags::NoRemoveDir);
            }
        }
    }

    /// Places the server's own file `source` among `task`'s attachments as `name`, replacing an
    /// earlier regular file of that name (never a link or a folder), streamed through the
    /// attachment path's handles (a reference bundle, SPEC-267): at most `limit` bytes, audited
    /// for `actor`.
    ///
    /// # Errors
    /// `Invalid` above `limit`; otherwise as [`App::begin_task_attachment`] and
    /// [`App::finish_upload`]; I/O errors reading `source`.
    pub(crate) async fn attach_server_file(
        &self,
        (sandbox, task): (&SandboxId, &TaskId),
        name: &str,
        source: &Path,
        limit: u64,
        actor: String,
    ) -> AppResult<UploadedFile> {
        use tokio::io::AsyncReadExt as _;
        let mut file = tokio::fs::File::open(source)
            .await
            .map_err(|e| upload_io_error(&e))?;
        let size = file
            .metadata()
            .await
            .map_err(|e| upload_io_error(&e))?
            .len();
        if size > limit {
            return Err(too_large(limit));
        }
        let mut upload = self
            .begin_attachment(actor, sandbox, &Owner::Task(task), name, (None, true))
            .await?;
        upload.limit = limit;
        upload.placement = Placement::Overwrite;
        let mut chunk = vec![0u8; 256 * 1024];
        loop {
            let read = file
                .read(&mut chunk)
                .await
                .map_err(|e| upload_io_error(&e))?;
            if read == 0 {
                break;
            }
            upload.write(chunk.get(..read).unwrap_or_default()).await?;
        }
        self.finish_upload(upload).await
    }

    /// `replaces`: the caller places the file over an existing one of that name (see
    /// [`App::attach_server_file`]).
    async fn begin_attachment(
        &self,
        actor: String,
        sandbox: &SandboxId,
        owner: &Owner<'_>,
        name: &str,
        (declared, replaces): (Option<u64>, bool),
    ) -> AppResult<Upload> {
        let sandbox_dir = std::fs::canonicalize(self.sandbox_dir(sandbox))
            .map_err(|_| AppError::Conflict("the sandbox has no folder yet".into()))?;
        self.begin_attachment_in(
            actor,
            (&sandbox_dir, &self.sandbox_dir_absolute(sandbox)),
            owner,
            name,
            (declared, replaces),
        )
        .await
    }

    /// [`App::begin_attachment`] below `base` (canonical, opened by path), whose stored paths
    /// are answered below `shown` (the same folder as its runs see it).
    async fn begin_attachment_in(
        &self,
        actor: String,
        (base, shown): (&Path, &Path),
        owner: &Owner<'_>,
        name: &str,
        (declared, replaces): (Option<u64>, bool),
    ) -> AppResult<Upload> {
        self.check_declared(declared).await?;
        let name = attachment_name(name);
        let slot = self.upload_slot()?;
        let dir = ensure_plain_dirs(base, &owner.parts())?;
        if matches!(owner, Owner::Global(_)) {
            remove_old_files(&dir.1);
        }
        // A name that replaces a regular file adds none, so a full folder still takes it.
        let replacing = replaces && kind_in(&dir.1, &name)? == Some(SFlag::S_IFREG);
        let count = match owner {
            // Drafts older than a week stay only while a request names them (SPEC-413): they are
            // tasks' files now, so the bound counts the ones still waiting for a task.
            Owner::Draft(_) => count_waiting_drafts(&dir.1, MAX_ATTACHMENTS_PER_FOLDER + 1)?,
            _ => count_entries(&dir.1, MAX_ATTACHMENTS_PER_FOLDER + 1)?,
        };
        if !replacing && count >= MAX_ATTACHMENTS_PER_FOLDER {
            return Err(AppError::Conflict(format!(
                "at most {MAX_ATTACHMENTS_PER_FOLDER} attachments are kept per terminal, or drafts \
                 waiting for a task"
            )));
        }
        let shown_dir = owner
            .parts()
            .iter()
            .fold(shown.to_path_buf(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned();
        self.new_upload(
            slot,
            dir,
            (name, shown_dir),
            Placement::FreeName,
            (actor, owner.audit_target()),
        )
        .await
    }

    /// Completes an upload: syncs it, gives it its name atomically (see the module docs) and
    /// audits it.
    ///
    /// # Errors
    /// `Conflict` when the name exists (and was not to be replaced) or is a folder or a link;
    /// I/O errors.
    pub async fn finish_upload(&self, mut upload: Upload) -> AppResult<UploadedFile> {
        let mut file = upload
            .file
            .take()
            .ok_or_else(|| AppError::internal("the upload was already finished"))?;
        file.flush().await.map_err(|e| upload_io_error(&e))?;
        file.sync_all().await.map_err(|e| upload_io_error(&e))?;
        let file = file.into_std().await;
        crate::race::reached(crate::race::Point::UploadReceived, &upload.dir);
        // No check of the folder's path again: everything goes through its handle (#657).
        crate::race::reached(crate::race::Point::UploadPlace, &upload.dir);
        // Off the runtime's threads: links, renames and the folder's fsync can be slow.
        let folder = upload.folder.try_clone().map_err(|e| upload_io_error(&e))?;
        let (temp, wanted, placement) =
            (upload.temp.clone(), upload.name.clone(), upload.placement);
        let (name, replaced) =
            crate::projects::blocking(move || place(&folder, &temp, &wanted, placement, file))
                .await?;
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
        let [attachments, kind, name] = owner.parts();
        crate::race::reached(
            crate::race::Point::AttachmentsRemove,
            &dir.join(attachments).join(kind).join(name),
        );
        // The owner's folder is removed through its parent's handle, reached without following
        // links; a link on the way removes nothing (#660).
        let parent = match open_dirs_below(
            &dir,
            &[OsStr::new(attachments), OsStr::new(kind)],
            false,
        ) {
            Ok(parent) => parent,
            Err(Errno::ENOENT) => return,
            Err(err) => {
                tracing::warn!(%sandbox, error = %err, "attachments folder is not a plain folder");
                return;
            }
        };
        if let Err(err) = remove_tree(&parent, OsStr::new(name)) {
            tracing::warn!(%sandbox, error = %err, "could not remove attachments");
        }
    }

    /// Removes an ended shell's attachments, unless a task's request or an automation's prompt
    /// names its folder (SPEC-413 ANR-001): then, or when those cannot be read, they stay.
    pub(crate) async fn remove_shell_attachments(&self, sandbox: &SandboxId, shell: &str) {
        // Most shells never got a file: nothing to remove, nothing to read. Only a gate: the
        // removal itself goes through handles.
        let folder = [ATTACHMENTS_DIR, "shells", shell]
            .iter()
            .fold(self.sandbox_dir(sandbox), |path, part| path.join(part));
        if std::fs::symlink_metadata(&folder)
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return;
        }
        let namers = match self.namers(sandbox).await {
            Ok(namers) => namers,
            Err(err) => {
                tracing::warn!(%sandbox, error = %err, "shell attachments kept: tasks unreadable");
                return;
            }
        };
        self.remove_unnamed_shell_attachments(sandbox, shell, &namers);
    }

    /// Removes `shell`'s attachments unless `namers` name its folder; whether it removed them.
    fn remove_unnamed_shell_attachments(
        &self,
        sandbox: &SandboxId,
        shell: &str,
        namers: &crate::attachment_trash::Namers,
    ) -> bool {
        if namers.name(&format!("{ATTACHMENTS_DIR}/shells/{shell}/")) {
            tracing::info!(%sandbox, shell, "shell attachments kept: a request names them");
            return false;
        }
        self.remove_attachments(sandbox, &Owner::Shell(shell));
        true
    }

    /// Removes the attachment folders of shells that no longer run (they ended while no server
    /// was there to see it) and that nothing names (SPEC-413 ANR-001). Returns how many were
    /// removed. Bounded by [`MAX_PRUNED_ENTRIES`].
    pub(crate) async fn prune_shell_attachments(&self) -> usize {
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
            // Listed through the folder's handle, reached without following links (#660).
            let Ok(folder) = open_dirs_below(
                &base,
                &[OsStr::new(ATTACHMENTS_DIR), OsStr::new("shells")],
                false,
            ) else {
                continue;
            };
            let live: Vec<String> = self
                .sandbox_shells(&id)
                .iter()
                .map(|shell| shell.info().id)
                .collect();
            let Ok(shells) = names_in(&folder, MAX_PRUNED_ENTRIES) else {
                continue;
            };
            let gone: Vec<String> = shells
                .into_iter()
                .filter_map(|shell| shell.into_string().ok())
                .filter(|name| !live.contains(name))
                .collect();
            if gone.is_empty() {
                continue;
            }
            let namers = match self.namers(&id).await {
                Ok(namers) => namers,
                Err(err) => {
                    tracing::warn!(sandbox = %id, error = %err, "shell attachments kept: tasks unreadable");
                    continue;
                }
            };
            for name in gone {
                if self.remove_unnamed_shell_attachments(&id, &name, &namers) {
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

    /// SPEC-402 VUG-002 (#808): where the link fails (here a folder, which `linkat` refuses), a
    /// name another writer takes just before the fallback places the file is kept, and the call
    /// says the file was not placed.
    #[test]
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    fn a_name_taken_while_the_fallback_places_is_never_replaced()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().to_path_buf();
        std::fs::create_dir(base.join("upload.tmp"))?;
        std::fs::write(base.join("upload.tmp/ours"), "ours")?;
        let folder = crate::files::open_dirs_below(&base, &[], false)?;
        let taken = base.join("photo.png");
        FALLBACK_PLACING.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let _ = std::fs::create_dir(&taken);
            }));
        });
        let placed = link_new_in((&folder, "upload.tmp"), &folder, "photo.png");
        assert!(base.join("photo.png").is_dir(), "the racer took the name");
        assert_eq!(placed.ok(), Some(false));
        assert!(
            !base.join("photo.png/ours").exists(),
            "the racer's entry was not replaced"
        );
        assert!(base.join("upload.tmp/ours").exists());
        Ok(())
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
        // The default limit and a disk's free space (SPEC-325), not "1024 MB" or "9536.7 MB".
        assert_eq!(human_size(1024 * 1024 * 1024), "1 GB");
        assert_eq!(human_size(1536 * 1024 * 1024), "1.5 GB");
        assert_eq!(human_size(10_000_000_000), "9.3 GB");
        assert_eq!(human_size(1023 * 1024 * 1024), "1023 MB");
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(10), "10 bytes");
    }

    #[test]
    fn plain_dirs_refuse_links() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let base = dir.path().canonicalize()?;
        let (made, _) = ensure_plain_dirs(&base, &["attachments", "tasks", "t1"])?;
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
