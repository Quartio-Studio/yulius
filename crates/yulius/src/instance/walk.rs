//! The data directory as archive entries (SPEC-329 EXP-004, EXP-006): parents before children,
//! names sorted bytewise, links kept as links, hard links pointing at their first path. The
//! export walks twice, once to plan the exact length and once to stream, so nothing is held in
//! memory but the hard-linked files' first paths; the instance is frozen, so both walks agree.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use super::archive::{Entry, Kind};

/// Top-level names the new host makes its own (EXP-006): the sandbox base (a copy of this host's
/// system), live processes' runtime files, the update helper's request, SQLite's shared-memory
/// index, and the filesystem's own `lost+found`.
pub const LEFT_OUT: [&str; 5] = ["system", "runtime", "update", "yulius.db-shm", "lost+found"];

/// The archive path of the data directory itself.
pub const ROOT: &[u8] = b"data";

/// What the walk did not archive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    /// Sockets, FIFOs and devices other than whiteouts.
    pub special: u64,
}

/// Walks `root`, calling `visit` with each entry and its path on disk, in archive order.
///
/// # Errors
/// A directory or file that cannot be read, or `visit`'s error.
pub fn walk(
    root: &Path,
    visit: &mut dyn FnMut(&Entry, &Path) -> io::Result<()>,
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
    visit(&top, root)?;
    let mut stack: Vec<(PathBuf, Vec<u8>, Vec<OsString>)> =
        vec![(root.to_path_buf(), ROOT.to_vec(), children(root)?)];
    loop {
        let top_level = stack.len() == 1;
        let Some((dir, archived, names)) = stack.last_mut() else {
            break;
        };
        let Some(name) = names.pop() else {
            stack.pop();
            continue;
        };
        if top_level && LEFT_OUT.iter().any(|l| name.as_bytes() == l.as_bytes()) {
            continue;
        }
        let disk = dir.join(&name);
        let path = [archived.as_slice(), b"/", name.as_bytes()].concat();
        let meta = std::fs::symlink_metadata(&disk)?;
        let ft = meta.file_type();
        let kind = if ft.is_dir() {
            Kind::Dir
        } else if ft.is_symlink() {
            Kind::Symlink {
                target: std::fs::read_link(&disk)?.into_os_string().into_vec(),
            }
        } else if ft.is_file() {
            if meta.nlink() > 1 {
                match first_paths.get(&(meta.dev(), meta.ino())) {
                    Some(first) => Kind::HardLink {
                        target: first.clone(),
                    },
                    None => {
                        first_paths.insert((meta.dev(), meta.ino()), path.clone());
                        Kind::File { size: meta.len() }
                    }
                }
            } else {
                Kind::File { size: meta.len() }
            }
        } else if ft.is_char_device() && meta.rdev() == 0 {
            Kind::Whiteout
        } else {
            skipped.special += 1;
            continue;
        };
        let is_dir = kind == Kind::Dir;
        let e = entry(path.clone(), kind, &meta, &disk)?;
        visit(&e, &disk)?;
        if is_dir {
            let names = children(&disk)?;
            stack.push((disk, path, names));
        }
    }
    Ok(skipped)
}

/// A directory's names, sorted so that `pop` yields them in bytewise order.
fn children(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort_by(|a, b| b.as_bytes().cmp(a.as_bytes()));
    Ok(names)
}

fn entry(path: Vec<u8>, kind: Kind, meta: &std::fs::Metadata, disk: &Path) -> io::Result<Entry> {
    Ok(Entry {
        path,
        kind,
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        mtime: meta.mtime(),
        xattrs: xattrs(disk)?,
    })
}

/// A path's extended attributes (not following a final link), sorted by name; none where the
/// filesystem keeps none.
///
/// # Errors
/// A listed attribute that cannot be read.
pub fn xattrs(path: &Path) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut names = vec![0u8; 4096];
    let len = loop {
        match rustix::fs::llistxattr(path, &mut names[..]) {
            Ok(len) => break len,
            Err(rustix::io::Errno::RANGE) => {
                let need =
                    rustix::fs::llistxattr(path, &mut [0u8; 0][..]).map_err(io::Error::from)?;
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
            match rustix::fs::lgetxattr(path, cname, &mut value[..]) {
                Ok(got) => break Some(got),
                Err(rustix::io::Errno::RANGE) => {
                    let need = rustix::fs::lgetxattr(path, cname, &mut [0u8; 0][..])
                        .map_err(io::Error::from)?;
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
