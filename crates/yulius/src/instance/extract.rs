//! Installing an archive's `data/` entries (SPEC-329 IMP-003, IMP-004): every directory on the
//! way is opened with `O_NOFOLLOW` from the data directory's handle, so no link, whether the
//! archive's or one already there, takes a write elsewhere; owners are remapped to this host's
//! service user and its subordinate range, and nothing ends up owned by root or a foreign id.
//!
//! A pass of a move installs onto a staged copy ([`Onto`], SPEC-465 WCP-008, WCP-009): an entry
//! replaces what is at its path, a link may name a file of an earlier pass, a directory that
//! is not sent keeps its time, and only the frozen pass removes.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write as _};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::Path;

use rustix::fs::{
    AtFlags, CWD, FileType, Gid, Mode, OFlags, RawMode, Timespec, Timestamps, Uid, XattrFlags,
};

use super::archive::{Entry, Kind, path_digest as path_hash};
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

/// What entries are installed onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onto {
    /// A new copy: nothing of an earlier stream is there, and a directory where the copy has a
    /// file is an error.
    New,
    /// A move's staged copy, from a pass read while the instance ran: nothing is removed. An
    /// entry that would need a directory removed is left out ([`Extractor::deferred`]).
    Warm,
    /// A staged copy, from the frozen pass: a directory in an entry's way goes with everything
    /// below it, and [`Extractor::prune`] removes what the old host no longer holds.
    Frozen,
}

/// Whether an entry's place is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// Nothing is there (any more).
    Clear,
    /// A directory is there and stays.
    Dir,
    /// A directory is there, and a warm pass removes none.
    Blocked,
}

/// Installs entries below a data directory.
pub struct Extractor {
    root: OwnedFd,
    map: IdMap,
    chown: bool,
    onto: Onto,
    /// The last parent directory opened, by its components.
    parent: Option<(Vec<Vec<u8>>, OwnedFd)>,
    /// Directories' times, set at the end (their entries change them).
    dirs: Vec<(Vec<u8>, i64)>,
    /// Digests of the regular files' paths written so far: a hard link must name one of them.
    files: HashSet<[u8; 16]>,
    /// The times of the staged copy's directories something was written or removed in: they
    /// get them back at the end unless they were sent themselves.
    touched: HashMap<Vec<u8>, Timestamps>,
    /// The entries (by number) a warm pass left out.
    deferred: Vec<u64>,
    /// Something of the staged copy was removed beside the entries' own paths.
    cleared: bool,
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
        Self::onto(root, map, chown, Onto::New)
    }

    /// [`Extractor::new`] onto a new copy or a move's staged one.
    ///
    /// # Errors
    /// As [`Extractor::new`].
    pub fn onto(root: &Path, map: IdMap, chown: bool, onto: Onto) -> io::Result<Self> {
        let root = rustix::fs::openat(CWD, root, dir_flags(), Mode::empty())?;
        Ok(Self {
            root,
            map,
            chown,
            onto,
            parent: None,
            dirs: Vec::new(),
            files: HashSet::new(),
            touched: HashMap::new(),
            deferred: Vec::new(),
            cleared: false,
            report: Report::default(),
        })
    }

    /// The entries (numbered from 0 in the order they were put) that were not installed: they
    /// would need a directory removed, which only the frozen pass does.
    #[must_use]
    pub fn deferred(&self) -> &[u64] {
        &self.deferred
    }

    /// Whether paths other than the entries' own were removed from the staged copy (what was
    /// below a directory an entry replaced, or what [`Extractor::prune`] took): what the old
    /// host knew of them no longer holds.
    #[must_use]
    pub fn cleared(&self) -> bool {
        self.cleared
    }

    /// Remembers the time of the staged copy's directory `comps` before something in it
    /// changes.
    fn touch(&mut self, comps: &[&[u8]], fd: &OwnedFd) -> io::Result<()> {
        if self.onto == Onto::New {
            return Ok(());
        }
        let path = [&[ROOT][..], comps].concat().join(&b'/');
        if let std::collections::hash_map::Entry::Vacant(slot) = self.touched.entry(path) {
            let st = rustix::fs::fstat(fd)?;
            slot.insert(Timestamps {
                last_access: Timespec {
                    tv_sec: st.st_atime,
                    tv_nsec: st.st_atime_nsec as _,
                },
                last_modification: Timespec {
                    tv_sec: st.st_mtime,
                    tv_nsec: st.st_mtime_nsec as _,
                },
            });
        }
        Ok(())
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
        self.touch(comps, &fd)?;
        self.parent = Some((key, fd.try_clone()?));
        Ok(fd)
    }

    /// Makes room at `name` for an entry: what is there goes, unless it is a directory and
    /// `keep_dir`. A directory in a file's way is an error in a new copy, stays in a warm pass
    /// and goes with everything below it in the frozen one.
    fn clear(&mut self, dir: &OwnedFd, name: &OsStr, keep_dir: bool) -> io::Result<Way> {
        match rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Directory => {
                match (keep_dir, self.onto) {
                    (true, _) => Ok(Way::Dir),
                    (false, Onto::Warm) => Ok(Way::Blocked),
                    (false, Onto::Frozen) => {
                        self.cleared = true;
                        remove_tree(dir, name).map(|_| Way::Clear)
                    }
                    (false, Onto::New) => Err(io::Error::other(format!(
                        "{} is a directory where the copy has a file",
                        name.to_string_lossy()
                    ))),
                }
            }
            Ok(_) => {
                rustix::fs::unlinkat(dir, name, AtFlags::empty())?;
                Ok(Way::Clear)
            }
            Err(rustix::io::Errno::NOENT) => Ok(Way::Clear),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether a hard link may name `target` although this stream did not write it: a file of
    /// the staged copy that an earlier pass brought.
    fn staged(&self, target: &[u8]) -> bool {
        self.onto != Onto::New
            && components(target)
                .ok()
                .and_then(|comps| comps.first().copied())
                .is_some_and(super::walk::copied)
    }

    /// The directory and name of the file the hard link `e` names; `None` when a warm pass
    /// left that file out (a directory is in its way), so the link waits with it.
    fn link_target(&self, e: &Entry, target: &[u8]) -> io::Result<Option<(OwnedFd, OsString)>> {
        if !self.files.contains(&path_hash(target)) && !self.staged(target) {
            return Err(refuse(&e.path, "a hard link to a file not in the copy"));
        }
        let tcomps = components(target)?;
        let Some((tlast, tparents)) = tcomps.split_last() else {
            return Err(refuse(&e.path, "a hard link to the data directory"));
        };
        let tparent = self.open_dir(tparents)?;
        let tname = OsStr::from_bytes(tlast).to_owned();
        let st = rustix::fs::statat(&tparent, &tname, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| refuse(&e.path, "a hard link to a file not in the copy"))?;
        if FileType::from_raw_mode(st.st_mode) == FileType::RegularFile {
            return Ok(Some((tparent, tname)));
        }
        if self.onto == Onto::Warm {
            return Ok(None);
        }
        Err(refuse(
            &e.path,
            "a hard link to something other than a file",
        ))
    }

    /// A directory that was there before takes the entry's attributes and no others: what the
    /// old host removed since is removed here.
    fn drop_xattrs(fd: &OwnedFd, e: &Entry) -> io::Result<()> {
        for (name, _) in super::walk::xattrs_of(fd)? {
            if restorable(&name) && !e.xattrs.iter().any(|(kept, _)| *kept == name) {
                rustix::fs::fremovexattr(fd, OsStr::from_bytes(&name))?;
            }
        }
        Ok(())
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
        let number = self.report.entries;
        self.report.entries += 1;
        let Some((last, parents)) = comps.split_last() else {
            if e.kind != Kind::Dir {
                return Err(refuse(&e.path, "the data directory must be a directory"));
            }
            let root = self.root.try_clone()?;
            self.dirs.push((e.path.clone(), e.mtime));
            if self.onto != Onto::New {
                Self::drop_xattrs(&root, e)?;
            }
            return self.finish_fd(&root, e, false);
        };
        let parent = self.parent_of(parents)?;
        let name = OsStr::from_bytes(last);
        // A link's file is looked at before anything makes room for the link.
        let linked = match &e.kind {
            Kind::HardLink { target } => match self.link_target(e, target)? {
                Some(file) => Some(file),
                None => {
                    self.deferred.push(number);
                    return Ok(());
                }
            },
            _ => None,
        };
        let way = self.clear(&parent, name, e.kind == Kind::Dir)?;
        if way == Way::Blocked {
            self.deferred.push(number);
            return Ok(());
        }
        match &e.kind {
            Kind::Dir => {
                if way == Way::Clear {
                    rustix::fs::mkdirat(&parent, name, mode_bits(0o700))?;
                }
                let fd = rustix::fs::openat(&parent, name, dir_flags(), Mode::empty())?;
                self.dirs.push((e.path.clone(), e.mtime));
                if way == Way::Dir && self.onto != Onto::New {
                    Self::drop_xattrs(&fd, e)?;
                }
                self.finish_fd(&fd, e, false)
            }
            Kind::File { size } => {
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
            Kind::HardLink { .. } => {
                if let Some((tparent, tname)) = linked {
                    rustix::fs::linkat(&tparent, &tname, &parent, name, AtFlags::empty())?;
                }
                Ok(())
            }
            Kind::Whiteout => {
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

    /// Removes from the staged copy what the old host no longer holds (SPEC-465 WCP-008):
    /// every path whose digest is not in `kept` (sorted), with everything below it. The
    /// top-level names `spared` accepts are this host's own and stay. Returns how many entries
    /// went.
    ///
    /// # Errors
    /// A directory that cannot be read or an entry that cannot be removed.
    pub fn prune(&mut self, kept: &[[u8; 16]], spared: &dyn Fn(&[u8]) -> bool) -> io::Result<u64> {
        self.parent = None;
        let root = self.root.try_clone()?;
        self.prune_in(&root, &mut Vec::new(), kept, spared)
    }

    fn prune_in(
        &mut self,
        dir: &OwnedFd,
        comps: &mut Vec<Vec<u8>>,
        kept: &[[u8; 16]],
        spared: &dyn Fn(&[u8]) -> bool,
    ) -> io::Result<u64> {
        let mut removed = 0;
        for (name, is_dir) in list(dir)? {
            let bytes = name.as_bytes();
            if comps.is_empty() && spared(bytes) {
                continue;
            }
            comps.push(bytes.to_vec());
            let path = [&[ROOT.to_vec()][..], comps.as_slice()]
                .concat()
                .join(&b'/');
            if kept.binary_search(&path_hash(&path)).is_err() {
                let parents: Vec<&[u8]> =
                    comps[..comps.len() - 1].iter().map(Vec::as_slice).collect();
                self.touch(&parents, dir)?;
                self.cleared = true;
                removed += remove_tree(dir, &name)?;
            } else if is_dir {
                let fd = rustix::fs::openat(dir, &name, dir_flags(), Mode::empty())?;
                removed += self.prune_in(&fd, comps, kept, spared)?;
            }
            comps.pop();
        }
        Ok(removed)
    }

    /// Sets the directories' times and returns what changed from the archive.
    ///
    /// # Errors
    /// A directory that can no longer be opened.
    pub fn finish(mut self) -> io::Result<Report> {
        self.parent = None;
        // A directory that was not sent keeps its time, whatever was written below it; one
        // that is gone by now has none to keep.
        for (path, was) in std::mem::take(&mut self.touched) {
            let fd = components(&path).and_then(|comps| self.open_dir(&comps));
            if let Ok(fd) = fd {
                rustix::fs::futimens(fd.as_fd(), &was)?;
            }
        }
        for (path, mtime) in std::mem::take(&mut self.dirs) {
            let comps = components(&path)?;
            let fd = self.open_dir(&comps)?;
            rustix::fs::futimens(fd.as_fd(), &times(mtime))?;
        }
        Ok(self.report)
    }
}

/// The names in `dir`, and whether each is a directory (not a link to one).
fn list(dir: &OwnedFd) -> io::Result<Vec<(OsString, bool)>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name = OsString::from_vec(name.to_vec());
        let is_dir = match entry.file_type() {
            FileType::Directory => true,
            FileType::Unknown => {
                let st = rustix::fs::statat(dir, &name, AtFlags::SYMLINK_NOFOLLOW)?;
                FileType::from_raw_mode(st.st_mode) == FileType::Directory
            }
            _ => false,
        };
        names.push((name, is_dir));
    }
    Ok(names)
}

/// Removes `name` in `parent` with everything below it, never through a link; how many
/// entries went.
fn remove_tree(parent: &OwnedFd, name: &OsStr) -> io::Result<u64> {
    let st = match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(st) => st,
        Err(rustix::io::Errno::NOENT) => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
        rustix::fs::unlinkat(parent, name, AtFlags::empty())?;
        return Ok(1);
    }
    let dir = rustix::fs::openat(parent, name, dir_flags(), Mode::empty())?;
    let mut removed = 1;
    for (child, _) in list(&dir)? {
        removed += remove_tree(&dir, &child)?;
    }
    rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR)?;
    Ok(removed)
}

#[cfg(test)]
mod tests;
