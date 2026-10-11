//! Disk usage of a sandbox for its soft quota (SPEC-002 ADR-19 §5): allocated blocks of every
//! file under the given roots, without following links or crossing filesystems, bounded in
//! entries.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// Entries visited at most per measurement.
pub const MAX_ENTRIES: u64 = 4_000_000;

/// A measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Bytes allocated (512-byte blocks × 512), hard links counted once.
    pub bytes: u64,
    /// Entries visited.
    pub entries: u64,
    /// The bound was reached; `bytes` is a lower bound.
    pub truncated: bool,
}

/// Measures `roots` (missing roots count as empty).
#[must_use]
pub fn measure(roots: &[&Path]) -> Usage {
    let mut usage = Usage::default();
    let mut seen = HashSet::new();
    for root in roots {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        let device = meta.dev();
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            if usage.entries >= MAX_ENTRIES {
                usage.truncated = true;
                return usage;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            usage.entries += 1;
            if meta.nlink() <= 1 || seen.insert((meta.dev(), meta.ino())) {
                usage.bytes = usage
                    .bytes
                    .saturating_add(meta.blocks().saturating_mul(512));
            }
            if meta.is_dir()
                && meta.dev() == device
                && let Ok(entries) = std::fs::read_dir(&path)
            {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
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
        let usage = measure(&[dir.path(), &dir.path().join("missing")]);
        assert!(usage.bytes >= 256 * 1024, "{usage:?}");
        assert!(
            usage.bytes < 2 * 256 * 1024,
            "hard link counted twice: {usage:?}"
        );
        assert!(!usage.truncated);
        assert!(usage.entries >= 5);
        Ok(())
    }
}
