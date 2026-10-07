//! Snapshot of uncommitted changes as a deterministic tar.gz (SPEC-001 §7.5, IMP-001).
//!
//! Layout is defined in `yulius_contracts::import`: files and symbolic links under `files/`,
//! deleted paths in a NUL-terminated `DELETED` entry.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use flate2::{Compression, GzBuilder};
use tar::{EntryType, Header};
use yulius_contracts::import::{FileRef, SNAPSHOT_DELETED_ENTRY, SNAPSHOT_FILES_PREFIX};

use crate::hash::file_ref;
use crate::process::{
    GitError, LIST_OUTPUT, QUERY_TIMEOUT, SMALL_OUTPUT, bytes_to_path, git, trim_newline,
};

/// Files larger than this are left out of the snapshot (and reported).
pub const MAX_SNAPSHOT_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Upload name of the snapshot archive.
pub(crate) const SNAPSHOT_NAME: &str = "snapshot.tar.gz";

/// Why a changed path is not in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Larger than [`MAX_SNAPSHOT_FILE_BYTES`].
    TooLarge {
        /// Size in bytes.
        size: u64,
    },
    /// The path resolves outside the repository (for example through a symlinked directory).
    OutsideRepository,
    /// A directory (submodule or nested repository) or special file.
    NotRegularFile,
}

/// A changed path left out of the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPath {
    /// Repository-relative path.
    pub path: PathBuf,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Result of capturing uncommitted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The archive written to `out`; `None` when nothing is archived (no changes, or only skipped
    /// paths), in which case `out` is not left behind.
    pub file: Option<FileRef>,
    /// Regular files archived.
    pub files: usize,
    /// Symbolic links archived as links.
    pub links: usize,
    /// Paths deleted relative to `HEAD`.
    pub deleted: usize,
    /// Uncompressed bytes of archived file content.
    pub content_bytes: u64,
    /// Changed paths that were left out.
    pub skipped: Vec<SkippedPath>,
}

/// Snapshot failure.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Listing changes failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// Reading a changed file or writing the archive failed.
    #[error("snapshot I/O error at {}: {source}", path.display())]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The changed files exceed the caller's bound.
    #[error(
        "uncommitted files total more than {limit} bytes; commit, ignore or remove large files and retry"
    )]
    TooLarge {
        /// The enforced bound.
        limit: u64,
    },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> SnapshotError + '_ {
    move |source| SnapshotError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Archives the checkout's differences from `HEAD` into a tar.gz at `out`: tracked files
/// changed in the worktree or index (`git diff --name-only HEAD`), and untracked files not
/// ignored (`git ls-files -o --exclude-standard`). `repo` may be any directory of the checkout;
/// paths are relative to its top level.
///
/// Files over [`MAX_SNAPSHOT_FILE_BYTES`], directories and anything resolving outside `repo` are
/// skipped and reported; symbolic links are stored as links and never followed. Entries are
/// sorted with zeroed timestamps and owners, so identical state yields an identical digest.
///
/// # Errors
/// [`SnapshotError::TooLarge`] when archived content would exceed `max_bytes`, or a Git or I/O
/// failure.
pub fn snapshot_uncommitted(
    repo: &Path,
    out: &Path,
    max_bytes: u64,
) -> Result<Snapshot, SnapshotError> {
    let toplevel = git(
        repo,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        SMALL_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    let toplevel = bytes_to_path(trim_newline(&toplevel.stdout));
    let root = fs::canonicalize(&toplevel).map_err(io_error(&toplevel))?;
    let paths = changed_paths(&root)?;
    let mut snapshot = Snapshot {
        file: None,
        files: 0,
        links: 0,
        deleted: 0,
        content_bytes: 0,
        skipped: Vec::new(),
    };
    let mut deleted = Vec::new();
    let mut entries = Vec::new();
    for relative in paths {
        match classify(&root, &relative)? {
            Change::Deleted => deleted.push(relative),
            Change::Skipped(reason) => snapshot.skipped.push(SkippedPath {
                path: relative,
                reason,
            }),
            Change::File(content, mode) => {
                snapshot.content_bytes += content.len() as u64;
                if snapshot.content_bytes > max_bytes {
                    return Err(SnapshotError::TooLarge { limit: max_bytes });
                }
                entries.push((relative, Entry::File(content, mode)));
            }
            Change::Link(target) => entries.push((relative, Entry::Link(target))),
        }
    }
    snapshot.deleted = deleted.len();
    if entries.is_empty() && deleted.is_empty() {
        return Ok(snapshot);
    }
    write_archive(out, &entries, &deleted)?;
    for (_, entry) in &entries {
        match entry {
            Entry::File(..) => snapshot.files += 1,
            Entry::Link(_) => snapshot.links += 1,
        }
    }
    snapshot.file = Some(file_ref(out, SNAPSHOT_NAME).map_err(io_error(out))?);
    Ok(snapshot)
}

/// Sorted, de-duplicated repository-relative paths of tracked changes and untracked files.
fn changed_paths(root: &Path) -> Result<BTreeSet<PathBuf>, GitError> {
    let tracked = git(
        root,
        &[
            OsStr::new("diff"),
            OsStr::new("--name-only"),
            OsStr::new("--no-renames"),
            OsStr::new("-z"),
            OsStr::new("HEAD"),
            OsStr::new("--"),
        ],
        LIST_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    let untracked = git(
        root,
        &[
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
        ],
        LIST_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    Ok(tracked
        .stdout
        .split(|b| *b == 0)
        .chain(untracked.stdout.split(|b| *b == 0))
        .filter(|raw| !raw.is_empty())
        .map(|raw| bytes_to_path(raw.strip_suffix(b"/").unwrap_or(raw)))
        .collect())
}

enum Change {
    Deleted,
    Skipped(SkipReason),
    File(Vec<u8>, u32),
    Link(PathBuf),
}

enum Entry {
    File(Vec<u8>, u32),
    Link(PathBuf),
}

fn classify(root: &Path, relative: &Path) -> Result<Change, SnapshotError> {
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Ok(Change::Skipped(SkipReason::OutsideRepository));
    }
    let absolute = root.join(relative);
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Change::Deleted),
        Err(error) => {
            return Err(SnapshotError::Io {
                path: relative.to_path_buf(),
                source: error,
            });
        }
    };
    // The parent directory must resolve inside the repository even for links, so a path
    // reached through a symlinked directory is never read or recorded.
    let parent_inside = absolute
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .is_some_and(|parent| parent.starts_with(root));
    if !parent_inside {
        return Ok(Change::Skipped(SkipReason::OutsideRepository));
    }
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(&absolute).map_err(io_error(relative))?;
        return Ok(Change::Link(target));
    }
    if !kind.is_file() {
        return Ok(Change::Skipped(SkipReason::NotRegularFile));
    }
    if metadata.len() > MAX_SNAPSHOT_FILE_BYTES {
        return Ok(Change::Skipped(SkipReason::TooLarge {
            size: metadata.len(),
        }));
    }
    let content = fs::read(&absolute).map_err(io_error(relative))?;
    if content.len() as u64 > MAX_SNAPSHOT_FILE_BYTES {
        return Ok(Change::Skipped(SkipReason::TooLarge {
            size: content.len() as u64,
        }));
    }
    Ok(Change::File(content, file_mode(&metadata)))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    }
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0o644
}

fn base_header(kind: EntryType, size: u64, mode: u32) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header
}

fn write_archive(
    out: &Path,
    entries: &[(PathBuf, Entry)],
    deleted: &[PathBuf],
) -> Result<(), SnapshotError> {
    let file = File::create(out).map_err(io_error(out))?;
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(BufWriter::new(file), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let prefix = Path::new(SNAPSHOT_FILES_PREFIX.trim_end_matches('/'));
    for (relative, entry) in entries {
        let name = prefix.join(relative);
        let result = match entry {
            Entry::File(content, mode) => {
                let mut header = base_header(EntryType::Regular, content.len() as u64, *mode);
                archive.append_data(&mut header, &name, content.as_slice())
            }
            Entry::Link(target) => {
                let mut header = base_header(EntryType::Symlink, 0, 0o777);
                archive.append_link(&mut header, &name, target)
            }
        };
        result.map_err(io_error(relative))?;
    }
    let mut list = Vec::new();
    for path in deleted {
        list.extend_from_slice(path.as_os_str().as_encoded_bytes());
        list.push(0);
    }
    let mut header = base_header(EntryType::Regular, list.len() as u64, 0o644);
    archive
        .append_data(&mut header, SNAPSHOT_DELETED_ENTRY, list.as_slice())
        .map_err(io_error(out))?;
    let encoder = archive.into_inner().map_err(io_error(out))?;
    let mut writer = encoder.finish().map_err(io_error(out))?;
    writer.flush().map_err(io_error(out))?;
    let file = writer.into_inner().map_err(|error| SnapshotError::Io {
        path: out.to_path_buf(),
        source: error.into_error(),
    })?;
    file.sync_all().map_err(io_error(out))
}
