//! Private run records identify disk build trees after a host is killed. Receipts
//! stay with the run's metadata. A delayed cleanup never removes a receipt.
use std::ffi::CString;
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::dir::Dir;
use nix::errno::Errno;
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use serde::{Deserialize, Serialize};
use yulius_contracts::ptyhost::StatusRecord;

use crate::files::{atomic_write_at, read_record};

const RECEIPT: &str = "build-scratch.json";
const OWNER: &str = ".yulius-build-owner";
const DIRECTORY: OFlag = OFlag::O_DIRECTORY
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

#[derive(Serialize, Deserialize)]
pub(crate) struct Receipt {
    path: PathBuf,
    host_pid: u32,
    token_sha256: String,
    boot_id: Option<String>,
    device: u64,
    inode: u64,
}

impl Receipt {
    pub(crate) fn belongs_to(&self, pid: u32, token: &str) -> bool {
        self.host_pid == pid && self.token_sha256 == token
    }

    pub(crate) fn matches_status(&self, status: &StatusRecord) -> bool {
        self.belongs_to(status.ptyhost_pid, &status.token_sha256)
            && self.boot_id.as_deref() == Some(status.boot_id.as_str())
    }

    pub(crate) fn reclaim(&self) -> io::Result<()> {
        if let Some(tree) = OpenedTree::open(self)? {
            tree.clear(self)?;
        }
        Ok(())
    }
}

pub(crate) fn recorded(run_dir: &Path) -> bool {
    run_dir.join(RECEIPT).is_file()
}

pub(crate) fn read(run_dir: &Path) -> io::Result<Option<Receipt>> {
    read_record(&run_dir.join(RECEIPT)).map_err(io::Error::other)
}

/// Retain ownership when CI removes its run record after the exit frame.
pub(crate) struct Guard {
    receipt: Receipt,
    tree: OpenedTree,
    released: bool,
}

impl Guard {
    pub(crate) fn path(&self) -> &Path {
        &self.receipt.path
    }

    /// Commands may clear or recreate everything under the advertised directory.
    pub(crate) fn payload(&self) -> PathBuf {
        self.path().join("payload")
    }

    pub(crate) fn close(mut self) -> io::Result<()> {
        self.released = true;
        self.tree.clear(&self.receipt)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.released
            && let Err(error) = self.tree.clear(&self.receipt)
        {
            eprintln!("cannot reclaim host build scratch: {error}");
        }
    }
}

pub(crate) fn own(
    run_dir: &File,
    dir: tempfile::TempDir,
    pid: u32,
    token: &str,
) -> io::Result<Guard> {
    let path = dir.keep();
    let tree = OpenedTree::at(&path)?
        .ok_or_else(|| io::Error::other("scratch disappeared during allocation"))?;
    let meta = tree.tree.metadata()?;
    let guard = Guard {
        receipt: Receipt {
            path,
            host_pid: pid,
            token_sha256: token.into(),
            boot_id: crate::current_boot_id(),
            device: meta.dev(),
            inode: meta.ino(),
        },
        tree,
        released: false,
    };
    // Pin the launch to this tree even if a released inode is later reused.
    // A current launch always has this entry before the child sees its root.
    let mut marker = File::from(openat(
        &guard.tree.tree,
        OWNER,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    marker.write_all(token.as_bytes())?;
    marker.sync_all()?;
    mkdirat(&guard.tree.tree, "payload", Mode::S_IRWXU)?;
    guard.tree.tree.sync_all()?;
    guard.tree.parent.sync_all()?;
    atomic_write_at(
        run_dir,
        RECEIPT,
        &serde_json::to_vec(&guard.receipt)?,
        0o600,
    )?;
    Ok(guard)
}

pub(crate) fn reclaim_recorded(run_dir: &Path, status: &StatusRecord) -> io::Result<()> {
    if let Some(receipt) = read(run_dir)?
        && receipt.matches_status(status)
    {
        receipt.reclaim()?;
    }
    Ok(())
}

/// A host may die before publishing status.json. A possibly live owner retains its tree.
pub(crate) fn reclaim_unrecorded(run_dir: &Path) -> io::Result<()> {
    if let Some(receipt) = read(run_dir)?
        && let (Some(start_boot), Some(now_boot)) = (&receipt.boot_id, crate::current_boot_id())
        && (start_boot != &now_boot
            || !crate::process::pid(receipt.host_pid).is_some_and(crate::process::alive))
    {
        receipt.reclaim()?;
    }
    Ok(())
}

pub(crate) struct OpenedTree {
    parent: File,
    tree: File,
}

impl OpenedTree {
    fn at(path: &Path) -> io::Result<Option<Self>> {
        let parent = File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("missing scratch parent"))?,
        )?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("missing scratch name"))?;
        let tree = match openat(&parent, name, DIRECTORY, Mode::empty()) {
            Ok(fd) => File::from(fd),
            Err(Errno::ENOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(Self { parent, tree }))
    }

    pub(crate) fn open(receipt: &Receipt) -> io::Result<Option<Self>> {
        let Some(opened) = Self::at(&receipt.path)? else {
            return Ok(None);
        };
        let tree = &opened.tree;
        let meta = tree.metadata()?;
        if (meta.dev(), meta.ino()) != (receipt.device, receipt.inode) {
            return Err(io::Error::other("build scratch directory identity changed"));
        }
        let marker = match openat(
            tree,
            OWNER,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            // Other reapers remove the launch marker only after clearing the payload.
            Err(Errno::ENOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut owner = Vec::new();
        if !marker.metadata()?.is_file() {
            return Err(io::Error::other("invalid build scratch owner marker"));
        }
        marker.take(65).read_to_end(&mut owner)?;
        if owner != receipt.token_sha256.as_bytes() {
            return Err(io::Error::other("build scratch launch identity changed"));
        }
        Ok(Some(opened))
    }

    pub(crate) fn clear(&self, receipt: &Receipt) -> io::Result<()> {
        // Even if the name is exchanged after open(), all recursive deletion
        // is relative to the verified original directory.
        if self.tree.metadata()?.nlink() == 0 {
            return Ok(());
        }
        clear_contents(&self.tree)?;
        missing_ok(unlinkat(&self.tree, OWNER, UnlinkatFlags::NoRemoveDir))?;
        let name = receipt
            .path
            .file_name()
            .ok_or_else(|| io::Error::other("missing scratch name"))?;
        match openat(&self.parent, name, DIRECTORY, Mode::empty()) {
            Ok(fd) => {
                let meta = File::from(fd).metadata()?;
                if (meta.dev(), meta.ino()) == (receipt.device, receipt.inode) {
                    // Only remove an empty directory entry. Never recurse through its name.
                    missing_ok(unlinkat(&self.parent, name, UnlinkatFlags::RemoveDir))?;
                }
            }
            Err(Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

/// Stream each directory relative to its opened parent; workspace sizes and
/// nesting do not dictate Rust call-stack or per-directory listing sizes.
fn clear_contents(root: &File) -> io::Result<()> {
    struct Level {
        folder: File,
        entries: nix::dir::OwningIter,
        name: Option<CString>,
    }
    let level = |folder: File, name| -> io::Result<Level> {
        let entries = Dir::openat(&folder, ".", DIRECTORY, Mode::empty())?.into_iter();
        Ok(Level {
            folder,
            entries,
            name,
        })
    };
    let mut stack = vec![level(root.try_clone()?, None)?];
    while let Some(current) = stack.last_mut() {
        let Some(entry) = current.entries.next() else {
            let name = current.name.take();
            stack.pop();
            if let (Some(parent), Some(name)) = (stack.last(), name) {
                missing_ok(unlinkat(
                    &parent.folder,
                    name.as_c_str(),
                    UnlinkatFlags::RemoveDir,
                ))?;
            }
            continue;
        };
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..")
            || (current.name.is_none() && name.to_bytes() == OWNER.as_bytes())
        {
            continue;
        }
        match openat(&current.folder, name, DIRECTORY, Mode::empty()) {
            Ok(child) => stack.push(level(File::from(child), Some(name.to_owned()))?),
            Err(Errno::ENOTDIR | Errno::ELOOP) => {
                missing_ok(unlinkat(&current.folder, name, UnlinkatFlags::NoRemoveDir))?;
            }
            Err(Errno::ENOENT) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn missing_ok(result: nix::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) | Err(Errno::ENOENT) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::atomic_write;

    #[test]
    fn a_missing_status_keeps_a_starting_host_and_reclaims_scratch_from_a_previous_boot()
    -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let guard = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            std::process::id(),
            "new-token",
        )?;
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
        assert!(recorded(run.path()), "receipt stays with the run metadata");
        Ok(())
    }

    #[test]
    fn an_old_host_preserves_the_receipt_in_a_reused_run_directory() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let old = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            std::process::id(),
            "new-token",
        )?;
        fs::remove_dir_all(run.path())?;
        fs::create_dir(run.path())?;
        let current = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            std::process::id(),
            "new-token",
        )?;
        old.close()?;
        assert!(read(run.path())?.is_some_and(|receipt| receipt.path == current.path()));
        assert!(current.path().is_dir());
        Ok(())
    }

    #[test]
    fn exchanging_the_path_during_cleanup_keeps_the_new_tree() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let original = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            std::process::id(),
            "new-token",
        )?;
        fs::write(original.path().join("old"), b"finished")?;
        let moved = sandbox.path().join("moved");
        let tree = OpenedTree::open(&original.receipt)?
            .ok_or_else(|| io::Error::other("scratch absent"))?;
        fs::rename(original.path(), &moved)?;
        fs::create_dir(original.path())?;
        fs::write(original.path().join("new"), b"live")?;
        tree.clear(&original.receipt)?;
        assert!(
            original.path().join("new").exists(),
            "replacement contents were deleted"
        );
        assert!(!moved.join("old").exists(), "original build survived");
        Ok(())
    }

    #[test]
    fn the_reaped_hosts_delayed_callback_leaves_a_new_attempt_running()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::process::CommandExt;
        use yulius_contracts::ptyhost::{RECORD_VERSION, STATUS_FILE, StatusRecord};
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let mut host = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()?;
        let live = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            host.id(),
            "new-token",
        )?;
        fs::write(live.path().join("target"), b"live")?;
        crate::reconcile::reclaim_after_host_exit(run.path(), host.id(), "old-token")?;
        let retained_before_status = live.path().join("target").exists();
        let status = StatusRecord {
            version: RECORD_VERSION,
            attempt_id: "new".into(),
            ptyhost_pid: host.id(),
            child_pid: 0,
            started_at_ms: crate::files::now_ms(),
            boot_id: crate::current_boot_id().ok_or("boot id")?,
            token_sha256: "new-token".into(),
        };
        atomic_write(
            run.path(),
            STATUS_FILE,
            &serde_json::to_vec(&status)?,
            0o600,
        )?;
        crate::reconcile::reclaim_after_host_exit(run.path(), host.id(), "old-token")?;
        let alive = host.try_wait()?.is_none();
        if alive {
            host.kill()?;
        }
        host.wait()?;
        assert!(
            retained_before_status,
            "an older waiter removed a starting attempt's scratch"
        );
        assert!(alive, "an older waiter terminated the newer host");
        Ok(())
    }

    #[test]
    fn ownership_captured_at_ready_outlives_the_ci_control_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let pid = std::process::id();
        let old = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            pid,
            "first",
        )?;
        fs::create_dir(old.path().join("target"))?;
        fs::write(old.path().join("target/build"), b"old")?;
        let observed = crate::reconcile::BuildOwner::capture(run.path(), pid, "first", None)?
            .ok_or("missing original ownership")?;
        fs::remove_dir_all(run.path())?;
        fs::create_dir(run.path())?;
        let new = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            pid,
            "second",
        )?;
        fs::write(new.path().join("target"), b"new")?;
        observed.reclaim()?;
        assert!(!old.path().exists());
        assert!(new.path().join("target").is_file());
        assert!(read(run.path())?.is_some_and(|receipt| receipt.belongs_to(pid, "second")));
        Ok(())
    }

    #[test]
    fn late_registration_writes_only_the_opened_control_directory() -> io::Result<()> {
        let base = tempfile::tempdir()?;
        let run = base.path().join("control");
        fs::create_dir(&run)?;
        let first = File::open(&run)?;
        let moved = base.path().join("previous");
        fs::rename(&run, &moved)?;
        fs::create_dir(&run)?;
        let new = own(
            &File::open(&run)?,
            tempfile::tempdir_in(base.path())?,
            42,
            "new",
        )?;
        let old = own(&first, tempfile::tempdir_in(base.path())?, 51, "old")?;
        assert!(read(&moved)?.is_some_and(|r| r.path == old.path()));
        assert!(read(&run)?.is_some_and(|r| r.path == new.path()));
        Ok(())
    }

    #[test]
    fn cleanup_retains_a_replacement_at_the_old_scratch_name() -> io::Result<()> {
        let run = tempfile::tempdir()?;
        let sandbox = tempfile::tempdir()?;
        let guard = own(
            &File::open(run.path())?,
            tempfile::tempdir_in(sandbox.path())?,
            std::process::id(),
            "new-token",
        )?;
        let owned = guard.path();
        fs::rename(owned, sandbox.path().join("moved"))?;
        fs::create_dir(owned)?;
        fs::write(owned.join("other"), b"live")?;
        assert!(guard.receipt.reclaim().is_err());
        assert!(owned.join("other").is_file());
        Ok(())
    }
}
