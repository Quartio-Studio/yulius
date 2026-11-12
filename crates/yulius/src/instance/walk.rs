//! The data directory as archive entries (SPEC-329 EXP-004, EXP-006): parents before children,
//! names sorted bytewise, links kept as links, hard links pointing at their first path. The
//! export walks twice, once to plan the exact length and once to stream, so nothing is held in
//! memory but the hard-linked files' first paths; the instance is frozen, so both walks agree.
//!
//! A move also walks while the instance runs (SPEC-465). Such a walk holds every directory it
//! is in open and names its entries through that, so a directory that is renamed or replaced by
//! a link meanwhile cannot lead it anywhere else: what a sandbox's processes do to their own
//! files never makes a walk that runs as root read outside the data directory.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use super::archive::{Entry, Kind};

/// Top-level names the new host makes its own (EXP-006): the sandbox base (a copy of this host's
/// system), live processes' runtime files, the update helper's request and the move helper's
/// (SPEC-470: what was asked of this host's root is never asked of another's), SQLite's
/// shared-memory index, the filesystem's own `lost+found`, and the Claude Code builds for this
/// host's platform (SPEC-421: the new host downloads its own within the hour).
pub const LEFT_OUT: [&str; 7] = [
    "system",
    "runtime",
    "update",
    "migrate",
    "yulius.db-shm",
    "lost+found",
    "claude-code",
];

/// The import's own mark at the top (SPEC-459 HSL-005). Every import writes a new one, so the
/// old host's never travels and is never counted, whatever sits at its name (SPEC-467 LFT-005).
/// It is not in [`LEFT_OUT`], which also says what may be in a target before an import whatever
/// its kind: the mark may be there only as a file.
const IMPORT_MARK: &str = yulius_app::limits::ADAPT_MARK;

/// Whether a copy leaves the top-level `name` out: the export does not send it, and neither
/// host counts it ([`super::inventory`]).
#[must_use]
pub fn left_out(name: &[u8]) -> bool {
    name == IMPORT_MARK.as_bytes() || LEFT_OUT.iter().any(|l| name == l.as_bytes())
}

/// The archive path of the data directory itself.
pub const ROOT: &[u8] = b"data";

/// What the walk did not archive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    /// Sockets, FIFOs and devices other than whiteouts.
    pub special: u64,
    /// Paths that were gone when a live walk looked at them.
    pub vanished: u64,
}

/// An entry as the walk found it: its archive form and what identifies its state on this host.
#[derive(Debug)]
pub struct Seen<'a> {
    /// The entry.
    pub entry: &'a Entry,
    /// A path to it on disk, good while it is shown: in a live walk it goes through the open
    /// directory that holds the entry, so only its last name is looked up (never follow it).
    pub disk: &'a Path,
    /// Its inode number.
    pub ino: u64,
    /// Its filesystem.
    pub dev: u64,
    /// Its link count.
    pub nlink: u64,
    /// Its inode's change time: seconds and nanoseconds.
    pub ctime: (i64, i64),
}

/// Whether `e` says that a path is not there (any more): what a walk over a running instance
/// leaves out instead of failing.
#[must_use]
pub fn gone(e: &io::Error) -> bool {
    [
        rustix::io::Errno::NOENT,
        rustix::io::Errno::NOTDIR,
        rustix::io::Errno::LOOP,
        rustix::io::Errno::STALE,
        // What cannot be opened: a socket where a file was.
        rustix::io::Errno::NXIO,
    ]
    .iter()
    .any(|errno| e.raw_os_error() == Some(errno.raw_os_error()))
}

/// Walks `root`, calling `visit` with each entry and its path on disk, in archive order, without
/// the top-level names a copy leaves out ([`left_out`]).
///
/// # Errors
/// A directory or file that cannot be read, or `visit`'s error.
pub fn walk(
    root: &Path,
    visit: &mut dyn FnMut(&Entry, &Path) -> io::Result<()>,
) -> io::Result<Skipped> {
    walk_top(root, &copied, visit)
}

/// Whether a copy carries the top-level name `name`: every name but those it leaves out
/// ([`left_out`]).
#[must_use]
pub fn copied(name: &[u8]) -> bool {
    !left_out(name)
}

/// [`walk`] over the top-level names `keep` accepts (and everything below them).
///
/// # Errors
/// As [`walk`].
pub fn walk_top(
    root: &Path,
    keep: &dyn Fn(&[u8]) -> bool,
    visit: &mut dyn FnMut(&Entry, &Path) -> io::Result<()>,
) -> io::Result<Skipped> {
    walk_seen(root, false, keep, &mut |seen| {
        visit(seen.entry, seen.disk).map(|()| true)
    })
}

/// [`walk`] showing `visit` each entry's inode and change time. `visit` answers whether the
/// entry is there: `false` leaves out what is below it and lets no hard link name it. A `live`
/// walk is of a directory that changes meanwhile: a path that is gone when it is looked at is
/// left out and counted, not an error, and so is a directory that is no longer the one that was
/// looked at when it is opened. It never follows a link, also not one that took the place of a
/// directory above an entry: every directory is opened in the one that holds it and stays open
/// while the walk is in it (one open file for each level).
///
/// # Errors
/// As [`walk`].
pub fn walk_seen(
    root: &Path,
    live: bool,
    keep: &dyn Fn(&[u8]) -> bool,
    visit: &mut dyn FnMut(&Seen<'_>) -> io::Result<bool>,
) -> io::Result<Skipped> {
    let mut skipped = Skipped::default();
    let mut first_paths: HashMap<(u64, u64), Vec<u8>> = HashMap::new();
    let meta = std::fs::symlink_metadata(root)?;
    if !meta.is_dir() {
        return Err(io::Error::other(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let top = entry(ROOT.to_vec(), Kind::Dir, &meta, root)?;
    if !visit(&seen(&top, root, &meta))? {
        return Ok(skipped);
    }
    let Some((pin, dir)) = enter(root, &meta, live)? else {
        return Err(io::Error::other(format!(
            "{} is not the directory it was",
            root.display()
        )));
    };
    let mut stack = vec![Level {
        names: children(&dir)?,
        through: dir,
        archived: ROOT.to_vec(),
        _pin: pin,
    }];
    loop {
        let top_level = stack.len() == 1;
        let Some(Level {
            through: dir,
            archived,
            names,
            ..
        }) = stack.last_mut()
        else {
            break;
        };
        let Some(name) = names.pop() else {
            stack.pop();
            continue;
        };
        if top_level && !keep(name.as_bytes()) {
            continue;
        }
        let disk = dir.join(&name);
        let path = [archived.as_slice(), b"/", name.as_bytes()].concat();
        // What a live walk finds gone is left out; anything else is an error.
        let found =
            |skipped: &mut Skipped| -> io::Result<Option<(Entry, std::fs::Metadata, bool)>> {
                let meta = std::fs::symlink_metadata(&disk)?;
                let ft = meta.file_type();
                let mut first = false;
                let kind = if ft.is_dir() {
                    Kind::Dir
                } else if ft.is_symlink() {
                    Kind::Symlink {
                        target: link_text(&disk)?,
                    }
                } else if ft.is_file() {
                    match first_paths.get(&(meta.dev(), meta.ino())) {
                        Some(first) if meta.nlink() > 1 => Kind::HardLink {
                            target: first.clone(),
                        },
                        _ => {
                            first = meta.nlink() > 1;
                            Kind::File { size: meta.len() }
                        }
                    }
                } else if ft.is_char_device() && meta.rdev() == 0 {
                    Kind::Whiteout
                } else {
                    skipped.special += 1;
                    return Ok(None);
                };
                let e = entry(path.clone(), kind, &meta, &disk)?;
                Ok(Some((e, meta, first)))
            };
        let (e, meta, first) = match found(&mut skipped) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(err) if live && gone(&err) => {
                skipped.vanished += 1;
                continue;
            }
            Err(err) => return Err(err),
        };
        let link = (meta.dev(), meta.ino());
        if first {
            first_paths.insert(link, path.clone());
        }
        if !visit(&seen(&e, &disk, &meta))? {
            if first {
                first_paths.remove(&link);
            }
            continue;
        }
        if e.kind == Kind::Dir {
            let opened = enter(&disk, &meta, live).and_then(|entered| match entered {
                Some((pin, through)) => Ok(Some(Level {
                    names: children(&through)?,
                    through,
                    archived: path,
                    _pin: pin,
                })),
                None => Ok(None),
            });
            match opened {
                Ok(Some(level)) => stack.push(level),
                // Something else took the directory's place since the walk looked at it.
                Ok(None) => skipped.vanished += 1,
                Err(err) if live && gone(&err) => skipped.vanished += 1,
                Err(err) => return Err(err),
            }
        }
    }
    Ok(skipped)
}

/// A directory the walk is in.
struct Level {
    /// The path its entries are named through.
    through: PathBuf,
    /// Its archive path.
    archived: Vec<u8>,
    /// Its names not looked at yet, the next one last.
    names: Vec<OsString>,
    /// What keeps it open during a live walk.
    _pin: Option<std::fs::File>,
}

/// The directory at `disk`, which the walk saw as `meta`, as the walk reads it: what keeps it
/// open and the path its entries are named through. A live walk on Linux opens it without
/// following a link (a link there now is gone, [`gone`]) and names its entries through the open
/// directory, so nothing that happens to `disk` afterwards changes where they are; `None` when
/// another directory is there now. Otherwise the directory is read by its path: the instance is
/// frozen, or nothing else writes there.
fn enter(
    disk: &Path,
    meta: &std::fs::Metadata,
    live: bool,
) -> io::Result<Option<(Option<std::fs::File>, PathBuf)>> {
    if !(live && cfg!(target_os = "linux")) {
        return Ok(Some((None, disk.to_path_buf())));
    }
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let dir = std::fs::File::options()
        .read(true)
        .custom_flags((nix::fcntl::OFlag::O_DIRECTORY | nix::fcntl::OFlag::O_NOFOLLOW).bits())
        .open(disk)?;
    let now = dir.metadata()?;
    if (now.dev(), now.ino()) != (meta.dev(), meta.ino()) {
        return Ok(None);
    }
    let through = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    Ok(Some((Some(dir), through)))
}

/// A link's text. A path that is no link any more (it was one when it was looked at) is
/// gone ([`gone`]).
fn link_text(disk: &Path) -> io::Result<Vec<u8>> {
    match std::fs::read_link(disk) {
        Ok(target) => Ok(target.into_os_string().into_vec()),
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error()) => Err(
            io::Error::from_raw_os_error(rustix::io::Errno::NOENT.raw_os_error()),
        ),
        Err(e) => Err(e),
    }
}

fn seen<'a>(entry: &'a Entry, disk: &'a Path, meta: &std::fs::Metadata) -> Seen<'a> {
    Seen {
        entry,
        disk,
        ino: meta.ino(),
        dev: meta.dev(),
        nlink: meta.nlink(),
        ctime: (meta.ctime(), meta.ctime_nsec()),
    }
}

/// A directory's names, sorted so that `pop` yields them in bytewise order.
fn children(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort_by(|a, b| b.as_bytes().cmp(a.as_bytes()));
    // A directory that changes while it is listed may name an entry twice.
    names.dedup();
    Ok(names)
}

fn entry(path: Vec<u8>, kind: Kind, meta: &std::fs::Metadata, disk: &Path) -> io::Result<Entry> {
    Ok(described(path, kind, meta, xattrs(disk)?))
}

/// The entry at `path` of `kind` with `meta`'s owner, mode and time.
#[must_use]
pub fn described(
    path: Vec<u8>,
    kind: Kind,
    meta: &std::fs::Metadata,
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
) -> Entry {
    Entry {
        path,
        kind,
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        mtime: meta.mtime(),
        xattrs,
    }
}

/// A path's extended attributes (not following a final link), sorted by name; none where the
/// filesystem keeps none.
///
/// # Errors
/// A listed attribute that cannot be read.
pub fn xattrs(path: &Path) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    read_xattrs(
        &mut |names| rustix::fs::llistxattr(path, names),
        &mut |name, value| rustix::fs::lgetxattr(path, name, value),
    )
}

/// An open file's extended attributes, as [`xattrs`] gives a path's.
///
/// # Errors
/// A listed attribute that cannot be read.
pub fn xattrs_of(file: &impl std::os::fd::AsFd) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    read_xattrs(
        &mut |names| rustix::fs::flistxattr(file, names),
        &mut |name, value| rustix::fs::fgetxattr(file, name, value),
    )
}

fn read_xattrs(
    list: &mut dyn FnMut(&mut [u8]) -> rustix::io::Result<usize>,
    get: &mut dyn FnMut(&std::ffi::OsStr, &mut [u8]) -> rustix::io::Result<usize>,
) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut names = vec![0u8; 4096];
    let len = loop {
        match list(&mut names[..]) {
            Ok(len) => break len,
            Err(rustix::io::Errno::RANGE) => {
                let need = list(&mut [0u8; 0][..]).map_err(io::Error::from)?;
                names.resize(need.max(names.len() * 2), 0);
            }
            Err(rustix::io::Errno::NOTSUP) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        }
    };
    let mut out = Vec::new();
    for name in names[..len].split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let cname = std::ffi::OsStr::from_bytes(name);
        let mut value = vec![0u8; 256];
        let got = loop {
            match get(cname, &mut value[..]) {
                Ok(got) => break Some(got),
                Err(rustix::io::Errno::RANGE) => {
                    let need = get(cname, &mut [0u8; 0][..]).map_err(io::Error::from)?;
                    value.resize(need.max(value.len() * 2), 0);
                }
                // Removed between the list and the read: not there.
                Err(rustix::io::Errno::NODATA) => break None,
                Err(e) => return Err(e.into()),
            }
        };
        if let Some(got) = got {
            value.truncate(got);
            out.push((name.to_vec(), value));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests;
