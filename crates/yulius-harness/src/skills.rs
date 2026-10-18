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

/// The names in the folder `dir` (`.` and `..` left out).
fn entries(dir: &OwnedFd) -> io::Result<Vec<OsString>> {
    use std::os::unix::ffi::OsStrExt as _;
    // A fresh description of the same folder, so every listing starts at its beginning.
    let mut listing = nix::dir::Dir::openat(dir, ".", DIR_FLAGS, Mode::empty())?;
    let mut names = Vec::new();
    for entry in listing.iter() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
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
    fn a_skills_folder_with_too_many_entries_is_refused() -> Result<(), Box<dyn std::error::Error>> {
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
