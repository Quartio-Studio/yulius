//! Installs the skill catalog into the Claude Code home (SPEC-001 SKL-001/002/004, SPEC-002
//! §7.2 "Skills", AC-SKL-1).
//!
//! Each skill directory (containing `SKILL.md`) is copied to `<claude_skills_dir>/<name>`. A copy is staged next to the target and swapped in by rename, so
//! readers see either the old or the new complete skill. Installed copies carry a
//! [`MANAGED_MARKER`] file; managed copies whose skill is no longer in the catalog are removed,
//! while skills Yulius did not install are left alone.
//!
//! Symbolic links inside a skill are followed only to regular files inside the skill's own
//! directory; others are skipped and counted. Sizes are bounded per skill.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// File written into every Yulius-installed skill copy.
pub const MANAGED_MARKER: &str = ".yulius-managed";
/// Maximum total bytes of one skill.
pub const MAX_SKILL_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum files and directories in one skill.
pub const MAX_SKILL_ENTRIES: usize = 2_000;
/// Maximum directory depth inside one skill.
pub const MAX_SKILL_DEPTH: usize = 16;

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

/// Copies every skill into the skills directory, replacing stale copies, and removes managed
/// copies of skills no longer listed.
///
/// # Errors
/// [`SkillsError`] for an invalid name, a missing `SKILL.md`, an exceeded bound or an I/O
/// failure. All sources are validated before anything is written.
pub fn install_skills(
    sources: &[SkillDir],
    root: &Path,
    skills_dir: &Path,
) -> Result<InstallReport, SkillsError> {
    let claude_skills_dir = &root.join(skills_dir);
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
    let dir = claude_skills_dir;
    fs::create_dir_all(dir).map_err(io_error(""))?;
    clean_leftovers(dir).map_err(io_error(""))?;
    for skill in sources {
        report.skipped_links += install_one(skill, dir)?;
        report.installed.push(skill.name.clone());
    }
    report.removed = prune(dir, &names).map_err(io_error(""))?;
    report.removed.sort();
    Ok(report)
}

fn staging_name(name: &str, kind: &str) -> String {
    format!(".{name}.yulius-{kind}-{}", std::process::id())
}

/// Removes staging/backup directories left by an interrupted earlier run.
fn clean_leftovers(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let leftover = name.starts_with('.')
            && (name.contains(".yulius-stage-") || name.contains(".yulius-old-"));
        if leftover && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn install_one(skill: &SkillDir, dir: &Path) -> Result<usize, SkillsError> {
    let staging = dir.join(staging_name(&skill.name, "stage"));
    let backup = dir.join(staging_name(&skill.name, "old"));
    let target = dir.join(&skill.name);
    let result = (|| {
        let root = fs::canonicalize(&skill.path).map_err(io_error(&skill.name))?;
        let mut budget = Budget {
            bytes: 0,
            entries: 0,
        };
        fs::create_dir(&staging).map_err(io_error(&skill.name))?;
        staged(&staging);
        let skipped = copy_tree(&root, &root, &staging, 0, &mut budget, &skill.name)?;
        fs::write(staging.join(MANAGED_MARKER), b"installed by yulius\n")
            .map_err(io_error(&skill.name))?;
        let had_old = match fs::symlink_metadata(&target) {
            Ok(_) => {
                fs::rename(&target, &backup).map_err(io_error(&skill.name))?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(io_error(&skill.name)(error)),
        };
        if let Err(error) = fs::rename(&staging, &target) {
            if had_old {
                let _restored = fs::rename(&backup, &target);
            }
            return Err(io_error(&skill.name)(error));
        }
        if had_old {
            remove_any(&backup).map_err(io_error(&skill.name))?;
        }
        Ok(skipped)
    })();
    if result.is_err() {
        let _ignored = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(test)]
thread_local! {
    /// Test-only pause point (ADR-0174): runs once the staging folder exists, before the copy.
    static STAGED: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

/// The staging folder exists and is empty; nothing is copied yet.
fn staged(staging: &Path) {
    #[cfg(test)]
    if let Some(hook) = STAGED.with(|hook| hook.borrow_mut().take()) {
        hook(staging);
    }
    #[cfg(not(test))]
    let _ = staging;
}

fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

struct Budget {
    bytes: u64,
    entries: usize,
}

fn copy_tree(
    root: &Path,
    from: &Path,
    to: &Path,
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
        let dest = to.join(&file_name);
        let kind = entry.file_type().map_err(io_error(name))?;
        if kind.is_dir() {
            fs::create_dir(&dest).map_err(io_error(name))?;
            skipped += copy_tree(root, &source, &dest, depth + 1, budget, name)?;
        } else if kind.is_file() {
            copy_file(&source, &dest, budget, name)?;
        } else if kind.is_symlink() {
            match fs::canonicalize(&source) {
                Ok(real) if real.starts_with(root) && real.is_file() => {
                    copy_file(&real, &dest, budget, name)?;
                }
                _ => skipped += 1,
            }
        }
    }
    Ok(skipped)
}

fn copy_file(
    source: &Path,
    dest: &Path,
    budget: &mut Budget,
    name: &str,
) -> Result<(), SkillsError> {
    let len = fs::metadata(source).map_err(io_error(name))?.len();
    budget.bytes = budget.bytes.saturating_add(len);
    if budget.bytes > MAX_SKILL_BYTES {
        return Err(SkillsError::TooLarge(name.to_owned()));
    }
    fs::copy(source, dest).map_err(io_error(name))?;
    Ok(())
}

/// Removes managed copies whose names are not in `keep`; returns their names.
fn prune(dir: &Path, keep: &[&str]) -> io::Result<Vec<String>> {
    let mut removed = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || keep.contains(&name.as_str()) {
            continue;
        }
        if entry.path().join(MANAGED_MARKER).is_file() {
            fs::remove_dir_all(entry.path())?;
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
            *hook.borrow_mut() = Some(Box::new(move |staging: &Path| {
                std::os::unix::fs::symlink(&target, staging.join("SKILL.md")).ok();
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
    fn names() {
        assert!(valid_name("om-code-review"));
        assert!(valid_name("a.b_c"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name(""));
    }
}
