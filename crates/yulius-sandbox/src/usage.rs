//! Disk usage of a sandbox for its soft quota (SPEC-002 ADR-22 §5): allocated blocks of every
//! file under the given roots, without following links or crossing filesystems, bounded in
//! entries.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// Entries visited at most per measurement.
pub const MAX_ENTRIES: u64 = 4_000_000;
/// Largest consumers kept per measurement (SPEC-059 DSK-202).
pub const LARGEST: usize = 5;

/// A measurement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    /// Bytes allocated (512-byte blocks × 512), hard links counted once.
    pub bytes: u64,
    /// Entries visited.
    pub entries: u64,
    /// The bound was reached; `bytes` is a lower bound.
    pub truncated: bool,
    /// The [`LARGEST`] largest consumers, largest first: a labelled root as one, else the
    /// directories two levels below an unlabelled root (`worktrees/<task>`, `ci/jobs`) and its
    /// own top-level files.
    pub largest: Vec<(String, u64)>,
}

/// Measures `roots` (missing roots count as empty); a root with a label is one consumer, one
/// without is split two levels down.
#[must_use]
pub fn measure(roots: &[(&Path, Option<&str>)]) -> Usage {
    let mut usage = Usage::default();
    let mut seen = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    let mut groups: HashMap<usize, u64> = HashMap::new();
    'roots: for (root, label) in roots {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        let device = meta.dev();
        let top = label.map(|label| {
            names.push(label.to_owned());
            names.len() - 1
        });
        // (path, depth below the root, its consumer once known)
        let mut stack: Vec<(PathBuf, usize, Option<usize>)> = vec![(root.to_path_buf(), 0, top)];
        while let Some((path, depth, mut group)) = stack.pop() {
            if usage.entries >= MAX_ENTRIES {
                usage.truncated = true;
                break 'roots;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            usage.entries += 1;
            if group.is_none() && (depth == 2 || (depth == 1 && !meta.is_dir())) {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                names.push(relative.to_string_lossy().into_owned());
                group = Some(names.len() - 1);
            }
            if meta.nlink() <= 1 || seen.insert((meta.dev(), meta.ino())) {
                let bytes = meta.blocks().saturating_mul(512);
                usage.bytes = usage.bytes.saturating_add(bytes);
                if let Some(group) = group {
                    let total = groups.entry(group).or_default();
                    *total = total.saturating_add(bytes);
                }
            }
            if meta.is_dir()
                && meta.dev() == device
                && let Ok(entries) = std::fs::read_dir(&path)
            {
                stack.extend(entries.flatten().map(|e| (e.path(), depth + 1, group)));
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
