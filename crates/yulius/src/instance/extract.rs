//! Installing an archive's `data/` entries (SPEC-329 IMP-003, IMP-004): every directory on the
//! way is opened with `O_NOFOLLOW` from the data directory's handle, so no link, whether the
//! archive's or one already there, takes a write elsewhere; owners are remapped to this host's
//! service user and its subordinate range, and nothing ends up owned by root or a foreign id.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write as _};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use rustix::fs::{
    AtFlags, CWD, FileType, Gid, Mode, OFlags, RawMode, Timespec, Timestamps, Uid, XattrFlags,
};

use super::archive::{Entry, Kind};
use super::walk::ROOT;

/// How ids move from the exporting host to this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdMap {
    /// The service uid there and here.
    pub uid: (u32, u32),
    /// The service gid there and here.
    pub gid: (u32, u32),
    /// The subordinate uid range there and here (start, start, count).
    pub subuid: Option<(u32, u32, u32)>,
    /// The subordinate gid range there and here.
    pub subgid: Option<(u32, u32, u32)>,
}

fn map_id(id: u32, service: (u32, u32), sub: Option<(u32, u32, u32)>) -> Option<u32> {
    if id == service.0 {
        return Some(service.1);
    }
    let (from, to, count) = sub?;
    let offset = id.checked_sub(from).filter(|&o| o < count)?;
    to.checked_add(offset)
}

impl IdMap {
    /// This host's uid for `uid`; `None` when it is foreign (neither the service nor its range).
    #[must_use]
    pub fn uid(&self, uid: u32) -> Option<u32> {
        map_id(uid, self.uid, self.subuid)
    }

    /// This host's gid for `gid`; `None` when it is foreign.
    #[must_use]
    pub fn gid(&self, gid: u32) -> Option<u32> {
        map_id(gid, self.gid, self.subgid)
    }
}

/// What the extraction changed from the archive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Entries installed.
    pub entries: u64,
    /// Entries whose owner was foreign and became the service user (without setuid/setgid).
    pub foreign_owners: u64,
    /// Extended attributes not restored (not `user.*` or `trusted.overlay.*`, on a link, or
    /// not kept by this filesystem).
    pub skipped_xattrs: u64,
}

/// Permission bits as `rustix` takes them (`mode_t` is 16 bits on macOS).
fn mode_bits(bits: u32) -> Mode {
    Mode::from_bits_truncate(RawMode::try_from(bits & 0o7777).unwrap_or(0))
}

/// Makes an overlay whiteout, the character device 0:0.
#[cfg(target_os = "linux")]
fn whiteout(dir: &OwnedFd, name: &OsStr, mode: u32) -> io::Result<()> {
    rustix::fs::mknodat(dir, name, FileType::CharacterDevice, mode_bits(mode), 0)?;
    Ok(())
}

/// Overlay whiteouts exist only on Linux, where every export comes from.
#[cfg(not(target_os = "linux"))]
fn whiteout(_: &OwnedFd, name: &OsStr, _: u32) -> io::Result<()> {
    Err(io::Error::other(format!(
        "{}: an overlay whiteout needs Linux",
        name.to_string_lossy()
    )))
}

/// A path's digest: the first 128 bits of its SHA-256, so two paths of one copy never share it.
fn path_hash(path: &[u8]) -> [u8; 16] {
    let digest = Sha256::digest(path);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Whether an extended attribute is restored (IMP-003): overlay layers' and users' own; the rest
/// hold host ids or grant privileges.
#[must_use]
pub fn restorable(name: &[u8]) -> bool {
    name.starts_with(b"user.") || name.starts_with(b"trusted.overlay.")
}

fn refuse(path: &[u8], why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("refused {:?}: {why}", String::from_utf8_lossy(path)),
    )
}

/// The components of an archive path below `data`.
///
/// # Errors
/// Not `data` or below it, or an empty, `.`, `..` or NUL-holding component.
pub fn components(path: &[u8]) -> io::Result<Vec<&[u8]>> {
    let mut parts = path.split(|&b| b == b'/');
    if parts.next() != Some(ROOT) {
        return Err(refuse(path, "not inside the data directory"));
    }
    let rest: Vec<&[u8]> = parts.collect();
    if rest
        .iter()
        .any(|c| c.is_empty() || *c == b"." || *c == b".." || c.contains(&0))
    {
        return Err(refuse(path, "not a plain relative path"));
    }
    Ok(rest)
}

/// Installs entries below a data directory.
pub struct Extractor {
    root: OwnedFd,
    map: IdMap,
    chown: bool,
    /// The last parent directory opened, by its components.
    parent: Option<(Vec<Vec<u8>>, OwnedFd)>,
    /// Directories' times, set at the end (their entries change them).
    dirs: Vec<(Vec<u8>, i64)>,
    /// Digests of the regular files' paths written so far: a hard link must name one of them.
    files: HashSet<[u8; 16]>,
    report: Report,
}

impl std::fmt::Debug for Extractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extractor")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn times(mtime: i64) -> Timestamps {
    let t = Timespec {
        tv_sec: mtime,
        tv_nsec: 0,
    };
    Timestamps {
        last_access: t,
        last_modification: t,
    }
}

impl Extractor {
    /// An extractor into the existing directory `root`; `chown` off only where the caller cannot
    /// (tests that do not run as root).
    ///
    /// # Errors
    /// `root` cannot be opened as a directory (a link is refused).
    pub fn new(root: &Path, map: IdMap, chown: bool) -> io::Result<Self> {
        let root = rustix::fs::openat(CWD, root, dir_flags(), Mode::empty())?;
        Ok(Self {
            root,
            map,
            chown,
            parent: None,
            dirs: Vec::new(),
            files: HashSet::new(),
            report: Report::default(),
        })
    }

    /// Opens the directory of `comps` below the root without following any link.
    fn open_dir(&self, comps: &[&[u8]]) -> io::Result<OwnedFd> {
        let mut fd = rustix::fs::openat(&self.root, ".", dir_flags(), Mode::empty())?;
        for c in comps {
            fd = rustix::fs::openat(&fd, OsStr::from_bytes(c), dir_flags(), Mode::empty())
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "refused a path through {:?}: not a directory of the copy ({e})",
                            String::from_utf8_lossy(c)
                        ),
                    )
                })?;
        }
        Ok(fd)
    }

    fn parent_of(&mut self, comps: &[&[u8]]) -> io::Result<OwnedFd> {
        let key: Vec<Vec<u8>> = comps.iter().map(|c| c.to_vec()).collect();
        if let Some((cached, fd)) = &self.parent
            && *cached == key
        {
            return fd.try_clone();
        }
        let fd = self.open_dir(comps)?;
        self.parent = Some((key, fd.try_clone()?));
        Ok(fd)
    }

    /// Removes what is at `name` unless it is a directory and `keep_dir`; `true` when a directory
    /// is kept.
    fn clear(dir: &OwnedFd, name: &OsStr, keep_dir: bool) -> io::Result<bool> {
        match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Directory => {
                if keep_dir {
                    Ok(true)
                } else {
                    Err(io::Error::other(format!(
                        "{} is a directory where the copy has a file",
                        name.to_string_lossy()
                    )))
                }
            }
            Ok(_) => {
                rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
                Ok(false)
            }
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// The owner and mode `e` gets here.
    fn owner(&mut self, e: &Entry) -> (u32, u32, u32) {
        match (self.map.uid(e.uid), self.map.gid(e.gid)) {
            (Some(uid), Some(gid)) => (uid, gid, e.mode),
            (uid, gid) => {
                self.report.foreign_owners += 1;
                (
                    uid.unwrap_or(self.map.uid.1),
                    gid.unwrap_or(self.map.gid.1),
                    e.mode & !0o6000,
                )
            }
        }
    }

    /// Owner, then mode, then attributes, then time: a chown clears setuid bits and capabilities.
    fn finish_fd(&mut self, fd: &OwnedFd, e: &Entry, mtime: bool) -> io::Result<()> {
        let (uid, gid, mode) = self.owner(e);
        if self.chown {
            rustix::fs::fchown(fd, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))?;
        }
        rustix::fs::fchmod(fd, mode_bits(mode))?;
        for (name, value) in &e.xattrs {
            if !restorable(name) {
                self.report.skipped_xattrs += 1;
                continue;
            }
            let name = OsStr::from_bytes(name);
            match rustix::fs::fsetxattr(fd, name, value, XattrFlags::empty()) {
                Ok(()) => {}
                Err(rustix::io::Errno::NOTSUP | rustix::io::Errno::PERM) => {
                    self.report.skipped_xattrs += 1;
                }
                Err(err) => return Err(err.into()),
            }
        }
        if mtime {
            rustix::fs::futimens(fd, &times(e.mtime))?;
        }
        Ok(())
    }

    /// Installs `e`, reading a file's content from `data`.
    ///
    /// # Errors
    /// A refused path, a link through which it would write, a hard link to something that is
    /// not a file of the copy, or an I/O error.
    pub fn put(&mut self, e: &Entry, data: &mut dyn Read) -> io::Result<()> {
        let comps = components(&e.path)?;
        self.report.entries += 1;
        let Some((last, parents)) = comps.split_last() else {
            if e.kind != Kind::Dir {
                return Err(refuse(&e.path, "the data directory must be a directory"));
            }
            let root = self.root.try_clone()?;
            self.dirs.push((e.path.clone(), e.mtime));
            return self.finish_fd(&root, e, false);
        };
        let parent = self.parent_of(parents)?;
        let name = OsStr::from_bytes(last);
        match &e.kind {
            Kind::Dir => {
                if !Self::clear(&parent, name, true)? {
                    rustix::fs::mkdirat(&parent, name, mode_bits(0o700))?;
                }
                let fd = rustix::fs::openat(&parent, name, dir_flags(), Mode::empty())?;
                self.dirs.push((e.path.clone(), e.mtime));
                self.finish_fd(&fd, e, false)
            }
            Kind::File { size } => {
                Self::clear(&parent, name, false)?;
                let fd = rustix::fs::openat(
                    &parent,
                    name,
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    mode_bits(0o600),
                )?;
                let mut file = File::from(fd);
                let copied = io::copy(&mut data.take(*size), &mut file)?;
                if copied != *size {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the stream ended inside a file (an incomplete copy)",
                    ));
                }
                file.flush()?;
                self.files.insert(path_hash(&e.path));
                let fd = OwnedFd::from(file);
                self.finish_fd(&fd, e, true)
            }
            Kind::Symlink { target } => {
                Self::clear(&parent, name, false)?;
                rustix::fs::symlinkat(OsStr::from_bytes(target), &parent, name)?;
                let (uid, gid, _) = self.owner(e);
                if self.chown {
                    rustix::fs::chownat(
                        &parent,
                        name,
                        Some(Uid::from_raw(uid)),
                        Some(Gid::from_raw(gid)),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )?;
                }
                self.report.skipped_xattrs += e.xattrs.len() as u64;
                rustix::fs::utimensat(&parent, name, &times(e.mtime), AtFlags::SYMLINK_NOFOLLOW)?;
                Ok(())
            }
            Kind::HardLink { target } => {
                if !self.files.contains(&path_hash(target)) {
                    return Err(refuse(&e.path, "a hard link to a file not in the copy"));
                }
                let tcomps = components(target)?;
                let Some((tlast, tparents)) = tcomps.split_last() else {
                    return Err(refuse(&e.path, "a hard link to the data directory"));
                };
                let tparent = self.open_dir(tparents)?;
                let tname = OsStr::from_bytes(tlast);
                let st = rustix::fs::statat(&tparent, tname, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|_| refuse(&e.path, "a hard link to a file not in the copy"))?;
                if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
                    return Err(refuse(
                        &e.path,
                        "a hard link to something other than a file",
                    ));
                }
                Self::clear(&parent, name, false)?;
                rustix::fs::linkat(&tparent, tname, &parent, name, AtFlags::empty())?;
                Ok(())
            }
            Kind::Whiteout => {
                Self::clear(&parent, name, false)?;
                let (uid, gid, mode) = self.owner(e);
                whiteout(&parent, name, mode)?;
                if self.chown {
                    rustix::fs::chownat(
                        &parent,
                        name,
                        Some(Uid::from_raw(uid)),
                        Some(Gid::from_raw(gid)),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )?;
                }
                rustix::fs::utimensat(&parent, name, &times(e.mtime), AtFlags::SYMLINK_NOFOLLOW)?;
                Ok(())
            }
        }
    }

    /// Sets the directories' times and returns what changed from the archive.
    ///
    /// # Errors
    /// A directory that can no longer be opened.
    pub fn finish(mut self) -> io::Result<Report> {
        self.parent = None;
        for (path, mtime) in std::mem::take(&mut self.dirs) {
            let comps = components(&path)?;
            let fd = self.open_dir(&comps)?;
            rustix::fs::futimens(fd.as_fd(), &times(mtime))?;
        }
        Ok(self.report)
    }
}

#[cfg(test)]
mod tests;
