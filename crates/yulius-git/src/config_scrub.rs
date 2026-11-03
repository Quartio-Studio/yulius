//! The configuration scrub of [`Git::scrub_transport_config`] through no-follow handles
//! (SPEC-402, ADR-0257).
//!
//! The clone's `.git` is the agents' to change at any moment, so nothing below the repository is
//! reached by path once it is checked: `.git` and its configuration files are opened with
//! `O_NOFOLLOW`, Git's lock (`<file>.lock`) is taken in the `.git` handle, Git edits a private copy,
//! and the scrubbed bytes are renamed over the name through the handle. A name swapped for a link
//! meanwhile has only the link replaced.

use std::path::Path;

use crate::Git;
use crate::error::GitError;

/// The configuration files a scrub covers, in `.git`.
#[cfg(unix)]
const FILES: [&str; 2] = ["config", "config.worktree"];

/// A configuration file larger than this is refused rather than read.
#[cfg(unix)]
const MAX_CONFIG_BYTES: u64 = 4 << 20;

/// Removes from `repo`'s `.git/config` and `.git/config.worktree` every key `removed` answers
/// for.
#[cfg(unix)]
pub(crate) fn scrub(
    git: &Git,
    repo: &Path,
    removed: &dyn Fn(&str) -> bool,
) -> Result<(), GitError> {
    let git_dir = unix::open_git_dir(repo)?;
    // Every file is checked before the first one is written, so a refusal changes nothing.
    for name in FILES {
        unix::check_kind(&git_dir, name)?;
    }
    let private = unix::PrivateDir::new()?;
    for name in FILES {
        unix::scrub_file(git, &git_dir, name, private.path(), removed)?;
    }
    Ok(())
}

/// Off Unix there are no no-follow handles to scrub through, and no contained agents.
#[cfg(not(unix))]
pub(crate) fn scrub(
    _git: &Git,
    _repo: &Path,
    _removed: &dyn Fn(&str) -> bool,
) -> Result<(), GitError> {
    Err(GitError::InvalidInput {
        what: "repository configuration",
        reason: "the scrub needs a Unix host".to_owned(),
    })
}

#[cfg(unix)]
mod unix {
    use std::io::{Read as _, Write as _};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::DirBuilderExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use nix::errno::Errno;
    use nix::fcntl::{AtFlags, OFlag, open, openat, renameat};
    use nix::sys::stat::{Mode, SFlag, fchmod, fstat, fstatat};
    use nix::unistd::{UnlinkatFlags, unlinkat};

    use super::MAX_CONFIG_BYTES;
    use crate::Git;
    use crate::error::GitError;
    use crate::runner::Invocation;

    const DIR_FLAGS: OFlag = OFlag::O_RDONLY
        .union(OFlag::O_DIRECTORY)
        .union(OFlag::O_NOFOLLOW)
        .union(OFlag::O_CLOEXEC);

    fn refused(reason: &str) -> GitError {
        GitError::InvalidInput {
            what: "repository configuration",
            reason: reason.to_owned(),
        }
    }

    fn io(context: &'static str, err: impl Into<std::io::Error>) -> GitError {
        GitError::Io {
            context,
            source: err.into(),
        }
    }

    /// `repo/.git`, each of the two opened without following a link. Yulius makes every clone
    /// with a real `.git` directory; a pointer file or a link would send the scrub to
    /// configuration elsewhere while Git still reads another.
    pub(super) fn open_git_dir(repo: &Path) -> Result<OwnedFd, GitError> {
        let repo = match open(repo, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ELOOP | Errno::ENOTDIR) => {
                return Err(GitError::InvalidInput {
                    what: "repository",
                    reason: "it is not a directory".to_owned(),
                });
            }
            Err(err) => return Err(io("opening the repository", err)),
        };
        match openat(&repo, ".git", DIR_FLAGS, Mode::empty()) {
            Ok(fd) => Ok(fd),
            Err(Errno::ELOOP | Errno::ENOTDIR | Errno::ENOENT) => Err(GitError::InvalidInput {
                what: "repository",
                reason: "its .git is not a directory".to_owned(),
            }),
            Err(err) => Err(io("opening the repository's .git", err)),
        }
    }

    /// Refuses `name` in `git_dir` unless it is missing or a regular file.
    pub(super) fn check_kind(git_dir: &OwnedFd, name: &str) -> Result<(), GitError> {
        match fstatat(git_dir, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => regular(stat.st_mode),
            Err(Errno::ENOENT) => Ok(()),
            Err(err) => Err(io("checking the repository configuration", err)),
        }
    }

    fn regular(mode: nix::sys::stat::mode_t) -> Result<(), GitError> {
        match SFlag::from_bits_truncate(mode) & SFlag::S_IFMT {
            SFlag::S_IFREG => Ok(()),
            SFlag::S_IFLNK => Err(refused("a configuration file is a link")),
            _ => Err(refused("a configuration file is not a regular file")),
        }
    }

    /// Scrubs `name` of `git_dir` under Git's own lock, `<name>.lock`: Git's writers and the scrub
    /// exclude each other, and a lock (or a link) already there refuses the scrub.
    pub(super) fn scrub_file(
        git: &Git,
        git_dir: &OwnedFd,
        name: &str,
        private: &Path,
        removed: &dyn Fn(&str) -> bool,
    ) -> Result<(), GitError> {
        let lock_name = format!("{name}.lock");
        let lock_flags =
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let lock = match openat(
            git_dir,
            lock_name.as_str(),
            lock_flags,
            Mode::from_bits_truncate(0o600),
        ) {
            Ok(fd) => Lock {
                dir: git_dir,
                name: lock_name,
                file: Some(std::fs::File::from(fd)),
            },
            Err(Errno::EEXIST) => return Err(refused("its configuration is locked")),
            Err(err) => return Err(io("locking the repository configuration", err)),
        };
        // `O_NONBLOCK`: a FIFO put there must not hang the open; it is refused just after.
        let read_flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let file = match openat(git_dir, name, read_flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => return Ok(()),
            Err(Errno::ELOOP) => return Err(refused("a configuration file is a link")),
            Err(err) => return Err(io("opening the repository configuration", err)),
        };
        let stat = fstat(&file).map_err(|err| io("checking the repository configuration", err))?;
        regular(stat.st_mode)?;
        let mut original = Vec::new();
        std::fs::File::from(file)
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut original)
            .map_err(|err| io("reading the repository configuration", err))?;
        if u64::try_from(original.len()).unwrap_or(u64::MAX) > MAX_CONFIG_BYTES {
            return Err(refused("a configuration file is too large"));
        }
        let copy = private.join(name);
        std::fs::write(&copy, &original).map_err(|err| io("copying the configuration", err))?;
        let copy_arg = copy.to_string_lossy().into_owned();
        let listed = git.run_ok(Invocation::new(git, "config", Some(private)).args([
            "config",
            "--file",
            &copy_arg,
            "--list",
            "--name-only",
        ]))?;
        let mut names: Vec<String> = listed
            .stdout_trimmed()
            .lines()
            .filter(|key| removed(key))
            .map(str::to_owned)
            .collect();
        if names.is_empty() {
            return Ok(());
        }
        names.sort();
        names.dedup();
        for key in names {
            git.run_ok(Invocation::new(git, "config", Some(private)).args([
                "config",
                "--file",
                &copy_arg,
                "--unset-all",
                &key,
            ]))?;
        }
        let scrubbed = std::fs::read(&copy).map_err(|err| io("reading the scrubbed copy", err))?;
        lock.commit(
            name,
            &scrubbed,
            Mode::from_bits_truncate(stat.st_mode & 0o7777),
        )
    }

    /// The taken `<name>.lock`; removed unless it was renamed over the name.
    struct Lock<'a> {
        dir: &'a OwnedFd,
        name: String,
        file: Option<std::fs::File>,
    }

    impl Lock<'_> {
        /// Writes `bytes` with `mode` and renames the lock over `name` through the `.git` handle
        /// (a link there is replaced, never followed).
        fn commit(mut self, name: &str, bytes: &[u8], mode: Mode) -> Result<(), GitError> {
            let mut file = self
                .file
                .take()
                .ok_or_else(|| refused("the lock was released"))?;
            file.write_all(bytes)
                .and_then(|()| fchmod(&file, mode).map_err(std::io::Error::from))
                .and_then(|()| file.sync_all())
                .map_err(|err| io("writing the scrubbed configuration", err))?;
            drop(file);
            renameat(self.dir, self.name.as_str(), self.dir, name)
                .map_err(|err| io("replacing the repository configuration", err))?;
            self.name.clear();
            Ok(())
        }
    }

    impl Drop for Lock<'_> {
        fn drop(&mut self) {
            if !self.name.is_empty() {
                let _ = unlinkat(self.dir, self.name.as_str(), UnlinkatFlags::NoRemoveDir);
            }
        }
    }

    /// A folder of the service user only (0700), for Git's copies; removed with its contents.
    pub(super) struct PrivateDir(PathBuf);

    impl PrivateDir {
        pub(super) fn new() -> Result<Self, GitError> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::temp_dir();
            let mut last = None;
            for _ in 0..8 {
                let path = base.join(format!(
                    "yulius-scrub-{}-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |since| since.subsec_nanos()),
                ));
                match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(err) => last = Some(err),
                }
            }
            Err(io(
                "creating the scrub's private folder",
                last.unwrap_or_else(|| std::io::Error::other("no attempt")),
            ))
        }

        pub(super) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for PrivateDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
