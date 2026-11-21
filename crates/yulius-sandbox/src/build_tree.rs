//! Descriptor-relative reclamation of disposable command and session build trees.
use std::ffi::{CString, OsString};
use std::fs::File;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use nix::dir::Dir;
use nix::errno::Errno;
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::Mode;
use nix::unistd::{UnlinkatFlags, unlinkat};

const DIRECTORY: OFlag = OFlag::O_DIRECTORY
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// File registering the launch that owns a session-level build tree.
pub const OWNER_MARKER: &str = ".yulius-build-owner";

/// An owned build directory and its parent, pinned across renames.
pub struct BuildTree {
    name: OsString,
    parent: File,
    tree: File,
}

impl BuildTree {
    /// Opens a build directory without following a final symlink.
    ///
    /// # Errors
    /// Returns an I/O error when the parent or directory cannot be opened.
    pub fn open(path: &Path) -> io::Result<Option<Self>> {
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
        Ok(Some(Self {
            parent,
            tree,
            name: name.into(),
        }))
    }

    /// Handle of the owned directory, for registration and identity checks.
    pub fn directory(&self) -> &File {
        &self.tree
    }

    /// Makes the new directory entry durable before registering ownership.
    ///
    /// # Errors
    /// Returns the parent directory fsync error.
    pub fn sync_parent(&self) -> io::Result<()> {
        self.parent.sync_all()
    }

    /// Reclaims this command-owned tree.
    ///
    /// # Errors
    /// Returns filesystem errors when the tree could not be reclaimed.
    pub fn clear(&self) -> io::Result<()> {
        self.remove(None)
    }

    /// Reclaims a session tree, removing its registered ownership marker last.
    ///
    /// # Errors
    /// Returns filesystem errors when the tree could not be reclaimed.
    pub fn clear_registered(&self) -> io::Result<()> {
        self.remove(Some(OWNER_MARKER))
    }

    fn remove(&self, owner_marker: Option<&str>) -> io::Result<()> {
        // Even if the name is exchanged after open(), all recursive deletion
        // is relative to the verified original directory.
        let owned = self.tree.metadata()?;
        if owned.nlink() == 0 {
            return Ok(());
        }
        clear_contents(&self.tree, owner_marker)?;
        if let Some(marker) = owner_marker {
            missing_ok(unlinkat(&self.tree, marker, UnlinkatFlags::NoRemoveDir))?;
        }
        let name = self.name.as_os_str();
        match openat(&self.parent, name, DIRECTORY, Mode::empty()) {
            Ok(fd) => {
                let meta = File::from(fd).metadata()?;
                if (meta.dev(), meta.ino()) == (owned.dev(), owned.ino()) {
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
fn clear_contents(root: &File, owner_marker: Option<&str>) -> io::Result<()> {
    struct Level {
        folder: File,
        entries: nix::dir::OwningIter,
        name: Option<CString>,
    }
    let level = |folder: File, name| -> io::Result<Level> {
        // Dependency managers commonly make completed packages read-only.
        if folder.metadata()?.mode() & 0o700 != 0o700 {
            folder.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        }
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
            || (current.name.is_none()
                && owner_marker.is_some_and(|marker| marker.as_bytes() == name.to_bytes()))
        {
            continue;
        }
        match open_for_cleanup(&current.folder, name) {
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

fn open_for_cleanup(parent: &File, name: &std::ffi::CStr) -> nix::Result<std::os::fd::OwnedFd> {
    let opened = openat(parent, name, DIRECTORY, Mode::empty());
    #[cfg(target_os = "linux")]
    if matches!(opened, Err(Errno::EACCES)) {
        use std::os::fd::AsRawFd;
        // O_PATH pins even an unreadable owned directory. chmod through its
        // procfs descriptor, then reopen that same directory for enumeration.
        let handle = openat(parent, name, DIRECTORY | OFlag::O_PATH, Mode::empty())?;
        let pinned = format!("/proc/self/fd/{}", handle.as_raw_fd());
        nix::sys::stat::fchmodat(
            nix::fcntl::AT_FDCWD,
            pinned.as_str(),
            Mode::S_IRWXU,
            nix::sys::stat::FchmodatFlags::FollowSymlink,
        )?;
        return openat(&handle, ".", DIRECTORY, Mode::empty());
    }
    #[cfg(target_os = "macos")]
    if matches!(opened, Err(Errno::EACCES)) {
        use nix::fcntl::AtFlags;
        use nix::sys::stat::{FchmodatFlags, SFlag, fchmodat, fstat, fstatat};
        // Darwin has no O_PATH. Resolve just this entry under the pinned
        // parent, without following a symlink on either chmod or open.
        let original = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
        if SFlag::from_bits_truncate(original.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR {
            return Err(Errno::ENOTDIR);
        }
        fchmodat(parent, name, Mode::S_IRWXU, FchmodatFlags::NoFollowSymlink)?;
        let directory = openat(parent, name, DIRECTORY, Mode::empty())?;
        let found = fstat(&directory)?;
        if (original.st_dev, original.st_ino) != (found.st_dev, found.st_ino) {
            return Err(Errno::ESTALE);
        }
        return Ok(directory);
    }
    opened
}

fn missing_ok(result: nix::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) | Err(Errno::ENOENT) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
