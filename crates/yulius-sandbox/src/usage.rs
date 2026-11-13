//! Disk usage of a sandbox for its soft quota (SPEC-002 ADR-22 §5): allocated blocks of every
//! file under the given roots, without following links or crossing filesystems, bounded in
//! entries.

use std::collections::{HashMap, HashSet};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Entries visited at most per measurement.
pub const MAX_ENTRIES: u64 = 4_000_000;
/// Folders nested deeper than this below a root are counted but not listed: the walk holds one
/// handle per level on the way down, and the depth is the sandbox's to choose (a chain of tens
/// of thousands of folders would use up the server's file descriptors).
pub const MAX_DEPTH: usize = 128;
/// Largest consumers kept per measurement (SPEC-059 DSK-202).
pub const LARGEST: usize = 5;

/// A measurement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    /// Bytes allocated (512-byte blocks × 512), hard links counted once.
    pub bytes: u64,
    /// Entries visited.
    pub entries: u64,
    /// A bound was reached ([`MAX_ENTRIES`], [`MAX_DEPTH`]); `bytes` is a lower bound.
    pub truncated: bool,
    /// The [`LARGEST`] largest consumers, largest first: a labelled root as one, else the
    /// directories two levels below an unlabelled root (`worktrees/<task>`, `ci/jobs`) and its
    /// own top-level files.
    pub largest: Vec<(String, u64)>,
}

/// Measures `roots` (missing roots count as empty); a root with a label is one consumer, one
/// without is split two levels down.
///
/// The trees are the sandbox's to write while they are measured, so they are walked through
/// handles (ADR-0315): a folder is opened from its parent without following a link and its
/// entries are looked at from that handle, never by a path looked up again. A folder swapped for
/// a link between its listing and its visit is skipped, not followed out of the sandbox.
#[must_use]
pub fn measure(roots: &[(&Path, Option<&str>)]) -> Usage {
    use nix::fcntl::{AtFlags, OFlag, open, openat};
    use nix::sys::stat::{Mode, SFlag, fstat, fstatat};
    let folder = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let listing = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
    let mut usage = Usage::default();
    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    let mut groups: HashMap<usize, u64> = HashMap::new();
    'roots: for (root, label) in roots {
        let Ok(top_dir) = open(*root, folder, Mode::empty()) else {
            continue;
        };
        let Ok(top) = fstat(&top_dir) else {
            continue;
        };
        let device = top.st_dev;
        let top_group = label.map(|label| {
            names.push(label.to_owned());
            names.len() - 1
        });
        usage.entries += 1;
        let root_bytes = blocks_bytes(top.st_blocks);
        usage.bytes = usage.bytes.saturating_add(root_bytes);
        if let Some(group) = top_group {
            *groups.entry(group).or_default() += root_bytes;
        }
        // Folders still to list: (the parent's handle and the name in it, or the root's own
        // handle; the path below the root; the depth below the root; the consumer once known).
        // A parent's handle lives while a child of it waits, so the open handles are those of
        // the folders on the way to the one being listed.
        let mut stack: Vec<(Pending, PathBuf, usize, Option<usize>)> =
            vec![(Pending::Open(top_dir), PathBuf::new(), 0, top_group)];
        while let Some((pending, below, depth, group)) = stack.pop() {
            let dir = match pending {
                Pending::Open(dir) => Rc::new(dir),
                Pending::Child(parent, name) => {
                    match openat(&*parent, name.as_c_str(), folder, Mode::empty()) {
                        Ok(dir) => Rc::new(dir),
                        // Gone, or no longer a folder (a link put there since): not followed.
                        Err(_) => continue,
                    }
                }
            };
            let Ok(entries) = nix::dir::Dir::openat(&*dir, ".", listing, Mode::empty()) else {
                continue;
            };
            for entry in entries.into_iter().flatten() {
                let name = entry.file_name();
                if matches!(name.to_bytes(), b"." | b"..") {
                    continue;
                }
                if usage.entries >= MAX_ENTRIES {
                    usage.truncated = true;
                    break 'roots;
                }
                let Ok(stat) = fstatat(&*dir, name, AtFlags::AT_SYMLINK_NOFOLLOW) else {
                    continue;
                };
                usage.entries += 1;
                let is_dir =
                    SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR;
                let below = below.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
                let mut group = group;
                // Two levels below the root, or a file directly in it.
                if group.is_none() && (depth == 1 || (depth == 0 && !is_dir)) {
                    names.push(below.to_string_lossy().into_owned());
                    group = Some(names.len() - 1);
                }
                if stat.st_nlink <= 1 || seen.insert((stat.st_dev, stat.st_ino)) {
                    let bytes = blocks_bytes(stat.st_blocks);
                    usage.bytes = usage.bytes.saturating_add(bytes);
                    if let Some(group) = group {
                        let total = groups.entry(group).or_default();
                        *total = total.saturating_add(bytes);
                    }
                }
                if is_dir && stat.st_dev == device {
                    if depth + 1 >= MAX_DEPTH {
                        usage.truncated = true;
                        continue;
                    }
                    let child = Pending::Child(Rc::clone(&dir), name.to_owned());
                    stack.push((child, below, depth + 1, group));
                }
            }
        }
    }
    let mut largest: Vec<(String, u64)> = groups
        .into_iter()
        .map(|(group, bytes)| (std::mem::take(&mut names[group]), bytes))
        .collect();
    largest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    largest.truncate(LARGEST);
    usage.largest = largest;
    usage
}

/// A folder still to list.
enum Pending {
    /// Its own handle (a root).
    Open(OwnedFd),
    /// A name in its parent's handle.
    Child(Rc<OwnedFd>, std::ffi::CString),
}

/// Bytes of 512-byte blocks.
fn blocks_bytes(blocks: i64) -> u64 {
    u64::try_from(blocks).unwrap_or(0).saturating_mul(512)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_count_once_and_links_are_not_followed() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("a/b"))?;
        std::fs::write(dir.path().join("a/b/big"), vec![7u8; 256 * 1024])?;
        std::fs::hard_link(dir.path().join("a/b/big"), dir.path().join("a/again"))?;
        std::os::unix::fs::symlink("/", dir.path().join("a/root"))?;
        let usage = measure(&[(dir.path(), None), (&dir.path().join("missing"), None)]);
        assert!(usage.bytes >= 256 * 1024, "{usage:?}");
        assert!(
            usage.bytes < 2 * 256 * 1024,
            "hard link counted twice: {usage:?}"
        );
        assert!(!usage.truncated);
        assert!(usage.entries >= 5);
        Ok(())
    }

    /// ADR-0315: a root that is a link is not measured, and the entries of a folder are counted
    /// from its handle: what a link in the tree points at is never visited.
    #[test]
    fn a_linked_root_or_folder_is_not_walked() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(outside.join("deep"))?;
        std::fs::write(outside.join("deep/big"), vec![7u8; 512 * 1024])?;
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("worktrees"))?;
        std::os::unix::fs::symlink(&outside, tree.join("worktrees/t1"))?;
        let usage = measure(&[(&tree, None)]);
        assert!(usage.bytes < 64 * 1024, "{usage:?}");
        assert_eq!(usage.entries, 3, "the tree, worktrees and the link");

        let linked_root = dir.path().join("linked-root");
        std::os::unix::fs::symlink(&outside, &linked_root)?;
        assert_eq!(measure(&[(&linked_root, Some("system"))]), Usage::default());
        Ok(())
    }

    /// ADR-0315: the walk holds one handle per level, so folders nested deeper than
    /// [`MAX_DEPTH`] are counted and not listed, and the measurement says it is a lower bound.
    #[test]
    fn folders_nested_deeper_than_the_bound_are_not_listed()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        // Two folders per level, so each level's handle waits for its sibling.
        let mut at = dir.path().to_path_buf();
        for level in 0..MAX_DEPTH + 20 {
            std::fs::create_dir(at.join("side"))?;
            at.push("down");
            std::fs::create_dir(&at)?;
            if level == MAX_DEPTH + 10 {
                std::fs::write(at.join("deep"), vec![7u8; 256 * 1024])?;
            }
        }
        let usage = measure(&[(dir.path(), None)]);
        assert!(usage.truncated, "{usage:?}");
        assert!(
            usage.bytes < 256 * 1024,
            "the file below the bound is not counted"
        );
        let entries = u64::try_from(2 * MAX_DEPTH).unwrap_or(0);
        assert!(usage.entries <= entries + 1, "{usage:?}");
        Ok(())
    }

    /// SPEC-059 DSK-202: a measurement keeps the largest directories two levels down (a task's
    /// worktree, the CI checkouts) and a labelled root as one, largest first.
    #[test]
    fn the_largest_consumers_are_kept() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let system = tempfile::tempdir()?;
        let write = |path: &Path, kib: usize| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap_or(path))?;
            std::fs::write(path, vec![1u8; kib * 1024])
        };
        write(&dir.path().join("worktrees/t-big/target/debug/a"), 512)?;
        write(&dir.path().join("worktrees/t-big/src/lib.rs"), 64)?;
        write(&dir.path().join("worktrees/t-small/target/b"), 128)?;
        write(&dir.path().join("ci/jobs/quality/target/c"), 256)?;
        write(&dir.path().join("ci/jobs/e2e/node_modules/d"), 32)?;
        write(&dir.path().join("loose"), 16)?;
        write(&system.path().join("usr/lib/e"), 384)?;
        for n in 0..LARGEST {
            write(&dir.path().join(format!("home/.c{n}/f")), 4)?;
        }
        let usage = measure(&[(dir.path(), None), (system.path(), Some("system"))]);
        let names: Vec<&str> = usage
            .largest
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "worktrees/t-big",
                "system",
                "ci/jobs",
                "worktrees/t-small",
                "loose"
            ],
            "{usage:?}"
        );
        assert!(usage.largest[0].1 >= 576 * 1024, "{usage:?}");
        let total: u64 = usage.largest.iter().map(|(_, bytes)| bytes).sum();
        assert!(total <= usage.bytes);
        Ok(())
    }
}
