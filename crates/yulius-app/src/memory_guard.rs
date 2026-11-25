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

/// The one sentence a refused write is explained with (SPEC-652 MEM-006).
pub const READ_ONLY_SENTENCE: &str = "Yulius keeps this harness's memory and user-level \
     instructions read-only (the owner's decision of 2026-11-25): put lasting knowledge in the \
     repository through a pull request.";

/// Longest Claude Code key kept whole; a longer one is cut there and ends in a hash (2.1.332).
const CLAUDE_KEY_MAX: usize = 200;

/// Largest `.git` or `commondir` file read to find a worktree's main repository.
const GIT_POINTER_MAX: u64 = 4096;

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

/// Reads a small file without following a link at its last component.
fn read_pointer(path: &Path) -> Option<String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(GIT_POINTER_MAX).read_to_string(&mut text).ok()?;
    Some(text)
}

/// `path` with `.` and `..` resolved lexically; `None` when it is not absolute or climbs past
/// the root.
fn normalized(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() || out.as_os_str().is_empty() {
                    return None;
                }
            }
            Component::Prefix(_) => return None,
        }
    }
    out.is_absolute().then_some(out)
}

/// The folder Claude Code keys the memory of a session in `cwd` by: the git root of `cwd`, a
/// worktree's main repository, else `cwd` itself. Only `.git` pointers that stay inside `within`
/// (the sandbox) are followed, and no link is: a session whose repository cannot be read keys by
/// its worktree, which `autoMemoryDirectory` then names for it (MEM-002).
#[must_use]
pub fn claude_project_base(cwd: &Path, within: &Path) -> PathBuf {
    let inside = |path: &Path| path.starts_with(within);
    let mut dir = Some(cwd);
    while let Some(here) = dir.filter(|d| inside(d)) {
        let dot_git = here.join(".git");
        match std::fs::symlink_metadata(&dot_git) {
            Ok(meta) if meta.is_dir() => return here.to_path_buf(),
            Ok(meta) if meta.is_file() => {
                return main_repository(&dot_git, within).unwrap_or_else(|| here.to_path_buf());
            }
            _ => dir = here.parent(),
        }
    }
    cwd.to_path_buf()
}

/// The main repository of the worktree whose `.git` file is `dot_git` (`gitdir: <repo>/.git/
/// worktrees/<name>`, whose `commondir` names `<repo>/.git`).
fn main_repository(dot_git: &Path, within: &Path) -> Option<PathBuf> {
    let text = read_pointer(dot_git)?;
    let gitdir = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    let gitdir = normalized(&dot_git.parent()?.join(gitdir))?;
    let common = read_pointer(&gitdir.join("commondir"))?;
    let common = normalized(&gitdir.join(common.lines().next()?.trim()))?;
    (common.starts_with(within) && common.file_name() == Some(OsStr::new(".git")))
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
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
fn guarded(anchor: &Path, outer: &Path, inner: PathBuf, kind: GuardKind) -> Option<GuardPath> {
    Some(GuardPath {
        anchor: anchor.to_string_lossy().into_owned(),
        relative: below(anchor, outer)?.to_string_lossy().into_owned(),
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
    /// `rules`. Empty when `outer` is not beneath `anchor`.
    #[must_use]
    pub fn guard(&self) -> Vec<GuardPath> {
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
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
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

/// The guarded paths of a Codex home `codex_home` beneath `anchor`, bound at the same path.
#[must_use]
pub fn codex_guard(anchor: &Path, codex_home: &Path) -> Vec<GuardPath> {
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
    .collect::<Option<Vec<_>>>()
    .unwrap_or_default()
}

/// The guarded paths of an Antigravity run whose home is `run_home` (its `.gemini` bound at
/// `gemini_inner`), opened from `run_home`, the run's own folder.
#[must_use]
pub fn antigravity_guard(run_home: &Path, gemini_inner: &Path) -> Vec<GuardPath> {
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
    .collect::<Option<Vec<_>>>()
    .unwrap_or_default()
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

    /// A worktree's memory is keyed by its main repository; a plain folder by itself; a pointer
    /// out of the sandbox or through a link is not followed.
    #[test]
    fn a_worktree_keys_by_its_main_repository() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let sandbox = root.path().join("s1");
        let repo = sandbox.join("repo");
        let worktree = sandbox.join("worktrees/t1");
        let admin = repo.join(".git/worktrees/t1");
        std::fs::create_dir_all(&admin)?;
        std::fs::create_dir_all(worktree.join("src"))?;
        std::fs::write(admin.join("commondir"), "../..\n")?;
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )?;
        assert_eq!(claude_project_base(&worktree.join("src"), &sandbox), repo);
        assert_eq!(claude_project_base(&repo, &sandbox), repo);
        let plain = sandbox.join("home");
        std::fs::create_dir_all(&plain)?;
        assert_eq!(claude_project_base(&plain, &sandbox), plain);
        // A pointer out of the sandbox: the worktree itself.
        let outside = root.path().join("other/.git/worktrees/t1");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("commondir"), "../..\n")?;
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", outside.display()),
        )?;
        assert_eq!(claude_project_base(&worktree, &sandbox), worktree);
        // A `.git` that is a link is not read.
        std::fs::remove_file(worktree.join(".git"))?;
        std::os::unix::fs::symlink(repo.join(".git"), worktree.join(".git"))?;
        assert_eq!(claude_project_base(&worktree, &sandbox), worktree);
        Ok(())
    }

    #[test]
    fn claude_guards_pin_the_parents_and_protect_memory_and_instructions() {
        let config = ClaudeConfig {
            anchor: Path::new("/data/users/u1/home"),
            outer: Path::new("/data/users/u1/home/.claude"),
            inner: Path::new("/home/alice/.claude"),
            key: "-k",
        };
        let guard = config.guard();
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
        let outside = ClaudeConfig {
            outer: Path::new("/elsewhere/.claude"),
            ..config
        };
        assert!(outside.guard().is_empty());
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
    fn codex_and_antigravity_guard_their_instruction_files() {
        let codex = codex_guard(
            Path::new("/data/sandboxes/s1"),
            Path::new("/data/sandboxes/s1/codex/a1"),
        );
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
        );
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
    }

    #[test]
    fn the_sentence_says_why_and_where_knowledge_goes() {
        assert!(READ_ONLY_SENTENCE.contains("read-only"));
        assert!(READ_ONLY_SENTENCE.contains("repository through a pull request"));
        assert_eq!(READ_ONLY_SENTENCE.matches(". ").count(), 0, "one sentence");
    }
}
