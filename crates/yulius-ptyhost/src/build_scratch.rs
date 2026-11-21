//! The host records scratch ownership outside the writable sandbox before launching the child.
//! A server that reaps a killed host or reconciles a stopped attempt can finish its cleanup.
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::{atomic_write, read_record};

const RECEIPT: &str = "build-scratch.json";

#[derive(Serialize, Deserialize)]
struct Receipt {
    path: PathBuf,
    host_pid: u32,
    boot_id: Option<String>,
    device: u64,
    inode: u64,
}

pub(crate) fn recorded(run_dir: &Path) -> bool {
    run_dir.join(RECEIPT).is_file()
}

pub(crate) fn record(run_dir: &Path, owned: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(owned)?;
    let receipt = Receipt {
        path: owned.to_owned(),
        host_pid: std::process::id(),
        boot_id: crate::current_boot_id(),
        device: meta.dev(),
        inode: meta.ino(),
    };
    atomic_write(run_dir, RECEIPT, &serde_json::to_vec(&receipt)?, 0o600)
}

/// Only after the recorded attempt has stopped. Concurrent cleanup is idempotent.
/// Check the directory identity before removing anything at the recorded name.
pub(crate) fn reclaim(run_dir: &Path) -> io::Result<()> {
    if let Some(receipt) = read(run_dir)? {
        remove(run_dir, &receipt)?;
    }
    Ok(())
}

/// A host can be killed before writing status.json. Retain the directory if
/// its owner might still be starting. Bubblewrap exits with a dead host.
pub(crate) fn reclaim_unrecorded(run_dir: &Path) -> io::Result<()> {
    if let Some(receipt) = read(run_dir)?
        && let (Some(start_boot), Some(now_boot)) = (&receipt.boot_id, crate::current_boot_id())
        && (start_boot != &now_boot
            || !crate::process::pid(receipt.host_pid).is_some_and(crate::process::alive))
    {
        remove(run_dir, &receipt)?;
    }
    Ok(())
}

fn read(run_dir: &Path) -> io::Result<Option<Receipt>> {
    read_record(&run_dir.join(RECEIPT)).map_err(io::Error::other)
}

fn remove(run_dir: &Path, receipt: &Receipt) -> io::Result<()> {
    match fs::symlink_metadata(&receipt.path) {
        Ok(meta)
            if meta.is_dir() && (meta.dev(), meta.ino()) == (receipt.device, receipt.inode) =>
        {
            ignore_missing(fs::remove_dir_all(&receipt.path))?;
        }
        Ok(_) => return Err(io::Error::other("build scratch directory identity changed")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    ignore_missing(fs::remove_file(run_dir.join(RECEIPT)))
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    result.or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_status_keeps_a_starting_host_and_reclaims_scratch_from_a_previous_boot()
    -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let owned = tempfile::tempdir_in(sandbox.path())?.keep();
        fs::write(owned.join("artifact"), b"build")?;
        record(run.path(), &owned)?;
        assert_eq!(
            crate::inspect_run_dir(run.path()),
            crate::RunDirState::Missing
        );
        assert!(owned.join("artifact").exists());
        let mut receipt = read(run.path())?.ok_or_else(|| io::Error::other("missing receipt"))?;
        receipt.boot_id = Some("previous boot".into());
        atomic_write(run.path(), RECEIPT, &serde_json::to_vec(&receipt)?, 0o600)?;
        assert_eq!(
            crate::inspect_run_dir(run.path()),
            crate::RunDirState::Missing
        );
        assert!(!owned.exists());
        assert!(!recorded(run.path()));
        Ok(())
    }

    #[test]
    fn cleanup_retains_a_replacement_at_the_old_scratch_name() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let owned = tempfile::tempdir_in(sandbox.path())?.keep();
        record(run.path(), &owned)?;
        fs::rename(&owned, sandbox.path().join("moved"))?;
        fs::create_dir(&owned)?;
        fs::write(owned.join("other"), b"live")?;
        assert!(reclaim(run.path()).is_err());
        assert!(owned.join("other").is_file());
        Ok(())
    }
}
