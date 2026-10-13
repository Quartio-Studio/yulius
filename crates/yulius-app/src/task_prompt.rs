//! A task's original prompt (SPEC-043, ADR-0033): the text it started with and the files attached
//! to it, read on demand by whoever may see the task.
//!
//! The prompt is stored as the composer sent it: the text, then one `@path` line per attached
//! file ([`Request::parse`] splits them, as the session start does). A line can name any path on
//! the host, so a file is only ever served from the task's own attachment folders: its creator's
//! composer drafts (`attachments/drafts/<user>/`) and the task's terminal attachments
//! (`attachments/tasks/<task>/`), named exactly as an upload reports them (no `..`, no other
//! folder). Those folders must be real folders, not links (a contained process, root in its
//! sandbox, can plant one); the file is opened without following a link or blocking on a pipe,
//! checked again through its handle (a regular file within the upload limit) and, where
//! `/proc/self/fd` exists, at the path it was opened at, so a folder swapped for a link between
//! the checks and the open is refused too; on Linux a file whose opened path cannot be read there
//! is refused. Reads have their own concurrency bound, waited for briefly (uploads have theirs),
//! and a file is sent in chunks while its read slot is held (SPEC-080, ADR-0064).

use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use yulius_contracts::api::tasks::{PromptAttachment, TaskPrompt};
use yulius_domain::permission::Action;
use yulius_domain::request::{Request, is_image};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::uploads::{ATTACHMENTS_DIR, MAX_ATTACHMENTS_PER_FOLDER, UPLOAD_IDLE, plain_dirs_exist};

/// Attached files of one prompt checked on disk; any further ones are listed as not available.
const MAX_CHECKED: usize = MAX_ATTACHMENTS_PER_FOLDER;
/// Largest piece of an attached file held in memory while it is sent.
pub const ATTACHMENT_CHUNK: usize = 64 * 1024;
/// Attached files read at once on this instance (their own bound: uploads have theirs).
pub const MAX_CONCURRENT_ATTACHMENT_READS: usize = 16;
/// Longest wait for a read slot before a read fails with `Capacity`.
pub const ATTACHMENT_READ_WAIT: Duration = Duration::from_secs(30);
/// Longest pause in sending an attached file before it gives its read slot back and fails.
pub const ATTACHMENT_IDLE: Duration = UPLOAD_IDLE;
/// Whether an opened file must be found at its real path through `/proc/self/fd`: on Linux, where
/// containment exists and `/proc` is mounted; elsewhere there is no such link (ADR-0033).
const OPENED_AT_REQUIRED: bool = cfg!(target_os = "linux");

/// Attached files being read (the concurrency bound of reads).
pub struct AttachmentReads {
    slots: Arc<Semaphore>,
}

impl Default for AttachmentReads {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ATTACHMENT_READS)),
        }
    }
}

impl App {
    /// The prompt task `id` started with, its attached files listed by name.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks or without read access.
    pub async fn task_prompt(&self, ctx: &AuthContext, id: &str) -> AppResult<TaskPrompt> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let owners = Owners::of(self, &task);
        let limit = self.config.upload_max_bytes;
        let paths: Vec<String> = request
            .attachments
            .iter()
            .take(MAX_CHECKED)
            .cloned()
            .collect();
        // Checked on disk off the async runtime.
        let available = tokio::task::spawn_blocking(move || {
            let folders = owners.folders();
            paths
                .iter()
                .map(|path| {
                    served_path(&folders, path).is_some_and(|(file, _)| {
                        std::fs::symlink_metadata(file)
                            .is_ok_and(|meta| meta.file_type().is_file() && meta.len() <= limit)
                    })
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| AppError::Internal(format!("checking attached files: {e}")))?;
        let attachments = request
            .attachments
            .iter()
            .enumerate()
            .map(|(i, path)| PromptAttachment {
                name: file_name(path),
                image: is_image(path),
                available: available.get(i).copied().unwrap_or(false),
            })
            .collect();
        Ok(TaskPrompt {
            text: request.text,
            attachments,
        })
    }

    /// Attachment `index` of task `id`'s prompt, opened to be sent in chunks while it holds a read
    /// slot ([`AttachedFile::next_chunk`]).
    ///
    /// # Errors
    /// `NotFound` for unknown tasks, without read access, and for an attachment out of range, gone
    /// or not served (not in the task's attachment folders, through a link, not a regular file,
    /// over the upload limit, its opened path unconfirmed on Linux); `Capacity` when no read slot
    /// frees within [`ATTACHMENT_READ_WAIT`].
    pub async fn task_prompt_attachment(
        &self,
        ctx: &AuthContext,
        id: &str,
        index: usize,
    ) -> AppResult<AttachedFile> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let path = request
            .attachments
            .get(index)
            .filter(|_| index < MAX_CHECKED)
            .ok_or(AppError::NotFound)?
            .clone();
        let owners = Owners::of(self, &task);
        let slot = read_slot(&self.attachment_reads.slots, ATTACHMENT_READ_WAIT).await?;
        let limit = self.config.upload_max_bytes;
        let name = file_name(&path);
        let content_type = image_type(&path);
        // Checked and opened off the async runtime.
        let (opened, len) = tokio::task::spawn_blocking(move || {
            let folders = owners.folders();
            let (file, real) = served_path(&folders, &path)?;
            open_attached(file, &real, limit, opened_at)
        })
        .await
        .map_err(|e| AppError::Internal(format!("opening an attached file: {e}")))?
        .ok_or(AppError::NotFound)?;
        Ok(AttachedFile::new(
            (name, content_type),
            (tokio::fs::File::from_std(opened), len),
            slot,
            ATTACHMENT_IDLE,
        ))
    }
}

/// Where a task's attachment folders are: its sandbox directory, its creator and its id.
struct Owners {
    sandbox_dir: PathBuf,
    created_by: String,
    task: String,
}

impl Owners {
    fn of(app: &App, task: &TaskRecord) -> Self {
        Self {
            sandbox_dir: app.sandbox_dir(&task.sandbox_id),
            created_by: task.created_by.as_str().to_owned(),
            task: task.id.as_str().to_owned(),
        }
    }

    /// The folders a file of the task's prompt may be in, as an upload names them (the sandbox
    /// directory as processes see it, and resolved), when they are real folders, not links; each
    /// with where it really is (below the resolved sandbox directory, which is trusted). Blocks.
    fn folders(&self) -> Vec<(PathBuf, PathBuf)> {
        let absolute =
            std::path::absolute(&self.sandbox_dir).unwrap_or_else(|_| self.sandbox_dir.clone());
        let Ok(resolved) = std::fs::canonicalize(&absolute) else {
            return Vec::new();
        };
        let mut bases = vec![absolute.clone()];
        if resolved != absolute {
            bases.push(resolved.clone());
        }
        let owners = [
            [ATTACHMENTS_DIR, "drafts", self.created_by.as_str()],
            [ATTACHMENTS_DIR, "tasks", self.task.as_str()],
        ];
        let mut folders = Vec::new();
        for base in &bases {
            for parts in &owners {
                if plain_dirs_exist(base, parts) {
                    let under =
                        |root: &PathBuf| parts.iter().fold(root.clone(), |p, part| p.join(part));
                    folders.push((under(base), under(&resolved)));
                }
            }
        }
        folders
    }
}

/// A slot of `slots`, waited for at most `wait`.
async fn read_slot(slots: &Arc<Semaphore>, wait: Duration) -> AppResult<OwnedSemaphorePermit> {
    match tokio::time::timeout(wait, Arc::clone(slots).acquire_owned()).await {
        Ok(Ok(slot)) => Ok(slot),
        _ => Err(AppError::Capacity(
            "too many attached files are being read; try again in a moment".into(),
        )),
    }
}

/// `path` when it names a file directly inside one of `folders` (no `.` or `..` on the way),
/// with where that file really is.
fn served_path<'a>(folders: &[(PathBuf, PathBuf)], path: &'a str) -> Option<(&'a Path, PathBuf)> {
    let file = Path::new(path);
    let plain = file
        .components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    if !plain {
        return None;
    }
    let (parent, name) = (file.parent()?, file.file_name()?);
    let (_, real) = folders.iter().find(|(named, _)| named == parent)?;
    Some((file, real.join(name)))
}

/// Where `/proc/self/fd` says `file` was opened.
fn opened_at(file: &std::fs::File) -> std::io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

/// Opens `file` without following a link or blocking on a pipe; with its length, unless it is not
/// a regular file within `limit` or `opened_at` says it was not opened at `real` (on Linux, also
/// when `opened_at` cannot say). Blocks.
fn open_attached(
    file: &Path,
    real: &Path,
    limit: u64,
    opened_at: impl FnOnce(&std::fs::File) -> std::io::Result<PathBuf>,
) -> Option<(std::fs::File, u64)> {
    use nix::fcntl::OFlag;
    let opened = std::fs::File::options()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(file)
        .ok()?;
    let meta = opened.metadata().ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    // A folder on the way swapped for a link after the checks: the file opened is elsewhere.
    let confirmed = match opened_at(&opened) {
        Ok(at) => at == real,
        Err(_) => !OPENED_AT_REQUIRED,
    };
    confirmed.then_some((opened, meta.len()))
}

/// An attached file being sent: read in chunks of at most [`ATTACHMENT_CHUNK`] while it holds a
/// read slot. Left unread for its idle bound, it gives the slot back and fails.
#[derive(Debug)]
pub struct AttachedFile {
    /// File name.
    pub name: String,
    /// Its image type when Claude Code takes it as an image; `None` for any other file.
    pub content_type: Option<&'static str>,
    /// Its length when it was opened: exactly this many bytes are sent.
    pub len: u64,
    file: tokio::fs::File,
    remaining: u64,
    held: Arc<Mutex<Held>>,
}

/// The read slot of an [`AttachedFile`] and when it was last read.
#[derive(Debug)]
struct Held {
    slot: Option<OwnedSemaphorePermit>,
    last: Instant,
}

impl AttachedFile {
    fn new(
        (name, content_type): (String, Option<&'static str>),
        (file, len): (tokio::fs::File, u64),
        slot: OwnedSemaphorePermit,
        idle: Duration,
    ) -> Self {
        let held = Arc::new(Mutex::new(Held {
            slot: Some(slot),
            last: Instant::now(),
        }));
        tokio::spawn(release_when_idle(Arc::downgrade(&held), idle));
        Self {
            name,
            content_type,
            len,
            file,
            remaining: len,
            held,
        }
    }

    /// The next piece of the file, at most [`ATTACHMENT_CHUNK`] bytes; `None` once its length is
    /// sent.
    ///
    /// # Errors
    /// Reading fails, the file ends before its length, or it was left unread for its idle bound
    /// (its read slot is given back then).
    pub async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.touch()?;
        let want = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(ATTACHMENT_CHUNK);
        let mut chunk = vec![0; want];
        let read = self.file.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        chunk.truncate(read);
        self.remaining = self
            .remaining
            .saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
        self.touch()?;
        Ok(Some(chunk))
    }

    /// Marks the file as read now; fails once its slot was given back.
    fn touch(&self) -> std::io::Result<()> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.slot.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the attached file was left unread too long",
            ));
        }
        held.last = Instant::now();
        Ok(())
    }
}

/// Gives the read slot back once the file is left unread for `idle`; ends with the file.
async fn release_when_idle(held: Weak<Mutex<Held>>, idle: Duration) {
    loop {
        let deadline = {
            let Some(held) = held.upgrade() else { return };
            let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
            if held.slot.is_none() {
                return;
            }
            let deadline = held.last + idle;
            if Instant::now() >= deadline {
                held.slot = None;
                return;
            }
            deadline
        };
        tokio::time::sleep_until(deadline).await;
    }
}

/// The image type of a file Claude Code takes as an image ([`is_image`]), by its extension.
fn image_type(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit_once('.')?.1;
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The last component of an attached file's path.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn every_image_claude_code_pastes_has_an_image_type() {
        for path in ["/a/b.png", "/a/b.JPG", "/a/b.jpeg", "/a/b.gif", "/a/b.webp"] {
            assert!(is_image(path));
            assert!(image_type(path).is_some(), "{path}");
        }
        assert_eq!(image_type("/a/b.svg"), None);
        assert_eq!(image_type("/a/png"), None);
        assert_eq!(file_name("/a/b c.png"), "b c.png");
        let folders = [(
            PathBuf::from("/s/attachments/drafts/u"),
            PathBuf::from("/real/s/attachments/drafts/u"),
        )];
        assert_eq!(
            served_path(&folders, "/s/attachments/drafts/u/a.png").map(|(_, real)| real),
            Some(PathBuf::from("/real/s/attachments/drafts/u/a.png"))
        );
        for path in [
            "/s/attachments/drafts/u/../v/a.png",
            "/s/attachments/drafts/u/x/a.png",
            "/s/attachments/drafts/a.png",
            "/s/attachments/drafts/u",
        ] {
            assert!(served_path(&folders, path).is_none(), "{path}");
        }
    }

    /// A file in a new temporary folder, with where it really is.
    fn temp_file(
        content: &[u8],
    ) -> Result<(tempfile::TempDir, PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.png");
        std::fs::write(&file, content)?;
        let real = std::fs::canonicalize(&file)?;
        Ok((dir, file, real))
    }

    #[test]
    fn on_linux_a_file_whose_opened_path_cannot_be_read_is_refused() -> TestResult {
        let (_dir, file, real) = temp_file(b"png")?;
        let unreadable = |_: &std::fs::File| Err(std::io::Error::other("no /proc"));
        assert_eq!(
            open_attached(&file, &real, 1024, unreadable).is_some(),
            !cfg!(target_os = "linux")
        );
        let there = open_attached(&file, &real, 1024, |_| Ok(real.clone()));
        assert_eq!(there.map(|(_, len)| len), Some(3));
        let elsewhere = |_: &std::fs::File| Ok(PathBuf::from("/elsewhere/a.png"));
        assert!(open_attached(&file, &real, 1024, elsewhere).is_none());
        assert!(open_attached(&file, &real, 2, |_| Ok(real.clone())).is_none());
        // The real reader agrees with the path the file was opened at.
        if cfg!(target_os = "linux") {
            assert!(open_attached(&file, &real, 1024, opened_at).is_some());
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_read_waits_for_a_slot_within_its_bound() -> TestResult {
        let slots = Arc::new(Semaphore::new(1));
        let taken = read_slot(&slots, ATTACHMENT_READ_WAIT).await?;
        let refused = read_slot(&slots, Duration::from_millis(20)).await;
        assert!(matches!(refused, Err(AppError::Capacity(_))), "{refused:?}");
        let waiting = tokio::spawn({
            let slots = Arc::clone(&slots);
            async move { read_slot(&slots, ATTACHMENT_READ_WAIT).await }
        });
        drop(taken);
        assert!(waiting.await?.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn a_file_is_sent_in_chunks_and_gives_its_slot_back_when_left_unread() -> TestResult {
        let content: Vec<u8> = (0..ATTACHMENT_CHUNK * 2 + 10)
            .map(|i| u8::try_from(i % 251).unwrap_or(0))
            .collect();
        let (_dir, file, real) = temp_file(&content)?;
        let open = || -> Result<(tokio::fs::File, u64), Box<dyn std::error::Error>> {
            let (opened, len) =
                open_attached(&file, &real, u64::MAX, |_| Ok(real.clone())).ok_or("not opened")?;
            Ok((tokio::fs::File::from_std(opened), len))
        };
        let slots = Arc::new(Semaphore::new(1));

        let slot = Arc::clone(&slots).try_acquire_owned()?;
        let mut sent = AttachedFile::new(("a.png".into(), None), open()?, slot, ATTACHMENT_IDLE);
        let mut received = Vec::new();
        while let Some(chunk) = sent.next_chunk().await? {
            assert!(chunk.len() <= ATTACHMENT_CHUNK, "{}", chunk.len());
            received.extend(chunk);
        }
        assert!(received == content, "the bytes differ");
        assert_eq!(slots.available_permits(), 0, "held while the file is open");
        drop(sent);
        assert_eq!(slots.available_permits(), 1, "given back with the file");

        // Left unread: the slot frees by itself and the file fails.
        let slot = Arc::clone(&slots).try_acquire_owned()?;
        let idle = Duration::from_millis(20);
        let mut unread = AttachedFile::new(("a.png".into(), None), open()?, slot, idle);
        let freed = tokio::time::timeout(Duration::from_secs(10), slots.acquire()).await??;
        drop(freed);
        let failed = unread.next_chunk().await;
        assert!(
            failed.is_err_and(|e| e.kind() == std::io::ErrorKind::TimedOut),
            "the file goes on after its slot was given back"
        );
        Ok(())
    }
}
