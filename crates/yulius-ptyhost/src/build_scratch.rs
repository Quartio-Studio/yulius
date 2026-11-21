//! The host records scratch ownership outside the writable sandbox before launching the child.
//! A server that reaps a killed host or reconciles a stopped attempt can finish its cleanup.
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::{atomic_write, read_record};

const RECEIPT: &str = "build-scratch.json";

#[derive(PartialEq, Eq, Serialize, Deserialize)]
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

/// Keep the identity in the host even if CI removes its run directory after
/// receiving the exit frame. A normal return and an early error use the same
/// check as recovery after the host dies.
pub(crate) struct Guard<'a> {
    run_dir: &'a Path,
    receipt: Receipt,
    released: bool,
}

impl Guard<'_> {
    pub(crate) fn path(&self) -> &Path {
        &self.receipt.path
    }

    pub(crate) fn close(mut self) -> io::Result<()> {
        self.released = true;
        remove(self.run_dir, &self.receipt)
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if !self.released
            && let Err(error) = remove(self.run_dir, &self.receipt)
        {
            eprintln!("cannot reclaim host build scratch: {error}");
        }
    }
}

pub(crate) fn own(run_dir: &Path, dir: tempfile::TempDir) -> io::Result<Guard<'_>> {
    let path = dir.keep();
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() {
        return Err(io::Error::other("expected a private build directory"));
    }
    let guard = Guard {
        run_dir,
        receipt: Receipt {
            path,
            host_pid: std::process::id(),
            boot_id: crate::current_boot_id(),
            device: meta.dev(),
            inode: meta.ino(),
        },
        released: false,
    };
    atomic_write(
        run_dir,
        RECEIPT,
        &serde_json::to_vec(&guard.receipt)?,
        0o600,
    )?;
    Ok(guard)
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
    // A removed CI run directory may already have a new owner at its name.
    if read(run_dir)?.as_ref() == Some(receipt) {
        ignore_missing(fs::remove_file(run_dir.join(RECEIPT)))?;
    }
    Ok(())
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
        let guard = own(run.path(), tempfile::tempdir_in(sandbox.path())?)?;
        let owned = guard.path();
        fs::write(owned.join("artifact"), b"build")?;
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
    fn an_old_host_preserves_the_receipt_in_a_reused_run_directory() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let old = own(run.path(), tempfile::tempdir_in(sandbox.path())?)?;
        fs::remove_dir_all(run.path())?;
        fs::create_dir(run.path())?;
        let current = own(run.path(), tempfile::tempdir_in(sandbox.path())?)?;
        old.close()?;
        assert!(read(run.path())?.is_some_and(|receipt| receipt.path == current.path()));
        assert!(current.path().is_dir());
        Ok(())
    }

    #[test]
    fn cleanup_retains_a_replacement_at_the_old_scratch_name() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let guard = own(run.path(), tempfile::tempdir_in(sandbox.path())?)?;
        let owned = guard.path();
        fs::rename(owned, sandbox.path().join("moved"))?;
        fs::create_dir(owned)?;
        fs::write(owned.join("other"), b"live")?;
        assert!(reclaim(run.path()).is_err());
        assert!(owned.join("other").is_file());
        Ok(())
    }
}
