//! The paths of a session that are its harness's persistent memory and user-level instructions,
//! which every contained session Yulius starts sees read-only (SPEC-652 MEM-001, ADR-0479).
//!
//! - **Claude Code** (2.1.332): `<config>/projects/<key>/memory` (auto memory), `<config>/CLAUDE.md`
//!   and `<config>/rules`; `<config>`, `<config>/projects` and `<config>/projects/<key>` are
//!   pinned, so the memory folder's parents cannot be renamed away (MEM-005).
//! - **Codex** (0.170.0): `AGENTS.md`, `AGENTS.override.md` (an empty one is skipped, so the
//!   placeholder hides nothing) and `memories` in its `CODEX_HOME`.
//! - **Antigravity** (1.5.12): `.gemini/GEMINI.md` and `GEMINI.md`, `AGENTS.md` and `rules` in
//!   `.gemini/config`, in the run's own home.
//!
//! Each path is given relative to an anchor Yulius made, and `yulius sandbox-exec` opens it from
//! there without following a link.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use yulius_contracts::ptyhost::{GuardKind, GuardPath};

use crate::error::{AppError, AppResult};

/// The one sentence a refused write is explained with (SPEC-652 MEM-006).
pub const READ_ONLY_SENTENCE: &str = yulius_contracts::hooks::MEMORY_READ_ONLY_SENTENCE;

/// The run variable that tells the session's hooks its read-only paths (SPEC-652 MEM-006): the
/// guard's read-only folders and files inside the namespace, one per line; `None` for a guard
/// with none.
#[must_use]
pub fn readonly_paths_env(guard: &[GuardPath]) -> Option<(String, String)> {
    let paths: Vec<&str> = guard
        .iter()
        .filter(|p| matches!(p.kind, GuardKind::Dir | GuardKind::File))
        .map(|p| p.dest.as_str())
        .collect();
    (!paths.is_empty()).then(|| {
        (
            yulius_contracts::hooks::READONLY_PATHS_ENV.to_owned(),
            paths.join("\n"),
        )
    })
}

/// Longest Claude Code key kept whole; a longer one is cut there and ends in a hash (2.1.332).
const CLAUDE_KEY_MAX: usize = 200;

/// Claude Code's `projects/<key>` of the folder `base` (2.1.332): every UTF-16 unit outside
/// `[a-zA-Z0-9]` becomes `-`; past 200 units the key is cut there and gets `-` and the absolute
/// value of the folder's 31-hash (over its UTF-16 units, as a 32-bit integer) in base 36.
#[must_use]
pub fn claude_key(base: &Path) -> String {
    let text = base.to_string_lossy();
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut key = String::with_capacity(units.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else {
            key.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if key.len() <= CLAUDE_KEY_MAX {
        return key;
    }
    let hash = units.iter().fold(0_i32, |hash, &unit| {
        (hash << 5).wrapping_sub(hash).wrapping_add(i32::from(unit))
    });
    let mut value = u64::from(hash.unsigned_abs());
    let mut digits = Vec::new();
    loop {
        let digit = u8::try_from(value % 36).unwrap_or(0);
        digits.push(char::from_digit(u32::from(digit), 36).unwrap_or('0'));
        value /= 36;
        if value == 0 {
            break;
        }
    }
    let cut: String = key.chars().take(CLAUDE_KEY_MAX).collect();
    format!("{cut}-{}", digits.iter().rev().collect::<String>())
}

/// The folder Yulius keys a sandbox's Claude Code memory by: its repository, `<sandbox>/repo`,
/// which is what Claude Code itself keys a session in that repository or any of its worktrees by
/// (the git root, a worktree's main repository). Derived from the sandbox's path alone: nothing an
/// agent writes (a worktree's `.git`) can steer it or stall the launch (review round 1).
#[must_use]
pub fn sandbox_memory_base(sandbox_dir: &Path) -> PathBuf {
    sandbox_dir.join("repo")
}

/// The memory folder of a Claude Code session whose configuration is `config` (inside the
/// namespace) and whose memory is keyed by `base`: what the run settings name as
/// `autoMemoryDirectory` and the guard binds read-only (MEM-002).
#[must_use]
pub fn claude_memory_dir(config: &Path, base: &Path) -> PathBuf {
    config
        .join("projects")
        .join(claude_key(base))
        .join("memory")
}

/// A guarded path that is not beneath its anchor: a launch that cannot be guarded does not start.
fn not_beneath(anchor: &Path, path: &Path) -> AppError {
    AppError::internal(format!(
        "cannot guard {}: it is not beneath {}",
        path.display(),
        anchor.display()
    ))
}

/// `path` relative to `anchor`, as plain components; `None` when it is not beneath it.
fn below(anchor: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(anchor).ok()?;
    relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| relative.to_path_buf())
        .filter(|r| !r.as_os_str().is_empty())
}

/// One guarded path: `outer` (beneath `anchor`, in the view bwrap starts in) bound at `inner`.
fn guarded(anchor: &Path, outer: &Path, inner: PathBuf, kind: GuardKind) -> AppResult<GuardPath> {
    Ok(GuardPath {
        anchor: anchor.to_string_lossy().into_owned(),
        relative: below(anchor, outer)
            .ok_or_else(|| not_beneath(anchor, outer))?
            .to_string_lossy()
            .into_owned(),
        dest: inner.to_string_lossy().into_owned(),
        kind,
    })
}

/// Where a Claude Code session's configuration is: `outer` in the view bwrap starts in (beneath
/// `anchor`), `inner` inside the namespace, and the key of its memory folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeConfig<'a> {
    /// A folder Yulius made, containing `outer`.
    pub anchor: &'a Path,
    /// `CLAUDE_CONFIG_DIR` as bwrap starts.
    pub outer: &'a Path,
    /// `CLAUDE_CONFIG_DIR` inside the namespace.
    pub inner: &'a Path,
    /// The session's memory key ([`claude_key`]).
    pub key: &'a str,
}

impl ClaudeConfig<'_> {
    /// The memory folder inside the namespace, which the run settings name as
    /// `autoMemoryDirectory` (MEM-002).
    #[must_use]
    pub fn memory_dir(&self) -> PathBuf {
        self.inner.join("projects").join(self.key).join("memory")
    }

    /// The guarded paths: the pins, outer to inner, then the memory folder, `CLAUDE.md` and
    /// `rules`.
    ///
    /// # Errors
    /// `Internal` when `outer` is not beneath `anchor`.
    pub fn guard(&self) -> AppResult<Vec<GuardPath>> {
        let at = |rel: &[&str]| {
            let mut outer = self.outer.to_path_buf();
            let mut inner = self.inner.to_path_buf();
            for part in rel {
                outer.push(part);
                inner.push(part);
            }
            (outer, inner)
        };
        let entries: [(&[&str], GuardKind); 6] = [
            (&[], GuardKind::Pin),
            (&["projects"], GuardKind::Pin),
            (&["projects", self.key], GuardKind::Pin),
            (&["projects", self.key, "memory"], GuardKind::Dir),
            (&["CLAUDE.md"], GuardKind::File),
            (&["rules"], GuardKind::Dir),
        ];
        entries
            .iter()
            .map(|(rel, kind)| {
                let (outer, inner) = at(rel);
                guarded(self.anchor, &outer, inner, *kind)
            })
            .collect()
    }

    /// Makes the memory folder and its parents when missing, opened one folder at a time without
    /// following a link, so the pins hold from a sandbox's first session (MEM-001). A link or a
    /// file on the way stops it: the session then gets a placeholder there (MEM-003).
    ///
    /// # Errors
    /// The walk's error.
    pub fn prepare(&self) -> Result<(), nix::errno::Errno> {
        let Some(relative) = below(self.anchor, self.outer) else {
            return Err(nix::errno::Errno::EINVAL);
        };
        let parts: Vec<&OsStr> = relative
            .iter()
            .chain([
                OsStr::new("projects"),
                OsStr::new(self.key),
                OsStr::new("memory"),
            ])
            .collect();
        crate::files::open_dirs_below(self.anchor, &parts, true).map(drop)
    }
}

/// The guard of a Claude Code session whose configuration is `outer` beneath `anchor` (`inner`
/// inside the namespace) and whose memory is keyed by `base`: its memory folder made first, so its
/// parents are pinned from the first session; one that cannot be made (a link on the way) gets a
/// placeholder (MEM-003).
///
/// # Errors
/// `Internal` when `outer` is not beneath `anchor`.
pub fn claude_guard(
    anchor: &Path,
    outer: &Path,
    inner: &Path,
    base: &Path,
) -> AppResult<Vec<GuardPath>> {
    let key = claude_key(base);
    let config = ClaudeConfig {
        anchor,
        outer,
        inner,
        key: &key,
    };
    if let Err(err) = config.prepare() {
        tracing::warn!(config = %outer.display(), error = %err, "the memory folder could not be made; the session gets a placeholder");
    }
    config.guard()
}

/// The guarded paths of a Codex home `codex_home` beneath `anchor`, bound at the same path.
///
/// # Errors
/// `Internal` when `codex_home` is not beneath `anchor`.
pub fn codex_guard(anchor: &Path, codex_home: &Path) -> AppResult<Vec<GuardPath>> {
    [
        ("AGENTS.md", GuardKind::File),
        ("AGENTS.override.md", GuardKind::File),
        ("memories", GuardKind::Dir),
    ]
    .into_iter()
    .map(|(name, kind)| {
        let path = codex_home.join(name);
        guarded(anchor, &path, path.clone(), kind)
    })
    .collect()
}

/// The guarded paths of an Antigravity run whose home is `run_home` (its `.gemini` bound at
/// `gemini_inner`), opened from `run_home`, the run's own folder.
///
/// # Errors
/// Never in practice (every path is beneath `run_home`); `Internal` otherwise.
pub fn antigravity_guard(run_home: &Path, gemini_inner: &Path) -> AppResult<Vec<GuardPath>> {
    [
        (&["GEMINI.md"][..], GuardKind::File),
        (&["config", "GEMINI.md"][..], GuardKind::File),
        (&["config", "AGENTS.md"][..], GuardKind::File),
        (&["config", "rules"][..], GuardKind::Dir),
    ]
    .into_iter()
    .map(|(rel, kind)| {
        let mut outer = run_home.join(".gemini");
        let mut inner = gemini_inner.to_path_buf();
        for part in rel {
            outer.push(part);
            inner.push(part);
        }
        guarded(run_home, &outer, inner, kind)
    })
    .collect()
}

/// What every other process of a sandbox that runs code an agent chose sees in place of the
/// sandbox home's Claude Code configuration and its Codex homes: an empty read-only folder
/// (SPEC-652 MEM-010). Its CI jobs and script watches get these masks, as its Docker engine does
/// (so a container's `-v` never reaches them); none of them needs either folder.
#[must_use]
pub fn sandbox_masks(sandbox_dir: &Path) -> Vec<GuardPath> {
    yulius_contracts::ptyhost::sandbox_masks(&sandbox_dir.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_keys_match_the_clis() {
        // This task's own memory folder (2.1.332).
        assert_eq!(
            claude_key(Path::new(
                "/var/lib/yulius/sandboxes/d62be476-f169-4960-b45c-0fce704d71f9/repo"
            )),
            "-var-lib-yulius-sandboxes-d62be476-f169-4960-b45c-0fce704d71f9-repo"
        );
        // A character outside the BMP is two UTF-16 units, so two dashes.
        assert_eq!(claude_key(Path::new("/a/é/😀")), "-a-----");
        // Long: cut at 200, then `-` and the 31-hash in base 36 (as the CLI's `Nle`).
        let long = format!("/{}", "a".repeat(250));
        let key = claude_key(Path::new(&long));
        let units: Vec<u16> = long.encode_utf16().collect();
        let mut hash: i32 = 0;
        for unit in &units {
            hash = (hash << 5)
                .wrapping_sub(hash)
                .wrapping_add(i32::from(*unit));
        }
        let (cut, suffix) = key.split_at(CLAUDE_KEY_MAX);
        assert_eq!(cut, format!("-{}", "a".repeat(199)));
        let suffix = suffix.strip_prefix('-').unwrap_or_default();
        assert_eq!(
            u64::from_str_radix(suffix, 36).ok(),
            Some(u64::from(hash.unsigned_abs()))
        );
    }

    /// A sandbox's sessions key their memory by its repository, from its path alone: the folder
    /// is the one Claude Code itself uses for the repository and its worktrees, and a worktree's
    /// `.git` (which the agent writes, and could make a FIFO) is never read.
    #[test]
    fn a_sandbox_keys_its_memory_by_its_repository() {
        let sandbox = Path::new("/var/lib/yulius/sandboxes/d62be476-f169-4960-b45c-0fce704d71f9");
        assert_eq!(sandbox_memory_base(sandbox), sandbox.join("repo"));
        assert_eq!(
            claude_memory_dir(
                Path::new("/home/alice/.claude"),
                &sandbox_memory_base(sandbox)
            ),
            Path::new(
                "/home/alice/.claude/projects/-var-lib-yulius-sandboxes-d62be476-f169-4960-b45c-0fce704d71f9-repo/memory"
            )
        );
    }

    /// MEM-010: a sandbox's engine, CI jobs and script watches see its home's `.claude` and its
    /// Codex homes as empty read-only folders.
    #[test]
    fn a_sandbox_masks_its_harness_folders_for_other_processes() {
        let masks = sandbox_masks(Path::new("/data/sandboxes/s1"));
        let shown: Vec<(&str, &str, GuardKind)> = masks
            .iter()
            .map(|p| (p.relative.as_str(), p.dest.as_str(), p.kind))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "home/.claude",
                    "/data/sandboxes/s1/home/.claude",
                    GuardKind::Mask
                ),
                ("codex", "/data/sandboxes/s1/codex", GuardKind::Mask),
            ]
        );
        assert!(masks.iter().all(|p| p.anchor == "/data/sandboxes/s1"));
    }

    #[test]
    fn claude_guards_pin_the_parents_and_protect_memory_and_instructions() -> AppResult<()> {
        let config = ClaudeConfig {
            anchor: Path::new("/data/users/u1/home"),
            outer: Path::new("/data/users/u1/home/.claude"),
            inner: Path::new("/home/alice/.claude"),
            key: "-k",
        };
        let guard = config.guard()?;
        let shown: Vec<(&str, &str, GuardKind)> = guard
            .iter()
            .map(|p| (p.relative.as_str(), p.dest.as_str(), p.kind))
            .collect();
        assert_eq!(
            shown,
            [
                (".claude", "/home/alice/.claude", GuardKind::Pin),
                (
                    ".claude/projects",
                    "/home/alice/.claude/projects",
                    GuardKind::Pin
                ),
                (
                    ".claude/projects/-k",
                    "/home/alice/.claude/projects/-k",
                    GuardKind::Pin
                ),
                (
                    ".claude/projects/-k/memory",
                    "/home/alice/.claude/projects/-k/memory",
                    GuardKind::Dir
                ),
                (
                    ".claude/CLAUDE.md",
                    "/home/alice/.claude/CLAUDE.md",
                    GuardKind::File
                ),
                (".claude/rules", "/home/alice/.claude/rules", GuardKind::Dir),
            ]
        );
        assert!(guard.iter().all(|p| p.anchor == "/data/users/u1/home"));
        assert_eq!(
            config.memory_dir(),
            Path::new("/home/alice/.claude/projects/-k/memory")
        );
        // A configuration outside its anchor cannot be guarded: the launch fails, never starts
        // unguarded (review round 1).
        let outside = ClaudeConfig {
            outer: Path::new("/elsewhere/.claude"),
            ..config
        };
        assert!(outside.guard().is_err());
        assert!(codex_guard(Path::new("/data/sandboxes/s1"), Path::new("/data/other/a1")).is_err());
        Ok(())
    }

    #[test]
    fn prepare_makes_the_memory_folder_without_following_links()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let anchor = root.path().join("s1");
        std::fs::create_dir_all(anchor.join("home"))?;
        let outer = anchor.join("home/.claude");
        let config = ClaudeConfig {
            anchor: &anchor,
            outer: &outer,
            inner: &outer,
            key: "-k",
        };
        config.prepare()?;
        assert!(outer.join("projects/-k/memory").is_dir());
        // A link on the way stops it, and nothing is made at its target.
        let target = root.path().join("target");
        std::fs::create_dir_all(&target)?;
        std::fs::remove_dir_all(outer.join("projects"))?;
        std::os::unix::fs::symlink(&target, outer.join("projects"))?;
        assert!(config.prepare().is_err());
        assert!(std::fs::read_dir(&target)?.next().is_none());
        Ok(())
    }

    #[test]
    fn codex_and_antigravity_guard_their_instruction_files() -> AppResult<()> {
        let codex = codex_guard(
            Path::new("/data/sandboxes/s1"),
            Path::new("/data/sandboxes/s1/codex/a1"),
        )?;
        let shown: Vec<(&str, GuardKind)> = codex
            .iter()
            .map(|p| (p.relative.as_str(), p.kind))
            .collect();
        assert_eq!(
            shown,
            [
                ("codex/a1/AGENTS.md", GuardKind::File),
                ("codex/a1/AGENTS.override.md", GuardKind::File),
                ("codex/a1/memories", GuardKind::Dir),
            ]
        );
        assert!(
            codex
                .iter()
                .all(|p| p.dest == format!("/data/sandboxes/s1/{}", p.relative))
        );
        let agy = antigravity_guard(
            Path::new("/data/runs/r1/antigravity-home"),
            Path::new("/data/sandboxes/s1/home/.gemini"),
        )?;
        let shown: Vec<(&str, &str)> = agy
            .iter()
            .map(|p| (p.relative.as_str(), p.dest.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    ".gemini/GEMINI.md",
                    "/data/sandboxes/s1/home/.gemini/GEMINI.md"
                ),
                (
                    ".gemini/config/GEMINI.md",
                    "/data/sandboxes/s1/home/.gemini/config/GEMINI.md"
                ),
                (
                    ".gemini/config/AGENTS.md",
                    "/data/sandboxes/s1/home/.gemini/config/AGENTS.md"
                ),
                (
                    ".gemini/config/rules",
                    "/data/sandboxes/s1/home/.gemini/config/rules"
                ),
            ]
        );
        assert!(
            agy.iter()
                .all(|p| p.anchor == "/data/runs/r1/antigravity-home")
        );
        Ok(())
    }

    #[test]
    fn the_sentence_says_why_and_where_knowledge_goes() {
        assert!(READ_ONLY_SENTENCE.contains("read-only"));
        assert!(READ_ONLY_SENTENCE.contains("repository through a pull request"));
        assert_eq!(READ_ONLY_SENTENCE.matches(". ").count(), 0, "one sentence");
    }
}
