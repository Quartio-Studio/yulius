//! Claude Code global config (`<CLAUDE_CONFIG_DIR>/.claude.json`) prepared before each launch.
//!
//! Without these keys an isolated config dir starts with interactive first-run screens: the
//! theme picker, and for every new working directory a folder-trust prompt whose default answer
//! is "No, exit" (verified live with Claude Code 2.1.292). An unattended launch or a typed wake
//! notice would then exit the agent. The file is merged, never replaced, because Claude Code
//! keeps its own state there. The same preparation keeps Claude Code's diff side panel from opening
//! by itself (SPEC-024). A linked git worktree is trusted under its main repository too: Claude
//! Code 2.1.292 looks a worktree's trust up there when `.claude/settings.json` pre-approves tools
//! (SPEC-106, verified live).

use std::io::{self, Read};
use std::path::Path;

use serde_json::{Map, Value};

use crate::launch::PlannedFile;

/// File name inside the config dir.
pub const CLAUDE_GLOBAL_CONFIG: &str = ".claude.json";
/// Claude Code's preference for its diff side panel; `false` keeps the panel from opening itself.
const DIFF_SIDEBAR_OPEN: &str = "diffSidebarOpen";
/// Store ids of the built-in diff plugin: the current one, then the legacy one Claude Code carries
/// into it when the current store is missing.
const DIFF_PLUGIN_STORE_IDS: [&str; 2] = ["cc-plugin-diff@builtin", "diff@builtin"];
/// The diff plugin's store key for its pane; `true` opens the pane by itself.
const DIFF_PANE_OPEN: &str = "open";
/// Largest existing config read; a larger file is treated as corrupt and left untouched.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// Merges onboarding completion, trust for `worktree` (and, for a linked git worktree, its main
/// repository) and a closed diff side panel into the
/// Claude global config of `config_dir` (created 0700 when missing). `bypass_permissions` also
/// records acceptance of the `--dangerously-skip-permissions` warning (contained runs only).
/// Idempotent.
///
/// # Errors
/// I/O failures, or an existing config that is not a JSON object or is larger than 8 MiB.
pub fn ensure_claude_config(
    config_dir: &Path,
    worktree: &Path,
    bypass_permissions: bool,
) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(config_dir)?;
    let path = config_dir.join(CLAUDE_GLOBAL_CONFIG);
    let mut root = match std::fs::File::open(&path) {
        Ok(file) => {
            let mut text = String::new();
            file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut text)?;
            if text.len() as u64 > MAX_CONFIG_BYTES {
                return Err(io::Error::other("Claude config is larger than 8 MiB"));
            }
            if text.trim().is_empty() {
                Map::new()
            } else {
                match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Object(map)) => map,
                    _ => return Err(io::Error::other("Claude config is not a JSON object")),
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(err) => return Err(err),
    };
    let keys: Vec<String> = std::iter::once(worktree.to_path_buf())
        .chain(main_repository(worktree))
        .map(|path| {
            path.canonicalize()
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let before = root.clone();
    root.insert("hasCompletedOnboarding".into(), Value::Bool(true));
    root.entry("theme")
        .or_insert_with(|| Value::String("dark".into()));
    // Claude Code opens its diff side panel by itself in a wide terminal (from 144 columns, or
    // 110 once it was opened by hand), squeezing the transcript in the task view (SPEC-024).
    // `false` stops the auto-open on every launch; the panel still opens by hand in a session.
    root.insert(DIFF_SIDEBAR_OPEN.into(), Value::Bool(false));
    if bypass_permissions {
        root.insert("bypassPermissionsModeAccepted".into(), Value::Bool(true));
    }
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    if !projects.is_object() {
        *projects = Value::Object(Map::new());
    }
    if let Value::Object(projects) = projects {
        for key in keys {
            let project = projects
                .entry(key)
                .or_insert_with(|| Value::Object(Map::new()));
            if !project.is_object() {
                *project = Value::Object(Map::new());
            }
            if let Value::Object(project) = project {
                project.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
                project.insert("hasCompletedProjectOnboarding".into(), Value::Bool(true));
            }
        }
    }
    if root != before {
        let contents = serde_json::to_vec_pretty(&Value::Object(root)).map_err(io::Error::other)?;
        PlannedFile {
            path,
            contents,
            mode: 0o600,
        }
        .write()?;
    }
    close_diff_pane(config_dir);
    Ok(())
}

/// The main repository of a linked git worktree, read from its files as Git lays them out (no
/// `git` process): `<worktree>/.git` is a file `gitdir: <admin dir>`, and `<admin dir>/commondir`
/// leads to the common git directory. That directory's parent when it is a `.git`, else the
/// directory itself (a bare repository). `None` for a main checkout, a plain directory or files
/// that do not read like that.
fn main_repository(worktree: &Path) -> Option<std::path::PathBuf> {
    // Regular files only: a FIFO planted in their place would block the launch.
    let small_file = |path: &Path| {
        path.metadata()
            .is_ok_and(|m| m.is_file())
            .then(|| read_small(path).ok().flatten())
            .flatten()
    };
    let dot_git = small_file(&worktree.join(".git"))?;
    let admin = dot_git.lines().next()?.strip_prefix("gitdir:")?.trim();
    let admin = worktree.join(admin);
    let common = small_file(&admin.join("commondir"))?;
    let common = admin
        .join(common.lines().next()?.trim())
        .canonicalize()
        .ok()?;
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

/// Stores `"open": false` in every existing store of Claude Code's built-in diff plugin
/// (SPEC-024). Since 2.1.292 that plugin draws the panel and keeps its own `"open"`, which `/diff`
/// sets to `true`; `diffSidebarOpen` only seeds a store that does not exist yet. A missing store
/// is left missing, and an unreadable one is left to Claude Code: the panel is cosmetic and never
/// blocks a launch.
fn close_diff_pane(config_dir: &Path) {
    let store = config_dir.join("plugins").join("store");
    for id in DIFF_PLUGIN_STORE_IDS {
        let path = store.join(plugin_store_file(id));
        let Some(Value::Object(mut map)) = read_small(&path)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        if map.get(DIFF_PANE_OPEN) == Some(&Value::Bool(false)) {
            continue;
        }
        map.insert(DIFF_PANE_OPEN.into(), Value::Bool(false));
        if let Ok(mut contents) = serde_json::to_vec_pretty(&Value::Object(map)) {
            contents.push(b'\n');
            // Best effort, like the read: a failed write leaves the pane as Claude Code had it.
            let _ = PlannedFile {
                path,
                contents,
                mode: 0o600,
            }
            .write();
        }
    }
}

/// File name Claude Code gives a plugin store: the id itself when it is a plain lowercase name,
/// else the id with other characters replaced by `_` and the first 12 hex digits of its SHA-256.
fn plugin_store_file(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let plain = !id.is_empty()
        && id.len() <= 64
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if plain {
        return format!("{id}.json");
    }
    let name: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{name}-{hex}.json")
}

/// The file at `path` when it exists and fits in [`MAX_CONFIG_BYTES`].
fn read_small(path: &Path) -> io::Result<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut text)?;
    Ok((text.len() as u64 <= MAX_CONFIG_BYTES).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::{CLAUDE_GLOBAL_CONFIG, ensure_claude_config};
    use serde_json::{Value, json};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn merges_trust_and_onboarding_and_keeps_existing_state() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join(".claude");
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree)?;
        std::fs::create_dir_all(&config)?;
        std::fs::write(
            config.join(CLAUDE_GLOBAL_CONFIG),
            r#"{"userID":"abc","theme":"light","projects":{"/other":{"hasTrustDialogAccepted":true}}}"#,
        )?;
        ensure_claude_config(&config, &worktree, false)?;
        let text = std::fs::read_to_string(config.join(CLAUDE_GLOBAL_CONFIG))?;
        let value: Value = serde_json::from_str(&text)?;
        let key = worktree.canonicalize()?.to_string_lossy().into_owned();
        assert_eq!(value["userID"], "abc");
        assert_eq!(value["theme"], "light", "user choices are kept");
        assert_eq!(value["hasCompletedOnboarding"], true);
        assert_eq!(value["projects"]["/other"]["hasTrustDialogAccepted"], true);
        assert_eq!(
            value["projects"][key.as_str()]["hasTrustDialogAccepted"],
            true
        );
        assert!(value.get("bypassPermissionsModeAccepted").is_none());
        ensure_claude_config(&config, &worktree, true)?;
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(config.join(CLAUDE_GLOBAL_CONFIG))?)?;
        assert_eq!(value["bypassPermissionsModeAccepted"], true);
        Ok(())
    }

    fn trusted(
        config: &std::path::Path,
        path: &std::path::Path,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(config.join(CLAUDE_GLOBAL_CONFIG))?)?;
        let key = path.canonicalize()?.to_string_lossy().into_owned();
        Ok(value["projects"][key.as_str()]["hasTrustDialogAccepted"] == json!(true))
    }

    /// A linked worktree laid out as `git worktree add` does: `<worktree>/.git` names its admin
    /// directory inside the main repository's `.git`, whose `commondir` leads back to it.
    fn linked_worktree(
        root: &std::path::Path,
        common: &std::path::Path,
        gitdir_line: impl Fn(&std::path::Path) -> String,
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let admin = common.join("worktrees").join("w1");
        std::fs::create_dir_all(&admin)?;
        std::fs::write(admin.join("commondir"), "../..\n")?;
        let worktree = root.join("worktrees").join("w1");
        std::fs::create_dir_all(&worktree)?;
        std::fs::write(worktree.join(".git"), gitdir_line(&admin))?;
        Ok(worktree)
    }

    /// SPEC-106 OTR-001 (#425): Claude Code 2.1.292 looks up the trust of a linked worktree under
    /// its main repository when `.claude/settings.json` pre-approves tools, so both are trusted.
    #[test]
    fn a_linked_worktree_trusts_its_main_repository_too() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join(".claude");
        let repo = dir.path().join("repo");
        let common = repo.join(".git");
        let worktree = linked_worktree(dir.path(), &common, |admin| {
            format!("gitdir: {}\n", admin.display())
        })?;
        ensure_claude_config(&config, &worktree, false)?;
        assert!(trusted(&config, &worktree)?, "the worktree");
        assert!(trusted(&config, &repo)?, "its main repository");
        // A relative `gitdir:` (`git worktree add --relative-paths`) leads to the same repository.
        let config = dir.path().join(".claude-relative");
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../../repo/.git/worktrees/w1\n",
        )?;
        ensure_claude_config(&config, &worktree, false)?;
        assert!(trusted(&config, &repo)?, "its main repository");
        Ok(())
    }

    /// The same with the layout the real `git worktree add` writes (Yulius's task worktrees).
    #[test]
    fn a_worktree_made_by_git_trusts_its_main_repository() -> TestResult {
        let dir = tempfile::tempdir()?;
        let repo = dir.path().join("repo");
        let worktree = dir.path().join("worktrees").join("task");
        let git = |args: &[&str]| -> TestResult {
            // Inherited Git variables (a hook's `GIT_DIR`) would point elsewhere.
            let status = std::process::Command::new("git")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_COMMON_DIR")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                ])
                .args(args)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("git {args:?}: {status}").into())
            }
        };
        let repo_text = repo.to_string_lossy().into_owned();
        git(&["init", "-q", &repo_text])?;
        git(&[
            "-C",
            &repo_text,
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ])?;
        git(&[
            "-C",
            &repo_text,
            "worktree",
            "add",
            "-q",
            "-b",
            "task",
            &worktree.to_string_lossy(),
        ])?;
        let config = dir.path().join(".claude");
        ensure_claude_config(&config, &worktree, false)?;
        assert!(trusted(&config, &worktree)?);
        assert!(trusted(&config, &repo)?);
        Ok(())
    }

    #[test]
    fn a_worktree_of_a_bare_repository_trusts_the_repository_itself() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join(".claude");
        let bare = dir.path().join("origin.git");
        let worktree = linked_worktree(dir.path(), &bare, |admin| {
            format!("gitdir: {}\n", admin.display())
        })?;
        ensure_claude_config(&config, &worktree, false)?;
        assert!(trusted(&config, &worktree)?);
        assert!(trusted(&config, &bare)?);
        Ok(())
    }

    #[test]
    fn an_unreadable_git_file_trusts_the_worktree_alone() -> TestResult {
        let dir = tempfile::tempdir()?;
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree)?;
        for (n, contents) in ["not a gitdir line\n", "gitdir: /nowhere/at/all\n", ""]
            .into_iter()
            .enumerate()
        {
            let config = dir.path().join(format!(".claude-{n}"));
            std::fs::write(worktree.join(".git"), contents)?;
            ensure_claude_config(&config, &worktree, false)?;
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(config.join(CLAUDE_GLOBAL_CONFIG))?)?;
            let projects = value["projects"].as_object().ok_or("projects")?;
            assert_eq!(projects.len(), 1, "{contents:?}: {projects:?}");
            assert!(trusted(&config, &worktree)?);
        }
        Ok(())
    }

    #[test]
    fn every_launch_starts_with_the_diff_side_panel_closed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join(".claude");
        let read = |config: &std::path::Path| -> Result<Value, Box<dyn std::error::Error>> {
            Ok(serde_json::from_str(&std::fs::read_to_string(
                config.join(CLAUDE_GLOBAL_CONFIG),
            )?)?)
        };
        ensure_claude_config(&config, dir.path(), false)?;
        assert_eq!(read(&config)?["diffSidebarOpen"], json!(false));
        // Opening the panel by hand in a session writes `true`; the next launch closes it again.
        std::fs::write(
            config.join(CLAUDE_GLOBAL_CONFIG),
            r#"{"userID":"abc","diffSidebarOpen":true}"#,
        )?;
        ensure_claude_config(&config, dir.path(), false)?;
        let value = read(&config)?;
        assert_eq!(value["diffSidebarOpen"], json!(false));
        assert_eq!(value["userID"], "abc");
        Ok(())
    }

    #[test]
    fn a_diff_pane_opened_by_hand_is_closed_at_the_next_launch() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join(".claude");
        let store = config.join("plugins").join("store");
        // Missing stores stay missing: Claude Code seeds them from `diffSidebarOpen`.
        ensure_claude_config(&config, dir.path(), false)?;
        assert!(!store.exists());
        // `/diff` in a session stores `"open": true` for the diff plugin (and its legacy id).
        std::fs::create_dir_all(&store)?;
        let current = store.join("cc-plugin-diff_builtin-3903e77c01b1.json");
        let legacy = store.join("diff_builtin-c6405f503984.json");
        std::fs::write(&current, r#"{"open":true,"baseMode":"branch"}"#)?;
        std::fs::write(&legacy, r#"{"open":true}"#)?;
        ensure_claude_config(&config, dir.path(), false)?;
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&current)?)?;
        assert_eq!(value["open"], json!(false));
        assert_eq!(value["baseMode"], "branch", "other plugin state is kept");
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&legacy)?)?;
        assert_eq!(value["open"], json!(false));
        // A store Yulius cannot read is Claude Code's to report; the launch goes on.
        std::fs::write(&current, "not json")?;
        ensure_claude_config(&config, dir.path(), false)?;
        assert_eq!(std::fs::read_to_string(&current)?, "not json");
        Ok(())
    }

    #[test]
    fn creates_a_missing_config_and_refuses_corrupt_ones() -> TestResult {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("cfg");
        ensure_claude_config(&config, dir.path(), false)?;
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(config.join(CLAUDE_GLOBAL_CONFIG))?)?;
        assert_eq!(value["hasCompletedOnboarding"], json!(true));
        assert_eq!(value["theme"], "dark");
        std::fs::write(config.join(CLAUDE_GLOBAL_CONFIG), "[1,2]")?;
        assert!(ensure_claude_config(&config, dir.path(), false).is_err());
        Ok(())
    }
}
