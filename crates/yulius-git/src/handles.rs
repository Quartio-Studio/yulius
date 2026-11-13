//! Files in a clone's `.git`, reached through handles that follow no link (ADR-0174, ADR-0257,
//! ADR-0314).
//!
//! The clone's `.git` is the agents' to change at any moment, and the server works in it as the
//! service user, outside every containment. A path there is therefore never looked up twice and
//! never followed: `.git` is opened from the repository folder with `O_NOFOLLOW`, every folder
//! below it from its parent's handle, a file is read through an `O_NOFOLLOW | O_NONBLOCK` handle
//! and only when it is a regular one, and a file is replaced by a new file of a random name,
//! created `O_EXCL`, that is renamed over the name in the same handle. A link an agent put at any
//! of these names is refused or replaced; what it points at is neither read nor written.

use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fchmod, fstat, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use crate::error::GitError;

const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

pub(crate) fn io(context: &'static str, err: impl Into<std::io::Error>) -> GitError {
    GitError::Io {
        context,
        source: err.into(),
    }
}

fn refused(what: &'static str, reason: &str) -> GitError {
    GitError::InvalidInput {
        what,
        reason: reason.to_owned(),
    }
}

/// `repo/.git`, each of the two opened without following a link. Yulius makes every clone with a
/// real `.git` directory; a pointer file or a link would send a write to another repository.
pub(crate) fn open_git_dir(repo: &Path) -> Result<OwnedFd, GitError> {
    let repo = match open(repo, DIR_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ELOOP | Errno::ENOTDIR) => {
            return Err(refused("repository", "it is not a directory"));
        }
        Err(err) => return Err(io("opening the repository", err)),
    };
    match openat(&repo, ".git", DIR_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(Errno::ELOOP | Errno::ENOTDIR | Errno::ENOENT) => {
            Err(refused("repository", "its .git is not a directory"))
        }
        Err(err) => Err(io("opening the repository's .git", err)),
    }
}

/// The folder `name` of `parent`, made (0755) when `create` and it is missing; `None` when it is
/// missing and not made. A link or a file at the name is refused.
pub(crate) fn open_dir_in(
    parent: &OwnedFd,
    name: &str,
    create: bool,
) -> Result<Option<OwnedFd>, GitError> {
    for _ in 0..2 {
        match openat(parent, name, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => return Ok(Some(fd)),
            Err(Errno::ENOENT) if create => {
                match mkdirat(parent, name, Mode::from_bits_truncate(0o755)) {
                    Ok(()) | Err(Errno::EEXIST) => {}
                    Err(err) => return Err(io("creating a folder in the repository's .git", err)),
                }
            }
            Err(Errno::ENOENT) => return Ok(None),
            Err(Errno::ELOOP | Errno::ENOTDIR) => {
                return Err(refused(
                    "repository",
                    "a folder in its .git is a link or a file",
                ));
            }
            Err(err) => return Err(io("opening a folder in the repository's .git", err)),
        }
    }
    Err(refused(
        "repository",
        "a folder in its .git keeps vanishing",
    ))
}

/// Refuses anything but a regular file.
pub(crate) fn regular(mode: nix::sys::stat::mode_t, what: &'static str) -> Result<(), GitError> {
    match SFlag::from_bits_truncate(mode) & SFlag::S_IFMT {
        SFlag::S_IFREG => Ok(()),
        SFlag::S_IFLNK => Err(refused(what, "it is a link")),
        _ => Err(refused(what, "it is not a regular file")),
    }
}

/// Whether `name` is in `dir`; refused unless it is missing or a regular file.
pub(crate) fn is_regular_in(
    dir: &OwnedFd,
    name: &str,
    what: &'static str,
) -> Result<bool, GitError> {
    match fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => regular(stat.st_mode, what).map(|()| true),
        Err(Errno::ENOENT) => Ok(false),
        Err(err) => Err(io("checking a file in the repository's .git", err)),
    }
}

/// The bytes and permission bits of the regular file `name` of `dir`, at most `max` bytes;
/// `None` when it is missing. A link, a FIFO (never waited on) or a larger file is refused.
pub(crate) fn read_in(
    dir: &OwnedFd,
    name: &str,
    max: u64,
    what: &'static str,
) -> Result<Option<(Vec<u8>, Mode)>, GitError> {
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = match openat(dir, name, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => return Ok(None),
        Err(Errno::ELOOP) => return Err(refused(what, "it is a link")),
        Err(err) => return Err(io("opening a file in the repository's .git", err)),
    };
    let stat = fstat(&file).map_err(|err| io("checking a file in the repository's .git", err))?;
    regular(stat.st_mode, what)?;
    let mut bytes = Vec::new();
    std::fs::File::from(file)
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io("reading a file in the repository's .git", err))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max {
        return Err(refused(what, "it is too large"));
    }
    Ok(Some((
        bytes,
        Mode::from_bits_truncate(stat.st_mode & 0o7777),
    )))
}

/// Puts `bytes` with `mode` at `name` of `dir`: a new file of a random name is written, synced
/// and renamed over the name, so a link there is replaced and never written through.
pub(crate) fn replace_in(
    dir: &OwnedFd,
    name: &str,
    bytes: &[u8],
    mode: Mode,
) -> Result<(), GitError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let flags =
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut last = Errno::EEXIST;
    for _ in 0..8 {
        let temp = format!(
            ".{name}.yulius-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos()),
        );
        let fd = match openat(dir, temp.as_str(), flags, Mode::from_bits_truncate(0o600)) {
            Ok(fd) => fd,
            Err(Errno::EEXIST) => continue,
            Err(err) => {
                last = err;
                break;
            }
        };
        let mut file = std::fs::File::from(fd);
        let written = file
            .write_all(bytes)
            .and_then(|()| fchmod(&file, mode).map_err(std::io::Error::from))
            .and_then(|()| file.sync_all())
            .map_err(|err| io("writing a file in the repository's .git", err))
            .and_then(|()| {
                renameat(dir, temp.as_str(), dir, name)
                    .map_err(|err| io("replacing a file in the repository's .git", err))
            });
        if written.is_err() {
            let _ = unlinkat(dir, temp.as_str(), UnlinkatFlags::NoRemoveDir);
        }
        return written;
    }
    Err(io("creating a file in the repository's .git", last))
}
