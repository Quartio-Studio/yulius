//! Installs the skill catalog into the Claude Code home (SPEC-001 SKL-001/002/004, SPEC-002
//! §7.2 "Skills", AC-SKL-1).
//!
//! Each skill directory (containing `SKILL.md`) is copied to `<skills dir>/<name>`. A copy is
//! staged next to the target and swapped in by rename, so readers see either the old or the new
//! complete skill.
//!
//! The skills folder lies in the agent's home, which the agent can change at any moment. So it is
//! reached from the sandbox directory one folder at a time without following links, and every
//! copy, rename and removal goes through folder handles (`openat`, `renameat`, `unlinkat`); files
//! are created with `O_EXCL | O_NOFOLLOW` in staging folders with unpredictable names. A link the
//! agent plants is refused or removed, never followed (#657, ADR-0174). Installed copies carry a
//! [`MANAGED_MARKER`] file; managed copies whose skill is no longer in the catalog are removed,
//! while skills Yulius did not install are left alone.
//!
//! Symbolic links inside a skill are followed only to regular files inside the skill's own
//! directory; others are skipped and counted. Sizes are bounded per skill.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fchmod, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use sha2::{Digest as _, Sha256};

/// File written into every Yulius-installed skill copy.
pub const MANAGED_MARKER: &str = ".yulius-managed";
/// Maximum total bytes of one skill.
pub const MAX_SKILL_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum files and directories in one skill.
pub const MAX_SKILL_ENTRIES: usize = 2_000;
/// Maximum directory depth inside one skill.
pub const MAX_SKILL_DEPTH: usize = 16;
/// Most entries read from one folder of the skills folder (the agent can fill it).
const MAX_LISTED_ENTRIES: usize = 10_000;
/// Deepest tree removed from the skills folder (each level holds one open handle).
const MAX_REMOVE_DEPTH: usize = 64;
/// Flags of every folder opened in the skills folder.
const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// One skill of the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDir {
    /// Directory name in the harness homes: 1–64 ASCII letters, digits, `.`, `-` or `_`, not
    /// starting with `.`.
    pub name: String,
    /// Source directory containing `SKILL.md`.
    pub path: PathBuf,
}

/// What [`install_skills`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    /// Skills installed (in catalog order).
    pub installed: Vec<String>,
    /// Managed copies removed because their skill left the catalog.
    pub removed: Vec<String>,
    /// Symbolic links skipped because they escape the skill or point to a directory.
    pub skipped_links: usize,
}

/// What [`refresh_skills`] did: nothing at all when it is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// Skills whose copy was missing or differed from the catalog, installed again (in catalog
    /// order).
    pub changed: Vec<String>,
    /// Managed copies removed because their skill left the catalog.
    pub removed: Vec<String>,
}

impl RefreshReport {
    /// Whether every copy matched, so nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// Why skills could not be installed. Nothing is swapped in for a skill that fails.
#[derive(Debug, thiserror::Error)]
pub enum SkillsError {
    /// The skill name is not a safe directory name, or is listed twice.
    #[error("invalid or duplicate skill name {0:?}")]
    InvalidName(String),
    /// The source has no `SKILL.md`.
    #[error("skill {0:?} has no SKILL.md")]
    MissingSkillMd(String),
    /// The source exceeds a size, entry or depth bound.
    #[error("skill {0:?} exceeds the size, entry or depth bound")]
    TooLarge(String),
    /// An I/O error.
    #[error("skill {name:?}: {source}")]
    Io {
        /// Skill being processed (empty for directory preparation).
        name: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

fn io_error(name: &str) -> impl FnOnce(io::Error) -> SkillsError + '_ {
    move |source| SkillsError::Io {
        name: name.to_owned(),
        source,
    }
}

/// Copies every skill into `skills_dir` (relative to `root`, the sandbox directory, created when
/// missing), replacing stale copies, and removes managed copies of skills no longer listed.
///
/// # Errors
/// [`SkillsError`] for an invalid name, a missing `SKILL.md`, an exceeded bound, a link or a file
/// on the way from `root` to `skills_dir`, or an I/O failure. All sources are validated before
/// anything is written.
pub fn install_skills(
    sources: &[SkillDir],
    root: &Path,
    skills_dir: &Path,
) -> Result<InstallReport, SkillsError> {
    let names = validated_names(sources)?;
    let mut report = InstallReport::default();
    let dir = open_skills_dir(root, skills_dir).map_err(io_error(""))?;
    clean_leftovers(&dir).map_err(io_error(""))?;
    for skill in sources {
        report.skipped_links += install_one(skill, &dir)?;
        report.installed.push(skill.name.clone());
    }
    report.removed = prune(&dir, &names).map_err(io_error(""))?;
    report.removed.sort();
    Ok(report)
}

/// Brings `skills_dir` (relative to `root`, as for [`install_skills`]) to the catalog `sources`,
/// writing only what differs: a skill whose copy is missing or differs from its source is installed
/// as [`install_skills`] installs it (staged, swapped in by rename), and managed copies of skills
/// no longer listed are removed. Copies that match are left as they are (SPEC-507 SKR-001).
///
/// A copy is compared by a digest of its files read through folder handles, never through a link
/// and within the install's bounds; a link, a FIFO or anything else that is not a regular file or
/// a folder in a copy, a copy without [`MANAGED_MARKER`] and a copy beyond a bound (a folder in it
/// too long to list included) all count as different (SKR-002). The skills folder itself is listed
/// within its own bound, as the install lists it: beyond it the refresh fails.
///
/// # Errors
/// As [`install_skills`]: an invalid source, a link or a file on the way from `root` to
/// `skills_dir`, or an I/O failure. A skill that fails stops the refresh; the skills before it
/// were refreshed.
pub fn refresh_skills(
    sources: &[SkillDir],
    root: &Path,
    skills_dir: &Path,
) -> Result<RefreshReport, SkillsError> {
    let names = validated_names(sources)?;
    let mut report = RefreshReport::default();
    let dir = open_skills_dir(root, skills_dir).map_err(io_error(""))?;
    clean_leftovers(&dir).map_err(io_error(""))?;
    for skill in sources {
        let wanted = source_digest(skill)?;
        let installed =
            installed_digest(&dir, OsStr::new(&skill.name)).map_err(io_error(&skill.name))?;
        if installed != Some(wanted) {
            install_one(skill, &dir)?;
            report.changed.push(skill.name.clone());
        }
    }
    report.removed = prune(&dir, &names).map_err(io_error(""))?;
    report.removed.sort();
    Ok(report)
}

/// The names of `sources`, each checked to be a safe directory name, listed once and holding a
/// `SKILL.md`.
fn validated_names(sources: &[SkillDir]) -> Result<Vec<&str>, SkillsError> {
    let mut names: Vec<&str> = Vec::new();
    for skill in sources {
        if !valid_name(&skill.name) || names.contains(&skill.name.as_str()) {
            return Err(SkillsError::InvalidName(skill.name.clone()));
        }
        names.push(&skill.name);
        let manifest = fs::symlink_metadata(skill.path.join("SKILL.md"));
        if !manifest.is_ok_and(|m| m.is_file()) {
            return Err(SkillsError::MissingSkillMd(skill.name.clone()));
        }
    }
    Ok(names)
}

/// The digest of what [`install_one`] would install from `skill`: the same files, links and
/// bounds as [`copy_tree`].
fn source_digest(skill: &SkillDir) -> Result<[u8; 32], SkillsError> {
    let root = fs::canonicalize(&skill.path).map_err(io_error(&skill.name))?;
    let mut hasher = Sha256::new();
    let mut budget = Budget {
        bytes: 0,
        entries: 0,
    };
    hash_source(&root, &root, &mut hasher, 0, &mut budget, &skill.name)?;
    Ok(hasher.finalize().into())
}

fn hash_source(
    root: &Path,
    from: &Path,
    hasher: &mut Sha256,
    depth: usize,
    budget: &mut Budget,
    name: &str,
) -> Result<(), SkillsError> {
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    if depth > MAX_SKILL_DEPTH {
        return Err(SkillsError::TooLarge(name.to_owned()));
    }
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(from)
        .map_err(io_error(name))?
        .collect::<io::Result<_>>()
        .map_err(io_error(name))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        budget.entries += 1;
        if budget.entries > MAX_SKILL_ENTRIES {
            return Err(SkillsError::TooLarge(name.to_owned()));
        }
        let file_name = entry.file_name();
        if file_name == MANAGED_MARKER {
            continue;
        }
        let source = entry.path();
        let kind = entry.file_type().map_err(io_error(name))?;
        let file = if kind.is_dir() {
            hash_name(hasher, b'd', file_name.as_bytes());
            hash_source(root, &source, hasher, depth + 1, budget, name)?;
            hasher.update(b"u");
            None
        } else if kind.is_file() {
            Some(source)
        } else if kind.is_symlink() {
            match fs::canonicalize(&source) {
                Ok(real) if real.starts_with(root) && real.is_file() => Some(real),
                _ => None,
            }
        } else {
            None
        };
        if let Some(path) = file {
            let meta = fs::metadata(&path).map_err(io_error(name))?;
            budget.bytes = budget.bytes.saturating_add(meta.len());
            if budget.bytes > MAX_SKILL_BYTES {
                return Err(SkillsError::TooLarge(name.to_owned()));
            }
            hash_name(hasher, b'f', file_name.as_bytes());
            hasher.update(file_mode(meta.permissions().mode()).to_le_bytes());
            let input = fs::File::open(&path).map_err(io_error(name))?;
            let left = MAX_SKILL_BYTES - budget.bytes + meta.len();
            hash_content(hasher, input, left).map_err(io_error(name))?;
        }
    }
    Ok(())
}

/// The digest of the installed copy `name` in `dir`, read through folder handles without
/// following a link; `None` when it is missing, is not a folder, has no [`MANAGED_MARKER`], holds
/// anything but regular files and folders, or exceeds a bound: a copy that differs.
fn installed_digest(dir: &OwnedFd, name: &OsStr) -> io::Result<Option<[u8; 32]>> {
    let copy = match open_dir_at(dir, name, false) {
        Ok(copy) => copy,
        Err(Errno::ENOENT | Errno::ELOOP | Errno::ENOTDIR) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if kind(&copy, OsStr::new(MANAGED_MARKER))? != Some(SFlag::S_IFREG) {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    let mut budget = Budget {
        bytes: 0,
        entries: 0,
    };
    Ok(hash_installed(&copy, &mut hasher, 0, &mut budget)?.then(|| hasher.finalize().into()))
}

/// Hashes the folder `dir` of an installed copy as [`hash_source`] hashes its source; `false` when
/// it holds something no install writes or exceeds a bound.
fn hash_installed(
    dir: &OwnedFd,
    hasher: &mut Sha256,
    depth: usize,
    budget: &mut Budget,
) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt as _;
    if depth > MAX_SKILL_DEPTH {
        return Ok(false);
    }
    // A folder too long to list is no copy the install wrote.
    let Ok(mut names) = entries(dir) else {
        return Ok(false);
    };
    names.sort();
    for entry in names {
        if depth == 0 && entry == MANAGED_MARKER {
            continue;
        }
        budget.entries += 1;
        if budget.entries > MAX_SKILL_ENTRIES {
            return Ok(false);
        }
        match kind(dir, &entry)? {
            Some(SFlag::S_IFDIR) => {
                let Ok(folder) = open_dir_at(dir, &entry, false) else {
                    return Ok(false);
                };
                hash_name(hasher, b'd', entry.as_bytes());
                if !hash_installed(&folder, hasher, depth + 1, budget)? {
                    return Ok(false);
                }
                hasher.update(b"u");
            }
            Some(SFlag::S_IFREG) => {
                // Non-blocking, so a FIFO swapped in after the check is never waited on; the
                // handle's own type is checked again.
                let flags = OFlag::O_RDONLY
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_NONBLOCK
                    | OFlag::O_NOCTTY
                    | OFlag::O_CLOEXEC;
                let Ok(fd) = openat(dir, entry.as_os_str(), flags, Mode::empty()) else {
                    return Ok(false);
                };
                let stat = nix::sys::stat::fstat(&fd)?;
                if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
                    return Ok(false);
                }
                let len = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
                budget.bytes = budget.bytes.saturating_add(len);
                if budget.bytes > MAX_SKILL_BYTES {
                    return Ok(false);
                }
                hash_name(hasher, b'f', entry.as_bytes());
                hasher.update(file_mode(stat.st_mode).to_le_bytes());
                // At most what the bound has left: a file grown after the check reads differently.
                let left = MAX_SKILL_BYTES - budget.bytes + len;
                hash_content(hasher, fs::File::from(fd), left)?;
            }
            // A link, a FIFO, a socket or a device: never installed, so the copy differs.
            Some(_) => return Ok(false),
            // Removed meanwhile.
            None => return Ok(false),
        }
    }
    Ok(true)
}

/// The permission bits a copy keeps of its source's mode (`mode_t` is 16 bits on macOS).
fn file_mode(mode: impl Into<u32>) -> u32 {
    mode.into() & 0o777
}

fn hash_name(hasher: &mut Sha256, tag: u8, name: &[u8]) {
    hasher.update([tag]);
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name);
}

/// Hashes at most `limit` bytes of `input` and their count.
fn hash_content(hasher: &mut Sha256, input: fs::File, limit: u64) -> io::Result<()> {
    let mut input = io::Read::take(input, limit);
    let mut content = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = io::Read::read(&mut input, &mut buf)?;
        if read == 0 {
            break;
        }
        content.update(&buf[..read]);
        total += read as u64;
    }
    hasher.update(total.to_le_bytes());
    hasher.update(content.finalize());
    Ok(())
}

/// Opens `skills_dir` below `root` one folder at a time without following links, creating the
/// missing ones.
fn open_skills_dir(root: &Path, skills_dir: &Path) -> io::Result<OwnedFd> {
    let mut dir = nix::fcntl::open(root, DIR_FLAGS.difference(OFlag::O_NOFOLLOW), Mode::empty())?;
    for component in skills_dir.components() {
        let Component::Normal(part) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the skills folder is not below the sandbox directory",
            ));
        };
        dir = open_dir_at(&dir, part, true).map_err(|err| match err {
            Errno::ELOOP | Errno::ENOTDIR => io::Error::other(format!(
                "{} on the way to the skills folder is a link or not a folder: Yulius follows no \
                 links",
                part.to_string_lossy()
            )),
            other => other.into(),
        })?;
    }
    Ok(dir)
}

/// Opens the folder `name` in `dir` without following a link, creating it first when it is
/// missing and `create`.
fn open_dir_at(dir: &OwnedFd, name: &OsStr, create: bool) -> Result<OwnedFd, Errno> {
    match openat(dir, name, DIR_FLAGS, Mode::empty()) {
        Err(Errno::ENOENT) if create => {
            match mkdirat(dir, name, Mode::from_bits_truncate(0o777)) {
                Ok(()) | Err(Errno::EEXIST) => {}
                Err(err) => return Err(err),
            }
            openat(dir, name, DIR_FLAGS, Mode::empty())
        }
        other => other,
    }
}

/// The names in the folder `dir` (`.` and `..` left out); more than [`MAX_LISTED_ENTRIES`] is an
/// error, so a folder the agent filled is never read whole into memory.
fn entries(dir: &OwnedFd) -> io::Result<Vec<OsString>> {
    use std::os::unix::ffi::OsStrExt as _;
    // A fresh description of the same folder, so every listing starts at its beginning.
    let mut listing = nix::dir::Dir::openat(dir, ".", DIR_FLAGS, Mode::empty())?;
    let mut names = Vec::new();
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            if names.len() == MAX_LISTED_ENTRIES {
                return Err(io::Error::other(format!(
                    "a folder in the skills folder has more than {MAX_LISTED_ENTRIES} entries"
                )));
            }
            names.push(OsStr::from_bytes(name).to_owned());
        }
    }
    Ok(names)
}

/// The type of `name` in `dir` (a link is not followed), `None` when it is missing.
fn kind(dir: &OwnedFd, name: &OsStr) -> io::Result<Option<SFlag>> {
    match fstatat(dir, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT,
        )),
        Err(Errno::ENOENT) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Removes staging/backup directories left by an interrupted earlier run.
fn clean_leftovers(dir: &OwnedFd) -> io::Result<()> {
    for name in entries(dir)? {
        let text = name.to_string_lossy();
        let leftover = text.starts_with('.')
            && (text.contains(".yulius-stage-") || text.contains(".yulius-old-"));
        if leftover && kind(dir, &name)? == Some(SFlag::S_IFDIR) {
            remove_tree(dir, &name, 0)?;
        }
    }
    Ok(())
}

fn install_one(skill: &SkillDir, dir: &OwnedFd) -> Result<usize, SkillsError> {
    // Unpredictable, so the agent cannot prepare an entry under the name before it exists.
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let staging = OsString::from(format!(".{}.yulius-stage-{tag}", skill.name));
    let backup = OsString::from(format!(".{}.yulius-old-{tag}", skill.name));
    let target = OsStr::new(&skill.name);
    let result = (|| {
        let root = fs::canonicalize(&skill.path).map_err(io_error(&skill.name))?;
        let mut budget = Budget {
            bytes: 0,
            entries: 0,
        };
        mkdirat(dir, staging.as_os_str(), Mode::from_bits_truncate(0o777))
            .map_err(|e| io_error(&skill.name)(e.into()))?;
        let staged_dir =
            open_dir_at(dir, &staging, false).map_err(|e| io_error(&skill.name)(e.into()))?;
        staged(&staged_dir);
        let skipped = copy_tree(&root, &root, &staged_dir, 0, &mut budget, &skill.name)?;
        write_new(
            &staged_dir,
            OsStr::new(MANAGED_MARKER),
            b"installed by yulius\n",
            0o644,
        )
        .map_err(io_error(&skill.name))?;
        let had_old = kind(dir, target).map_err(io_error(&skill.name))?.is_some();
        if had_old {
            renameat(dir, target, dir, backup.as_os_str())
                .map_err(|e| io_error(&skill.name)(e.into()))?;
        }
        if let Err(error) = renameat(dir, staging.as_os_str(), dir, target) {
            if had_old {
                let _restored = renameat(dir, backup.as_os_str(), dir, target);
            }
            return Err(io_error(&skill.name)(error.into()));
        }
        if had_old {
            remove_tree(dir, &backup, 0).map_err(io_error(&skill.name))?;
        }
        Ok(skipped)
    })();
    if result.is_err() {
        let _ignored = remove_tree(dir, &staging, 0);
    }
    result
}

/// A test hook run once the staging folder exists, before the copy (ADR-0174).
#[cfg(test)]
type StagedHook = Box<dyn FnOnce(&OwnedFd)>;

#[cfg(test)]
thread_local! {
    static STAGED: std::cell::RefCell<Option<StagedHook>> = const { std::cell::RefCell::new(None) };
}

/// Test-only pause point: the staging folder exists and is empty; nothing is copied yet.
fn staged(staging: &OwnedFd) {
    #[cfg(test)]
    if let Some(hook) = STAGED.with(|hook| hook.borrow_mut().take()) {
        hook(staging);
    }
    #[cfg(not(test))]
    let _ = staging;
}

/// Removes `name` from `dir`: a folder with everything below it, each opened through its parent's
/// handle; a link or a file is removed itself, never followed. A missing name is fine.
fn remove_tree(dir: &OwnedFd, name: &OsStr, depth: usize) -> io::Result<()> {
    match open_dir_at(dir, name, false) {
        Ok(folder) => {
            if depth > MAX_REMOVE_DEPTH {
                return Err(io::Error::other("a folder to remove is too deep"));
            }
            for child in entries(&folder)? {
                remove_tree(&folder, &child, depth + 1)?;
            }
            unlinkat(dir, name, UnlinkatFlags::RemoveDir)?;
        }
        Err(Errno::ELOOP | Errno::ENOTDIR) => unlinkat(dir, name, UnlinkatFlags::NoRemoveDir)?,
        Err(Errno::ENOENT) => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

/// Creates `name` in `dir` (never through an entry already there) with `bytes` and `mode`.
fn write_new(dir: &OwnedFd, name: &OsStr, bytes: &[u8], mode: nix::libc::mode_t) -> io::Result<()> {
    let mut file = create_new(dir, name)?;
    io::Write::write_all(&mut file, bytes)?;
    fchmod(&file, Mode::from_bits_truncate(mode & 0o7777))?;
    Ok(())
}

/// A new file `name` in `dir`, created with `O_EXCL | O_NOFOLLOW`.
fn create_new(dir: &OwnedFd, name: &OsStr) -> io::Result<fs::File> {
    let flags =
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let fd = openat(dir, name, flags, Mode::from_bits_truncate(0o600))?;
    Ok(fs::File::from(fd))
}

struct Budget {
    bytes: u64,
    entries: usize,
}

fn copy_tree(
    root: &Path,
    from: &Path,
    to: &OwnedFd,
    depth: usize,
    budget: &mut Budget,
    name: &str,
) -> Result<usize, SkillsError> {
    if depth > MAX_SKILL_DEPTH {
        return Err(SkillsError::TooLarge(name.to_owned()));
    }
    let mut skipped = 0;
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(from)
        .map_err(io_error(name))?
        .collect::<io::Result<_>>()
        .map_err(io_error(name))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        budget.entries += 1;
        if budget.entries > MAX_SKILL_ENTRIES {
            return Err(SkillsError::TooLarge(name.to_owned()));
        }
        let file_name = entry.file_name();
        if file_name == MANAGED_MARKER {
            continue;
        }
        let source = entry.path();
        let kind = entry.file_type().map_err(io_error(name))?;
        if kind.is_dir() {
            mkdirat(to, file_name.as_os_str(), Mode::from_bits_truncate(0o777))
                .map_err(|e| io_error(name)(e.into()))?;
            let dest = open_dir_at(to, &file_name, false).map_err(|e| io_error(name)(e.into()))?;
            skipped += copy_tree(root, &source, &dest, depth + 1, budget, name)?;
        } else if kind.is_file() {
            copy_file(&source, to, &file_name, budget, name)?;
        } else if kind.is_symlink() {
            match fs::canonicalize(&source) {
                Ok(real) if real.starts_with(root) && real.is_file() => {
                    copy_file(&real, to, &file_name, budget, name)?;
                }
                _ => skipped += 1,
            }
        }
    }
    Ok(skipped)
}

/// Copies `source` (the instance's own catalog) to the new file `dest` in `to`, with its mode.
fn copy_file(
    source: &Path,
    to: &OwnedFd,
    dest: &OsStr,
    budget: &mut Budget,
    name: &str,
) -> Result<(), SkillsError> {
    let meta = fs::metadata(source).map_err(io_error(name))?;
    budget.bytes = budget.bytes.saturating_add(meta.len());
    if budget.bytes > MAX_SKILL_BYTES {
        return Err(SkillsError::TooLarge(name.to_owned()));
    }
    let copied = (|| {
        let mut input = fs::File::open(source)?;
        let mut output = create_new(to, dest)?;
        io::copy(&mut input, &mut output)?;
        // On the new file's handle (fchmod), never by path.
        output.set_permissions(meta.permissions())?;
        Ok(())
    })();
    copied.map_err(io_error(name))
}

/// Removes managed copies whose names are not in `keep`; returns their names.
fn prune(dir: &OwnedFd, keep: &[&str]) -> io::Result<Vec<String>> {
    let mut removed = Vec::new();
    for entry in entries(dir)? {
        let name = entry.to_string_lossy().into_owned();
        if name.starts_with('.') || keep.contains(&name.as_str()) {
            continue;
        }
        if kind(dir, &entry)? != Some(SFlag::S_IFDIR) {
            continue;
        }
        let Ok(copy) = open_dir_at(dir, &entry, false) else {
            continue;
        };
        if kind(&copy, OsStr::new(MANAGED_MARKER))? == Some(SFlag::S_IFREG) {
            drop(copy);
            remove_tree(dir, &entry, 0)?;
            removed.push(name);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_planted_in_the_staging_folder_is_never_written_through()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("catalog/review");
        fs::create_dir_all(&source)?;
        fs::write(source.join("SKILL.md"), "---\nname: review\n---\n")?;
        let victim = dir.path().join("yulius.db");
        fs::write(&victim, "precious")?;
        // The agent watches its skills folder and plants a link the moment staging appears.
        let target = victim.clone();
        STAGED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |staging: &OwnedFd| {
                nix::unistd::symlinkat(&target, staging, "SKILL.md").ok();
            }));
        });
        let skill = SkillDir {
            name: "review".into(),
            path: source,
        };
        let result = install_skills(&[skill], dir.path(), Path::new("home/.claude/skills"));
        assert_eq!(fs::read_to_string(&victim)?, "precious", "{result:?}");
        Ok(())
    }

    #[test]
    fn a_skills_folder_with_too_many_entries_is_refused() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("catalog/review");
        fs::create_dir_all(&source)?;
        fs::write(source.join("SKILL.md"), "---\nname: review\n---\n")?;
        // The agent filled its skills folder: the server never lists it all into memory.
        let skills = dir.path().join("home/.claude/skills");
        fs::create_dir_all(&skills)?;
        for n in 0..=MAX_LISTED_ENTRIES {
            fs::write(skills.join(format!("f{n}")), "")?;
        }
        let skill = SkillDir {
            name: "review".into(),
            path: source,
        };
        let result = install_skills(&[skill], dir.path(), Path::new("home/.claude/skills"));
        assert!(result.is_err(), "{result:?}");
        Ok(())
    }

    /// SPEC-507 review round 2: the read of a copy the agent filled stops at the bound, without
    /// listing or counting what lies beyond it.
    #[test]
    fn the_read_of_a_copy_stops_at_its_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let open = |path: &Path| nix::fcntl::open(path, DIR_FLAGS, Mode::empty());
        let wide = tempfile::tempdir()?;
        for n in 0..MAX_SKILL_ENTRIES + 10 {
            fs::write(wide.path().join(format!("f{n:05}")), "")?;
        }
        let mut budget = Budget {
            bytes: 0,
            entries: 0,
        };
        let mut hasher = Sha256::new();
        assert!(!hash_installed(
            &open(wide.path())?,
            &mut hasher,
            0,
            &mut budget
        )?);
        assert_eq!(
            budget.entries,
            MAX_SKILL_ENTRIES + 1,
            "stopped at the entry bound"
        );

        let deep = tempfile::tempdir()?;
        let bottom: PathBuf = (0..MAX_SKILL_DEPTH + 3).map(|n| format!("d{n}")).collect();
        fs::create_dir_all(deep.path().join(&bottom))?;
        fs::write(deep.path().join(&bottom).join("f"), "")?;
        let mut budget = Budget {
            bytes: 0,
            entries: 0,
        };
        assert!(!hash_installed(
            &open(deep.path())?,
            &mut hasher,
            0,
            &mut budget
        )?);
        assert_eq!(
            budget.entries,
            MAX_SKILL_DEPTH + 1,
            "nothing counted below the depth bound"
        );
        Ok(())
    }

    #[test]
    fn names() {
        assert!(valid_name("om-code-review"));
        assert!(valid_name("a.b_c"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(""));
    }
}
