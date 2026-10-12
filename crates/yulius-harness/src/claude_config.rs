//! Claude Code global config (`<CLAUDE_CONFIG_DIR>/.claude.json`) prepared before each launch.
//!
//! Without these keys an isolated config dir starts with interactive first-run screens: the
//! theme picker, and for every new working directory a folder-trust prompt whose default answer
//! is "No, exit" (verified live with Claude Code 2.1.292). An unattended launch or a typed wake
//! notice would then exit the agent. The file is merged, never replaced, because Claude Code
//! keeps its own state there.

use std::io::{self, Read};
use std::path::Path;

use serde_json::{Map, Value};

use crate::launch::PlannedFile;

/// File name inside the config dir.
pub const CLAUDE_GLOBAL_CONFIG: &str = ".claude.json";
/// Claude Code's preference for its diff side panel; `false` keeps the panel from opening itself.
const DIFF_SIDEBAR_OPEN: &str = "diffSidebarOpen";
/// Largest existing config read; a larger file is treated as corrupt and left untouched.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// Merges onboarding completion, trust for `worktree` and a closed diff side panel into the
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
    let key = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf())
        .to_string_lossy()
        .into_owned();
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
    if root == before {
        return Ok(());
    }
    let contents = serde_json::to_vec_pretty(&Value::Object(root)).map_err(io::Error::other)?;
    PlannedFile {
        path,
        contents,
        mode: 0o600,
    }
    .write()
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
