//! A copy's inventory (SPEC-460 MIG-031): entries and bytes for each top-level folder of the
//! data directory and a fingerprint over every entry's path, kind, size, owner, mode, time and
//! restorable attributes in archive order. The exporting host counts it while it plans the
//! archive, the importing host counts the installed copy, and a move starts the new instance
//! only when the two are equal. Both read entries as the import leaves them and in the exporting
//! host's ids, so a remap of the service's ids changes nothing.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::archive::{Entry, Kind, headers};
use super::extract::{IdMap, restorable};
use super::walk::{self, LEFT_OUT, ROOT};
use super::{IdRange, Manifest};

/// The data directory itself in [`Inventory::folders`].
pub const TOP: &str = ".";

/// One top-level name of the data directory with everything below it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Its name ([`TOP`] for the data directory itself).
    pub name: String,
    /// Entries: itself and everything below.
    pub entries: u64,
    /// Bytes of its files (a hard-linked file once).
    pub bytes: u64,
    /// SHA-256 of its entries' headers, in hex.
    pub fingerprint: String,
}

/// What a data directory holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Every entry.
    pub entries: u64,
    /// Every file's bytes.
    pub bytes: u64,
    /// SHA-256 over the folders' names and fingerprints, in hex.
    pub fingerprint: String,
    /// The folders in archive order.
    pub folders: Vec<Folder>,
}

/// The exporting host's ids: its service user and that user's subordinate ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdSpace {
    /// The service uid.
    pub uid: u32,
    /// The service gid.
    pub gid: u32,
    /// Its subordinate uids.
    pub subuid: Option<IdRange>,
    /// Its subordinate gids.
    pub subgid: Option<IdRange>,
}

fn within(id: u32, own: u32, range: Option<IdRange>) -> bool {
    id == own || range.is_some_and(|r| id.checked_sub(r.start).is_some_and(|o| o < r.count))
}

/// What the import does to every entry whoever owns it: a link and a whiteout keep no
/// attributes, a file or directory only users' and overlay ones, and the data directory's own
/// time is not kept (removing the import's marker touches it).
fn settled(e: &mut Entry) {
    if matches!(e.kind, Kind::Symlink { .. } | Kind::Whiteout) {
        e.xattrs.clear();
    } else {
        e.xattrs.retain(|(name, _)| restorable(name));
    }
    if e.path == ROOT {
        e.mtime = 0;
    }
}

impl IdSpace {
    /// The ids a manifest names.
    #[must_use]
    pub fn of(m: &Manifest) -> Self {
        Self {
            uid: m.service.uid,
            gid: m.service.gid,
            subuid: m.subuid,
            subgid: m.subgid,
        }
    }

    /// `e` as the import installs it, still in this host's ids: an owner that is neither the
    /// service nor in its range becomes the service user, without setuid and setgid.
    #[must_use]
    pub fn as_installed(&self, e: &Entry) -> Entry {
        let mut e = e.clone();
        let (uid_ok, gid_ok) = (
            within(e.uid, self.uid, self.subuid),
            within(e.gid, self.gid, self.subgid),
        );
        if !(uid_ok && gid_ok) {
            if !uid_ok {
                e.uid = self.uid;
            }
            if !gid_ok {
                e.gid = self.gid;
            }
            e.mode &= !0o6000;
        }
        settled(&mut e);
        e
    }
}

fn back(id: u32, service: (u32, u32), sub: Option<(u32, u32, u32)>) -> Option<u32> {
    if id == service.1 {
        return Some(service.0);
    }
    let (from, to, count) = sub?;
    let offset = id.checked_sub(to).filter(|&o| o < count)?;
    from.checked_add(offset)
}

/// An installed entry in the exporting host's ids; `None` when its owner is neither this host's
/// service user nor in its range (nothing the import wrote).
#[must_use]
pub fn as_exported(e: &Entry, map: &IdMap) -> Option<Entry> {
    let mut e = e.clone();
    e.uid = back(e.uid, map.uid, map.subuid)?;
    e.gid = back(e.gid, map.gid, map.subgid)?;
    settled(&mut e);
    Some(e)
}

/// The folder of an archive path: its first name below `data`.
fn folder_of(path: &[u8]) -> String {
    path.strip_prefix(ROOT)
        .and_then(|rest| rest.strip_prefix(b"/"))
        .map_or_else(
            || TOP.to_owned(),
            |rest| {
                let name = rest.split(|&b| b == b'/').next().unwrap_or(rest);
                String::from_utf8_lossy(name).into_owned()
            },
        )
}

/// Counts entries, already in their compared form, into an [`Inventory`].
pub struct Tally {
    folders: Vec<(Folder, Sha256)>,
}

impl std::fmt::Debug for Tally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tally")
            .field("folders", &self.folders.len())
            .finish()
    }
}

impl Default for Tally {
    fn default() -> Self {
        Self::new()
    }
}

impl Tally {
    /// An empty tally.
    #[must_use]
    pub fn new() -> Self {
        Self {
            folders: Vec::new(),
        }
    }

    /// Adds `e`; entries of one folder come together (the walk's order).
    pub fn add(&mut self, e: &Entry) {
        let name = folder_of(&e.path);
        if self.folders.last().is_none_or(|(f, _)| f.name != name) {
            self.folders.push((
                Folder {
                    name,
                    entries: 0,
                    bytes: 0,
                    fingerprint: String::new(),
                },
                Sha256::new(),
            ));
        }
        if let Some((folder, hash)) = self.folders.last_mut() {
            folder.entries += 1;
            folder.bytes += e.size();
            hash.update(headers(e));
        }
    }

    /// The inventory.
    #[must_use]
    pub fn finish(self) -> Inventory {
        let mut all = Sha256::new();
        let mut inventory = Inventory::default();
        for (mut folder, hash) in self.folders {
            folder.fingerprint = hex::encode(hash.finalize());
            all.update(folder.name.as_bytes());
            all.update([0]);
            all.update(folder.fingerprint.as_bytes());
            inventory.entries += folder.entries;
            inventory.bytes += folder.bytes;
            inventory.folders.push(folder);
        }
        inventory.fingerprint = hex::encode(all.finalize());
        inventory
    }
}

/// The exporting host's tally: give it every entry the export plans
/// ([`super::export::plan_observed`]).
#[derive(Debug)]
pub struct Counter {
    ids: IdSpace,
    tally: Tally,
}

impl Counter {
    /// A counter for a host with `ids`.
    #[must_use]
    pub fn new(ids: IdSpace) -> Self {
        Self {
            ids,
            tally: Tally::new(),
        }
    }

    /// Counts `e` as the import will install it.
    pub fn add(&mut self, e: &Entry) {
        self.tally.add(&self.ids.as_installed(e));
    }

    /// The inventory.
    #[must_use]
    pub fn finish(self) -> Inventory {
        self.tally.finish()
    }
}

/// The inventory of the copy installed at `root` with `map`, over the top-level names `old`
/// lists: what the new host put beside them (a skeleton's dotfiles, the import's marks, its own
/// sandbox base) is not the copy.
///
/// # Errors
/// A directory or file that cannot be read, or an entry whose owner the import never gives.
pub fn of_copy(root: &Path, map: &IdMap, old: &Inventory) -> io::Result<Inventory> {
    let mut tally = Tally::new();
    let copied = |name: &[u8]| {
        !LEFT_OUT.iter().any(|l| name == l.as_bytes())
            && old.folders.iter().any(|f| f.name.as_bytes() == name)
    };
    walk::walk_top(root, &copied, &mut |e, _| {
        let e = as_exported(e, map).ok_or_else(|| {
            io::Error::other(format!(
                "{} belongs to {}:{}, which is neither the service user nor one of its ids",
                String::from_utf8_lossy(&e.path),
                e.uid,
                e.gid
            ))
        })?;
        tally.add(&e);
        Ok(())
    })?;
    Ok(tally.finish())
}

/// Why `new` (the copy) is not `old` (the exporting host), naming the first folder that differs;
/// `None` when they are equal.
#[must_use]
pub fn difference(old: &Inventory, new: &Inventory) -> Option<String> {
    if old == new {
        return None;
    }
    let counts = |f: &Folder| format!("{} entries, {} bytes", f.entries, f.bytes);
    for folder in &old.folders {
        let Some(copy) = new.folders.iter().find(|f| f.name == folder.name) else {
            return Some(format!(
                "{}: {} on the old host; missing in the copy",
                folder.name,
                counts(folder)
            ));
        };
        if (folder.entries, folder.bytes) != (copy.entries, copy.bytes) {
            return Some(format!(
                "{}: {} on the old host; {} in the copy",
                folder.name,
                counts(folder),
                counts(copy)
            ));
        }
        if folder.fingerprint != copy.fingerprint {
            return Some(format!(
                "{}: {} on both hosts, but an entry's name, kind, owner, mode, time or \
                 attributes differs",
                folder.name,
                counts(folder)
            ));
        }
    }
    let extra = new
        .folders
        .iter()
        .find(|f| !old.folders.iter().any(|o| o.name == f.name))
        .map_or_else(|| "its folders".to_owned(), |f| f.name.clone());
    Some(format!("{extra}: in the copy, not on the old host"))
}

#[cfg(test)]
mod tests;
