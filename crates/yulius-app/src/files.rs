//! Sandbox files for the built-in editor (SPEC-001 EDT-001..004).
//!
//! Contract:
//! - only users with write access to the sandbox list, read or change files;
//! - the root is the sandbox repository or a task worktree of that sandbox; paths are
//!   relative, made of normal components only, and every existing path (symbolic links
//!   resolved) must stay inside the root;
//! - `.git` directories are not listed and cannot be read or changed through this API;
//! - reads return UTF-8 text up to [`MAX_FILE_BYTES`] (binary or larger files are reported, not
//!   returned) with the SHA-256 of the bytes on disk;
//! - writes are atomic (temporary file + rename, permissions kept) and refused with `Conflict`
//!   when the file changed since the editor read it (agents edit the same files) or, for a new
//!   file, when it already exists;
//! - each user's editor view (open files, unsaved text) lives only in their browser; nothing
//!   of it is stored, and only where their selection is in a file is relayed to the sandbox's
//!   other writers (SPEC-127).

use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use yulius_contracts::api::files::{
    CreateEntry, DirListing, FileContent, FileEntry, FilePresence, FileRoot, FileRoots,
    FileWritten, WriteFile,
};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::notes::{check_presence, presence_name};

/// Largest file the editor opens or writes.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Entries returned per directory listing.
pub const MAX_ENTRIES: usize = 2000;
/// Longest path a presence update may name (SPEC-127).
pub const MAX_PRESENCE_PATH: usize = 4096;
/// Bytes inspected for NUL when telling text from binary.
const SNIFF_BYTES: usize = 8192;
/// Most folders deep a tree removal goes (SPEC-265 EDH-005).
const MAX_REMOVE_DEPTH: usize = 256;
/// Most entries one tree removal removes.
const MAX_REMOVED_ENTRIES: usize = 1_000_000;
/// Names read from a folder at a time while a tree removal empties it.
const REMOVE_BATCH: usize = 512;

/// A resolved file request: the root folder and the path below it.
pub(crate) struct Target {
    /// The sandbox directory (canonical): the folder handles are opened from, which the agent
    /// cannot replace (ADR-0174).
    pub(crate) anchor: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) relative: String,
}

impl Target {
    /// Opens the resolved `folder` (inside the root, out of `.git`) from the sandbox directory
    /// one folder at a time without following links: a folder on the way swapped for a link
    /// after `folder` was resolved is refused, and no `/proc` is needed to tell (#660).
    fn open_folder(&self, folder: &Path) -> AppResult<OwnedFd> {
        let outside = || AppError::Forbidden("the path leads outside the folder".into());
        if !inside(&self.root, folder) {
            return Err(outside());
        }
        // A file on the way is a mistake, not an escape: said as such (the walk refuses it too).
        if !folder.is_dir() {
            return Err(AppError::Invalid("that path is not a directory".into()));
        }
        let below = folder.strip_prefix(&self.anchor).map_err(|_| outside())?;
        let names: Vec<&OsStr> = below.components().map(Component::as_os_str).collect();
        open_dirs_below(&self.anchor, &names, false).map_err(|err| match err {
            Errno::ELOOP | Errno::ENOTDIR => outside(),
            other => errno_error(other),
        })
    }
}

/// Normalizes a relative path: normal components only, no `.git`.
pub(crate) fn relative_components(path: &str) -> AppResult<Vec<String>> {
    if path.contains('\0') || path.starts_with('/') {
        return Err(AppError::Invalid("paths are relative to the folder".into()));
    }
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy().into_owned();
                if part == ".git" {
                    return Err(AppError::Forbidden(
                        "the .git directory is not editable".into(),
                    ));
                }
                parts.push(part);
            }
            Component::CurDir => {}
            _ => return Err(AppError::Invalid("invalid path".into())),
        }
    }
    Ok(parts)
}

/// An OS error of a `*at` call, as [`io_error`] reports it.
pub(crate) fn errno_error(err: Errno) -> AppError {
    io_error(&std::io::Error::from(err))
}

pub(crate) fn io_error(err: &std::io::Error) -> AppError {
    match err.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound,
        std::io::ErrorKind::PermissionDenied => AppError::Forbidden("permission denied".into()),
        _ => AppError::internal(err.to_string()),
    }
}

/// Whether `path` (resolved) stays inside `root` (resolved) and out of every `.git` directory,
/// also when a symbolic link inside the folder leads into one (#59).
pub(crate) fn inside(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|rest| !rest.components().any(|part| part.as_os_str() == ".git"))
}

/// Where an open file or folder really is (`/proc/self/fd` on Linux, `F_GETPATH` on macOS).
pub(crate) fn real_path(fd: &impl std::os::fd::AsFd) -> AppResult<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()))
            .map_err(|e| io_error(&e))
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let mut path = PathBuf::new();
        nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut path)).map_err(errno_error)?;
        Ok(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        let _ = fd;
        Err(AppError::Forbidden(
            "reaching sandbox files needs Linux or macOS".into(),
        ))
    }
}

/// Opens the resolved `path` with `flags`, without following a link at its end and without
/// blocking, and checks that what was opened is still inside `root` and out of `.git`. A folder
/// on the way swapped for a link after `path` was resolved is refused: the handle, not the path,
/// is what gets read or written (#657, ADR-0174).
pub(crate) fn open_inside(
    root: &Path,
    path: &Path,
    flags: OFlag,
) -> AppResult<std::os::fd::OwnedFd> {
    let outside = || AppError::Forbidden("the path leads outside the folder".into());
    let flags = flags | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let fd = match nix::fcntl::open(path, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ELOOP | Errno::ENOTDIR) => return Err(outside()),
        Err(err) => return Err(errno_error(err)),
    };
    if !inside(root, &real_path(&fd)?) {
        return Err(outside());
    }
    Ok(fd)
}

/// Opens `parts` below `anchor` one folder at a time with `O_DIRECTORY | O_NOFOLLOW`, creating
/// the missing ones when `create`: a link or a file the agent put on the way fails the walk
/// (`ELOOP`, `ENOTDIR`) instead of steering what follows (#657, ADR-0174). `anchor` itself is
/// opened by path: the caller passes one the agent cannot replace (its sandbox directory).
pub(crate) fn open_dirs_below(
    anchor: &Path,
    parts: &[&std::ffi::OsStr],
    create: bool,
) -> Result<std::os::fd::OwnedFd, Errno> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    let mut dir = nix::fcntl::open(anchor, flags, Mode::empty())?;
    for part in parts {
        dir = open_dir_at(&dir, part, create)?;
    }
    Ok(dir)
}

/// Opens the folder `name` in the folder `dir` with `O_DIRECTORY | O_NOFOLLOW`, creating it
/// first when it is missing and `create` (see [`open_dirs_below`]).
pub(crate) fn open_dir_at(
    dir: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    create: bool,
) -> Result<std::os::fd::OwnedFd, Errno> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    match openat(dir, name, flags, Mode::empty()) {
        Err(Errno::ENOENT) if create => {
            match nix::sys::stat::mkdirat(dir, name, Mode::from_bits_truncate(0o777)) {
                Ok(()) | Err(Errno::EEXIST) => {}
                Err(err) => return Err(err),
            }
            openat(dir, name, flags, Mode::empty())
        }
        other => other,
    }
}

/// A listing of the folder `dir` through a fresh description of it, so it starts at the folder's
/// beginning.
fn listing(dir: &OwnedFd) -> Result<nix::dir::OwningIter, Errno> {
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    Ok(nix::dir::Dir::openat(dir, ".", flags, Mode::empty())?.into_iter())
}

/// Up to `limit` names in the folder `dir` (`.` and `..` left out), from its beginning.
pub(crate) fn names_in(dir: &OwnedFd, limit: usize) -> Result<Vec<OsString>, Errno> {
    next_names(&mut listing(dir)?, limit)
}

/// The next `limit` names of `listing` at most (`.` and `..` left out); it reads on from there.
fn next_names(listing: &mut nix::dir::OwningIter, limit: usize) -> Result<Vec<OsString>, Errno> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut names = Vec::new();
    while names.len() < limit {
        let Some(entry) = listing.next() else {
            break;
        };
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(OsStr::from_bytes(name).to_owned());
        }
    }
    Ok(names)
}

/// The type of `name` in the folder `dir`, a link not followed.
pub(crate) fn kind_at(
    dir: &OwnedFd,
    name: &OsStr,
) -> Result<(SFlag, nix::sys::stat::FileStat), Errno> {
    let stat = fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    Ok((
        SFlag::from_bits_truncate(stat.st_mode).intersection(SFlag::S_IFMT),
        stat,
    ))
}

/// Removes `name` from the folder `dir`: a folder with everything in it, handle by handle (each
/// sub-folder opened with `O_NOFOLLOW`; only folders are opened), a file or a link with `unlinkat`;
/// a link is removed, never followed (#660, ADR-0174). A missing `name` is not an error. Bounded by [`MAX_REMOVE_DEPTH`]
/// and [`MAX_REMOVED_ENTRIES`]: past them it stops and leaves the rest.
pub(crate) fn remove_tree(dir: &OwnedFd, name: &OsStr) -> AppResult<()> {
    let mut budget = MAX_REMOVED_ENTRIES;
    remove_entry(dir, name, 0, &mut budget)
}

fn remove_entry(dir: &OwnedFd, name: &OsStr, depth: usize, budget: &mut usize) -> AppResult<()> {
    *budget = budget.checked_sub(1).ok_or_else(|| {
        AppError::Conflict(format!(
            "more than {MAX_REMOVED_ENTRIES} entries to delete; the rest is left"
        ))
    })?;
    let removed = match kind_at(dir, name).map(|(kind, _)| kind) {
        Ok(SFlag::S_IFDIR) => match open_dir_at(dir, name, false) {
            Ok(folder) => {
                if depth >= MAX_REMOVE_DEPTH {
                    return Err(AppError::Conflict(format!(
                        "folders nest more than {MAX_REMOVE_DEPTH} deep; the rest is left"
                    )));
                }
                empty_folder(&folder, depth, budget)?;
                unlinkat(dir, name, UnlinkatFlags::RemoveDir)
            }
            // Swapped for a link or a file since: that is removed, not followed.
            Err(Errno::ELOOP | Errno::ENOTDIR) => unlinkat(dir, name, UnlinkatFlags::NoRemoveDir),
            Err(err) => Err(err),
        },
        Ok(_) => unlinkat(dir, name, UnlinkatFlags::NoRemoveDir),
        Err(err) => Err(err),
    };
    match removed {
        Ok(()) | Err(Errno::ENOENT) => Ok(()),
        Err(err) => Err(errno_error(err)),
    }
}

/// Removes everything in `folder`: one listing is read on while its entries go (linear in the
/// entries), then the folder is read again from its beginning until it is empty, so entries made
/// meanwhile go too.
fn empty_folder(folder: &OwnedFd, depth: usize, budget: &mut usize) -> AppResult<()> {
    loop {
        let mut stream = listing(folder).map_err(errno_error)?;
        let mut any = false;
        loop {
            let names = next_names(&mut stream, REMOVE_BATCH).map_err(errno_error)?;
            if names.is_empty() {
                break;
            }
            any = true;
            for child in &names {
                remove_entry(folder, child, depth + 1, budget)?;
            }
        }
        if !any {
            return Ok(());
        }
    }
}

/// Writes `name` in the folder `dir` whole: `write` fills a new file created through the handle
/// as `temporary` (`O_EXCL | O_NOFOLLOW`, so an entry planted there is never opened), which is
/// synced and renamed over `name` within the same handle; it is removed when anything fails.
pub(crate) fn replace_in(
    dir: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    temporary: &str,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> AppResult<()> {
    let fd = openat(
        dir,
        temporary,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o666),
    )
    .map_err(errno_error)?;
    let mut file = std::fs::File::from(fd);
    let written = write(&mut file)
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error(&e))
        .and_then(|()| renameat(dir, temporary, dir, name).map_err(errno_error));
    if written.is_err() {
        let _ = unlinkat(dir, temporary, UnlinkatFlags::NoRemoveDir);
    }
    written
}

/// Most bytes of `.git/HEAD` read: `ref: refs/heads/<branch>` fits well within it.
const MAX_HEAD_BYTES: u64 = 1024;

/// The sandbox repository's branch, as its `HEAD` names it: `None` when `HEAD` is detached or
/// unreadable, and `Err` when there is no repository folder. Walked from `anchor` (the sandbox
/// directory) folder by folder with `O_NOFOLLOW` and `HEAD` opened without following a link or
/// waiting on a pipe, so a contained shell (root in its sandbox) that swaps `repo` or `.git` for
/// a link, at any moment, steers nothing outside the sandbox (ADR-0174).
fn checked_out_branch(anchor: &Path) -> Result<Option<String>, Errno> {
    use std::io::Read as _;
    let repo = open_dirs_below(anchor, &[OsStr::new("repo")], false)?;
    crate::race::reached(crate::race::Point::RootsHead, &anchor.join("repo"));
    let Ok(git) = open_dir_at(&repo, OsStr::new(".git"), false) else {
        return Ok(None);
    };
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let Ok(head) = openat(&git, "HEAD", flags, Mode::empty()) else {
        return Ok(None);
    };
    let file = std::fs::File::from(head);
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return Ok(None);
    }
    let mut text = String::new();
    if file.take(MAX_HEAD_BYTES).read_to_string(&mut text).is_err() {
        return Ok(None);
    }
    Ok(text
        .trim()
        .strip_prefix("ref: refs/heads/")
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned))
}

/// A SHA-256 as the files API writes it: 64 lowercase hex digits.
fn is_sha256_hex(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl App {
    /// The worktree of `task` when it belongs to `sandbox` and lies inside the sandbox
    /// directory (shared by the editor and sandbox shells). With a `writer`, they must own the
    /// task (SPEC-070).
    ///
    /// # Errors
    /// `NotFound` for unknown tasks or tasks of other sandboxes, `Forbidden` for a writer who
    /// does not own the task, `Conflict` when the task has no worktree on disk.
    pub(crate) async fn sandbox_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &str,
        writer: Option<&AuthContext>,
    ) -> AppResult<PathBuf> {
        let task_id = TaskId::parse(task).map_err(|_| AppError::NotFound)?;
        let task = self
            .deps
            .store
            .task(&task_id)
            .await?
            .filter(|t| t.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        // Changing a task's worktree is its owner's: its agent commits and pushes what is there
        // as them (SPEC-070).
        if let Some(ctx) = writer {
            self.require_owner(ctx, &task).await?;
        }
        let no_worktree = || AppError::Conflict("the task has no worktree yet".into());
        let path = task.worktree_path.ok_or_else(no_worktree)?;
        let worktree = std::fs::canonicalize(&path).map_err(|_| no_worktree())?;
        let root =
            std::fs::canonicalize(self.sandbox_dir(&sandbox.id)).map_err(|_| no_worktree())?;
        if !worktree.starts_with(&root) || !worktree.is_dir() {
            return Err(AppError::Conflict(
                "the task worktree is outside the sandbox".into(),
            ));
        }
        Ok(worktree)
    }

    /// The folders a write user of `sandbox` can open in the editor (SPEC-340 EWT-001): the
    /// sandbox repository with the branch its `HEAD` names, then every task worktree that is on
    /// disk inside the sandbox directory, newest task first, writable for the task's owner only.
    ///
    /// # Errors
    /// Access errors (`Work` on the sandbox), store failures.
    pub async fn files_roots(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<FileRoots> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let mut tasks = self.deps.store.tasks(&record.id, false).await?;
        tasks.extend(self.deps.store.tasks(&record.id, true).await?);
        tasks.sort_by(|a, b| b.number.cmp(&a.number));
        let sandbox_dir = self.sandbox_dir(&record.id);
        let user = ctx.user.id.clone();
        // Disk only from here: off the async workers, as the reads of `file_read`.
        let roots = tokio::task::spawn_blocking(move || {
            let mut roots = Vec::new();
            let Ok(anchor) = std::fs::canonicalize(sandbox_dir) else {
                return roots;
            };
            if let Ok(branch) = checked_out_branch(&anchor) {
                roots.push(FileRoot {
                    root: "sandbox".into(),
                    branch,
                    task_number: None,
                    task_title: None,
                    writable: true,
                });
            }
            for task in tasks {
                let Some(path) = task.worktree_path.as_deref() else {
                    continue;
                };
                // As `sandbox_worktree`: only a directory inside the sandbox is the task's folder.
                match std::fs::canonicalize(path) {
                    Ok(worktree) if worktree.starts_with(&anchor) && worktree.is_dir() => {}
                    _ => continue,
                }
                roots.push(FileRoot {
                    root: format!("task:{}", task.id),
                    writable: task.owner == user,
                    branch: task.branch,
                    task_number: Some(task.number),
                    task_title: Some(task.title),
                });
            }
            roots
        })
        .await
        .map_err(AppError::internal)?;
        Ok(FileRoots { roots })
    }

    /// Resolves `root` and `path` for a write user of `sandbox`; to `write` into a task's
    /// worktree they must own the task (SPEC-070).
    pub(crate) async fn file_target(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
        write: bool,
    ) -> AppResult<Target> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let folder = match root.unwrap_or("sandbox") {
            "sandbox" | "" => {
                let repo = self.sandbox_repo(&record.id);
                if !repo.is_dir() {
                    return Err(AppError::Conflict(
                        "the sandbox repository is not cloned yet".into(),
                    ));
                }
                repo
            }
            other => {
                let task = other
                    .strip_prefix("task:")
                    .ok_or_else(|| AppError::Invalid("root is sandbox or task:<id>".into()))?;
                self.sandbox_worktree(&record, task, write.then_some(ctx))
                    .await?
            }
        };
        let root = std::fs::canonicalize(&folder).map_err(|e| io_error(&e))?;
        // A repository folder replaced by a link (a contained shell is root in its sandbox) must
        // not lead the editor or an upload into another sandbox (EDT-006).
        let sandbox_dir =
            std::fs::canonicalize(self.sandbox_dir(&record.id)).map_err(|e| io_error(&e))?;
        if !root.starts_with(&sandbox_dir) {
            return Err(AppError::Forbidden(
                "the folder leads outside the sandbox".into(),
            ));
        }
        let parts = relative_components(path)?;
        let relative = parts.join("/");
        let path = parts.iter().fold(root.clone(), |p, part| p.join(part));
        Ok(Target {
            anchor: sandbox_dir,
            root,
            path,
            relative,
        })
    }

    /// Resolves an existing target and checks it stays inside the root.
    fn existing(target: &Target) -> AppResult<PathBuf> {
        let resolved = std::fs::canonicalize(&target.path).map_err(|e| io_error(&e))?;
        if !inside(&target.root, &resolved) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        Ok(resolved)
    }

    /// Lists a directory.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Invalid` paths, `Forbidden` outside the folder.
    pub async fn files_list(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<DirListing> {
        let target = self.file_target(ctx, sandbox, root, path, false).await?;
        let dir = Self::existing(&target)?;
        crate::race::reached(crate::race::Point::FilesList, &dir);
        // Listed through the folder's handle, never by path again (#660).
        let folder = target.open_folder(&dir)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        // One more than shown, and one for `.git`, tells whether the listing is cut.
        for raw in names_in(&folder, MAX_ENTRIES + 2).map_err(errno_error)? {
            let name = raw.to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let Ok((kind, stat)) = kind_at(&folder, &raw) else {
                continue;
            };
            let kind = if kind == SFlag::S_IFLNK {
                // Links into the folder behave like their target; others are shown as links.
                match std::fs::canonicalize(dir.join(&raw)) {
                    Ok(resolved) if inside(&target.root, &resolved) && resolved.is_dir() => "dir",
                    Ok(resolved) if inside(&target.root, &resolved) => "file",
                    _ => "symlink",
                }
            } else if kind == SFlag::S_IFDIR {
                "dir"
            } else {
                "file"
            };
            let relative = if target.relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", target.relative)
            };
            entries.push(FileEntry {
                name,
                path: relative,
                kind: kind.into(),
                size: if kind == "file" {
                    u64::try_from(stat.st_size).unwrap_or(0)
                } else {
                    0
                },
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != "dir")
                .cmp(&(b.kind != "dir"))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(DirListing {
            path: target.relative,
            entries,
            truncated,
        })
    }

    /// Reads a file for the editor. With `known`, the `sha256` the caller already has, an
    /// unchanged file is `None`: the editor looks at the file in front every 2 s (SPEC-181).
    ///
    /// # Errors
    /// As [`App::files_list`]; `Invalid` for directories and a malformed `known`.
    pub async fn file_read(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
        known: Option<&str>,
    ) -> AppResult<Option<FileContent>> {
        let target = self.file_target(ctx, sandbox, root, path, false).await?;
        if known.is_some_and(|sha| !is_sha256_hex(sha)) {
            return Err(AppError::Invalid("known is 64 lowercase hex digits".into()));
        }
        let file = Self::existing(&target)?;
        crate::race::reached(crate::race::Point::FileRead, &file);
        let file = std::fs::File::from(open_inside(&target.root, &file, OFlag::O_RDONLY)?);
        let meta = file.metadata().map_err(|e| io_error(&e))?;
        if meta.is_dir() {
            return Err(AppError::Invalid("that path is a directory".into()));
        }
        if !meta.is_file() {
            return Err(AppError::Invalid("that path is not a regular file".into()));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Ok(Some(FileContent {
                path: target.relative,
                content: None,
                binary: false,
                too_large: true,
                size: meta.len(),
                sha256: String::new(),
            }));
        }
        // Read every 2 s while someone has the file in front: off the runtime's threads.
        let bytes = tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_FILE_BYTES), &mut bytes)
                .map(|_| bytes)
        })
        .await
        .map_err(AppError::internal)?
        .map_err(|e| io_error(&e))?;
        let sha256 = self.deps.crypto.sha256_hex(&bytes);
        if known == Some(sha256.as_str()) {
            return Ok(None);
        }
        let sniff = bytes
            .get(..SNIFF_BYTES.min(bytes.len()))
            .unwrap_or_default();
        let text = if sniff.contains(&0) {
            None
        } else {
            String::from_utf8(bytes).ok()
        };
        Ok(Some(FileContent {
            path: target.relative,
            binary: text.is_none(),
            content: text,
            too_large: false,
            size: meta.len(),
            sha256,
        }))
    }

    /// Writes a file atomically if it did not change since `base_sha256` (or creates it when
    /// the base is absent).
    ///
    /// # Errors
    /// As [`App::files_list`]; `Conflict` when the file changed or already exists; `Invalid`
    /// for oversized content.
    pub async fn file_write(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &WriteFile,
    ) -> AppResult<FileWritten> {
        if req.path.trim().is_empty() {
            return Err(AppError::Invalid("a file path is required".into()));
        }
        if u64::try_from(req.content.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            return Err(AppError::Invalid(format!(
                "files are limited to {MAX_FILE_BYTES} bytes in the editor"
            )));
        }
        let target = self
            .file_target(ctx, sandbox, req.root.as_deref(), &req.path, true)
            .await?;
        let parent = target
            .path
            .parent()
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        let parent = std::fs::canonicalize(parent).map_err(|e| io_error(&e))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        let file = parent.join(target.path.file_name().unwrap_or_default());
        let existing = match std::fs::symlink_metadata(&file) {
            Ok(_) => Some(Self::existing(&target)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(io_error(&err)),
        };
        let destination = existing.unwrap_or(file);
        crate::race::reached(crate::race::Point::FileWrite, &destination);
        // From here on everything goes through the destination folder's handle, checked to be
        // inside the root: a folder on the way swapped for a link is never written into (#657).
        let (Some(folder), Some(name)) = (destination.parent(), destination.file_name()) else {
            return Err(AppError::Invalid("invalid path".into()));
        };
        let dir = open_inside(&target.root, folder, OFlag::O_RDONLY | OFlag::O_DIRECTORY)?;
        let current = match openat(
            &dir,
            name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let handle = std::fs::File::from(fd);
                let meta = handle.metadata().map_err(|e| io_error(&e))?;
                if !meta.is_file() {
                    return Err(AppError::Conflict("that path is not a regular file".into()));
                }
                Some((handle, meta.permissions()))
            }
            Err(Errno::ENOENT) => None,
            Err(Errno::ELOOP) => {
                return Err(AppError::Forbidden(
                    "the path leads outside the folder".into(),
                ));
            }
            Err(err) => return Err(errno_error(err)),
        };
        let mode = match (current, &req.base_sha256) {
            (Some((handle, permissions)), Some(base)) => {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(
                    &mut std::io::Read::take(handle, MAX_FILE_BYTES + 1),
                    &mut bytes,
                )
                .map_err(|e| io_error(&e))?;
                if &self.deps.crypto.sha256_hex(&bytes) != base {
                    return Err(AppError::Conflict(
                        "the file changed on disk since you opened it; reload it first".into(),
                    ));
                }
                Some(permissions)
            }
            (Some(_), None) => {
                return Err(AppError::Conflict("a file with that name exists".into()));
            }
            (None, Some(_)) => {
                return Err(AppError::Conflict(
                    "the file was deleted on disk since you opened it".into(),
                ));
            }
            (None, None) => None,
        };
        // A new file at an unpredictable name: an entry an agent planted there (a link out of
        // the folder) is never opened, so the save cannot write through it (#655).
        let suffix: String = self.deps.crypto.random_bytes()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let temporary = format!(".{}.yulius-edit-{suffix}", name.to_string_lossy());
        replace_in(&dir, name, &temporary, |handle| {
            // On the handle, never by path: the agent can see the new entry and swap it for a
            // link before a path-based chmod (review round 2).
            if let Some(permissions) = mode {
                handle.set_permissions(permissions)?;
            }
            std::io::Write::write_all(handle, req.content.as_bytes())
        })?;
        Ok(FileWritten {
            path: target.relative,
            sha256: self.deps.crypto.sha256_hex(req.content.as_bytes()),
            size: u64::try_from(req.content.len()).unwrap_or(u64::MAX),
        })
    }

    /// Creates an empty file or a directory.
    ///
    /// # Errors
    /// As [`App::files_list`]; `Conflict` when the path exists.
    pub async fn file_create(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &CreateEntry,
    ) -> AppResult<FileEntry> {
        if req.kind == "file" {
            let written = self
                .file_write(
                    ctx,
                    sandbox,
                    &WriteFile {
                        root: req.root.clone(),
                        path: req.path.clone(),
                        content: String::new(),
                        base_sha256: None,
                    },
                )
                .await?;
            let name = written
                .path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            return Ok(FileEntry {
                name,
                path: written.path,
                kind: "file".into(),
                size: 0,
            });
        }
        if req.kind != "dir" {
            return Err(AppError::Invalid("kind is file or dir".into()));
        }
        let target = self
            .file_target(ctx, sandbox, req.root.as_deref(), &req.path, true)
            .await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid("a directory name is required".into()));
        }
        if std::fs::symlink_metadata(&target.path).is_ok() {
            return Err(AppError::Conflict("that path exists".into()));
        }
        let parent = target
            .path
            .parent()
            .map(std::fs::canonicalize)
            .transpose()
            .map_err(|e| io_error(&e))?
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        crate::race::reached(crate::race::Point::FileCreate, &parent);
        // Created through the parent's handle: a parent swapped for a link is refused (#660).
        let folder = target.open_folder(&parent)?;
        let name = target
            .path
            .file_name()
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        match mkdirat(&folder, name, Mode::from_bits_truncate(0o777)) {
            Ok(()) => {}
            Err(Errno::EEXIST) => return Err(AppError::Conflict("that path exists".into())),
            Err(err) => return Err(errno_error(err)),
        }
        let name = target
            .relative
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(FileEntry {
            name,
            path: target.relative,
            kind: "dir".into(),
            size: 0,
        })
    }

    /// Deletes a file, a symbolic link or a directory tree (never the folder itself).
    ///
    /// # Errors
    /// As [`App::files_list`].
    pub async fn file_delete(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<()> {
        let target = self.file_target(ctx, sandbox, root, path, true).await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid(
                "the folder itself cannot be deleted".into(),
            ));
        }
        // The entry itself must be inside the folder (its parent resolved), even for links.
        let parent = target
            .path
            .parent()
            .map(std::fs::canonicalize)
            .transpose()
            .map_err(|e| io_error(&e))?
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        crate::race::reached(crate::race::Point::FileDelete, &parent);
        // Removed through the parent's handle: a parent swapped for a link is refused, and a
        // link inside the tree is removed, never followed (#660).
        let folder = target.open_folder(&parent)?;
        let name = target
            .path
            .file_name()
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?
            .to_owned();
        kind_at(&folder, &name).map_err(errno_error)?;
        // A large tree takes a while: off the runtime's threads.
        crate::projects::blocking(move || remove_tree(&folder, &name)).await
    }
}

impl App {
    /// `PUT /api/sandboxes/{sid}/files/presence`: relays the caller's selection in a file to the
    /// people who may read the sandbox's files (SPEC-127 EDT-008..009), with the notepads' limiter.
    ///
    /// # Errors
    /// Access errors as reading the file, `Invalid` for a bad path, `sha256`, `client` or an
    /// offset past [`MAX_FILE_BYTES`], `Capacity` past the presence limit.
    pub async fn file_presence(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        input: &FilePresence,
    ) -> AppResult<()> {
        let root = match input.root.as_deref() {
            None | Some("" | "sandbox") => "sandbox",
            Some(other) => other,
        };
        if input.path.len() > MAX_PRESENCE_PATH {
            return Err(AppError::Invalid(format!(
                "a path is at most {MAX_PRESENCE_PATH} bytes"
            )));
        }
        let target = self
            .file_target(ctx, sandbox, Some(root), &input.path, false)
            .await?;
        // A folder is not a file; a link is not followed to tell (it may lead outside the folder).
        let folder = std::fs::symlink_metadata(&target.path).is_ok_and(|meta| meta.is_dir());
        if target.relative.is_empty() || folder {
            return Err(AppError::Invalid("presence names a file".into()));
        }
        // Someone is in a file that is there; leaving one may come after it was deleted.
        if input.selection.is_some() && !Self::existing(&target)?.is_file() {
            return Err(AppError::Invalid("presence names a file".into()));
        }
        if !(input.sha256.is_empty() || is_sha256_hex(&input.sha256)) {
            return Err(AppError::Invalid(
                "sha256 is 64 lowercase hex digits".into(),
            ));
        }
        // A UTF-16 offset never exceeds the UTF-8 length of the text.
        let max_offset = u32::try_from(MAX_FILE_BYTES).unwrap_or(u32::MAX);
        check_presence(input.selection, max_offset, &input.client)?;
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.count_presence(&format!(
            "{}|file:{sid}:{root}:{}",
            ctx.user.id, target.relative
        ))?;
        self.events.publish_to_writers(
            sid.clone(),
            UiEvent::FilePresence {
                sandbox_id: sid.to_string(),
                root: root.to_owned(),
                path: target.relative,
                user_id: ctx.user.id.to_string(),
                client: input.client.clone(),
                name: presence_name(ctx),
                sha256: input.sha256.clone(),
                selection: input.selection,
                hello: input.hello,
                seq: input.seq,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_pass() {
        assert_eq!(
            relative_components("src/./main.rs").ok(),
            Some(vec!["src".to_owned(), "main.rs".to_owned()])
        );
        assert_eq!(relative_components("").ok(), Some(Vec::new()));
        assert!(relative_components("../etc/passwd").is_err());
        assert!(relative_components("a/../../b").is_err());
        assert!(relative_components("/etc/passwd").is_err());
        assert!(relative_components("a\0b").is_err());
        assert!(relative_components(".git/config").is_err());
        assert!(relative_components("sub/.git/hooks").is_err());
    }

    #[test]
    fn a_tree_removal_unlinks_links_and_never_follows_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let base = dir.path();
        let outside = base.join("outside");
        std::fs::create_dir_all(outside.join("keep"))?;
        std::fs::write(outside.join("keep/file.txt"), "precious")?;
        let tree = base.join("folder/tree");
        std::fs::create_dir_all(tree.join("a/b"))?;
        std::fs::write(tree.join("a/b/c.txt"), "x")?;
        std::fs::write(tree.join("top.txt"), "x")?;
        std::os::unix::fs::symlink(&outside, tree.join("a/link"))?;
        std::os::unix::fs::symlink(outside.join("keep/file.txt"), tree.join("file-link"))?;
        let folder = open_dirs_below(base, &[OsStr::new("folder")], false)?;
        remove_tree(&folder, OsStr::new("tree"))?;
        assert!(!tree.exists() && base.join("folder").is_dir());
        assert_eq!(
            std::fs::read_to_string(outside.join("keep/file.txt"))?,
            "precious"
        );
        // A missing entry is not an error; a link as the entry itself is removed, not followed.
        remove_tree(&folder, OsStr::new("tree"))?;
        std::os::unix::fs::symlink(&outside, base.join("folder/gone"))?;
        remove_tree(&folder, OsStr::new("gone"))?;
        assert!(std::fs::symlink_metadata(base.join("folder/gone")).is_err());
        assert!(outside.join("keep/file.txt").is_file());
        Ok(())
    }
}
