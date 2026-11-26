//! What a sandbox's system holds: its base, and its overlay `upper` layer over it.

use std::path::Path;

/// Where a system layer has `dockerd` (`docker-ce` puts it in `usr/bin`, `docker.io` in
/// `usr/sbin`; `usr/local/sbin` comes first on the engine's `PATH`).
pub const DOCKERD_PATHS: [&str; 4] = [
    "usr/local/sbin/dockerd",
    "usr/local/bin/dockerd",
    "usr/sbin/dockerd",
    "usr/bin/dockerd",
];

/// Whether the system a sandbox's engine runs has `dockerd` (SPEC-199 DSY-002): the sandbox's
/// overlay `upper` layer decides where it has an entry, else its `base`. The upper layer is the
/// sandbox's to write, so it is walked from one directory descriptor to the next without
/// following links (no path is looked up twice): a link at `dockerd` counts (it resolves inside
/// the sandbox), and anything else over a path (a whiteout, a directory, a link over a parent)
/// hides the base's.
#[must_use]
pub fn system_has_dockerd(base: &Path, upper: &Path) -> bool {
    DOCKERD_PATHS
        .iter()
        .any(|path| upper_has(upper, path).unwrap_or_else(|| base.join(path).is_file()))
}

/// The `dockerd` a sandbox's engine runs when Docker installed in the sandbox shadows its
/// base's (SPEC-203 HST-017): the first of [`DOCKERD_PATHS`] in the merged system comes from the
/// upper layer, at another path than the base's own (Ubuntu's `docker.io` in `usr/sbin` over
/// `docker-ce` in `usr/bin`). `None` when the base's runs, the base has none, or the sandbox
/// reinstalled Docker at the base's path.
#[must_use]
pub fn own_engine(base: &Path, upper: &Path) -> Option<&'static str> {
    let base_first = DOCKERD_PATHS
        .iter()
        .find(|path| base.join(path).is_file())?;
    for path in DOCKERD_PATHS {
        match upper_has(upper, path) {
            Some(true) => return (path != *base_first).then_some(path),
            Some(false) => {}
            None if base.join(path).is_file() => return None,
            None => {}
        }
    }
    None
}

/// What an overlay upper layer says about the file at `relative`: `Some(true)` it provides one,
/// `Some(false)` it hides the lower one, `None` it has no entry on the way. Never follows a link.
#[cfg(target_os = "linux")]
fn upper_has(upper: &Path, relative: &str) -> Option<bool> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
    use rustix::io::Errno;
    let walk = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = openat(rustix::fs::CWD, upper, walk, Mode::empty()).ok()?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return match statat(&dir, part, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => Some(matches!(
                    FileType::from_raw_mode(stat.st_mode),
                    FileType::RegularFile | FileType::Symlink
                )),
                Err(_) => None,
            };
        }
        dir = match openat(&dir, part, walk, Mode::empty()) {
            Ok(next) => next,
            // A link or a file (a whiteout too) over the directory hides the lower one.
            Err(Errno::LOOP | Errno::NOTDIR) => return Some(false),
            Err(_) => return None,
        };
    }
    None
}

/// Sandboxes are Linux only: elsewhere the base decides.
#[cfg(not(target_os = "linux"))]
fn upper_has(_upper: &Path, _relative: &str) -> Option<bool> {
    None
}

/// The most a sandbox's `/etc/passwd` or `/etc/group` may hold to be read.
const MAX_SYSTEM_FILE_BYTES: u64 = 1 << 20;

/// The text of `etc/<name>` of a sandbox's system (ADR-0315): its overlay `upper` layer's file
/// where that layer has one, else its `base`'s. The upper layer is the sandbox's to write (root
/// in it owns `/etc`), so it is walked from one handle to the next without following a link,
/// the file is opened without following a link or waiting, and only a regular file of at most
/// 1 MiB is read. Anything else at the name or over `etc` (a link, which
/// would resolve inside the sandbox and not on the host; a whiteout; a larger file) gives
/// nothing, as it hides the base's.
#[must_use]
pub fn system_file_text(base: &Path, upper: &Path, name: &str) -> Option<String> {
    let in_base = || bounded_text(std::fs::File::open(base.join("etc").join(name)).ok()?);
    match upper_file(upper, name) {
        UpperFile::Regular(file) => bounded_text(file),
        UpperFile::Hidden => None,
        UpperFile::Absent => in_base(),
    }
}

/// What a sandbox's upper layer holds at `etc/<name>`.
enum UpperFile {
    /// A regular file, opened.
    Regular(std::fs::File),
    /// Something else there, or over `etc`: the base's file does not show.
    Hidden,
    /// No entry: the base's file shows.
    Absent,
}

fn upper_file(upper: &Path, name: &str) -> UpperFile {
    use nix::errno::Errno;
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    let walk = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let Ok(root) = open(upper, walk, Mode::empty()) else {
        return UpperFile::Absent;
    };
    let etc = match openat(&root, "etc", walk, Mode::empty()) {
        Ok(etc) => etc,
        Err(Errno::ENOENT) => return UpperFile::Absent,
        Err(_) => return UpperFile::Hidden,
    };
    let read = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    match openat(&etc, name, read, Mode::empty()) {
        Ok(fd) => {
            let file = std::fs::File::from(fd);
            if file.metadata().is_ok_and(|meta| meta.is_file()) {
                UpperFile::Regular(file)
            } else {
                UpperFile::Hidden
            }
        }
        Err(Errno::ENOENT) => UpperFile::Absent,
        Err(_) => UpperFile::Hidden,
    }
}

/// The text of `file` when it is UTF-8 of at most [`MAX_SYSTEM_FILE_BYTES`].
fn bounded_text(file: std::fs::File) -> Option<String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    file.take(MAX_SYSTEM_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SYSTEM_FILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}
