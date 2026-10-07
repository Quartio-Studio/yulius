//! Locating the Chromium executable (an external prerequisite on the VM).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::BrowserError;
use crate::process::{is_executable, resolve_program};

/// Program names searched on `PATH`, in order.
pub const CHROMIUM_NAMES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];

/// Fixed install locations tried after `PATH` (macOS application bundles).
pub const CHROMIUM_FIXED_PATHS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];

/// Finds Chromium on `PATH` or in the fixed locations.
///
/// # Errors
/// [`BrowserError::ExecutableNotFound`] listing everything that was tried.
pub fn discover_chromium() -> Result<PathBuf, BrowserError> {
    let fixed: Vec<PathBuf> = CHROMIUM_FIXED_PATHS.iter().map(PathBuf::from).collect();
    discover_in(std::env::var_os("PATH").as_deref(), &fixed)
}

/// [`discover_chromium`] with an explicit `PATH` value and fixed candidates (testable).
///
/// # Errors
/// [`BrowserError::ExecutableNotFound`] listing everything that was tried.
pub fn discover_in(path_var: Option<&OsStr>, fixed: &[PathBuf]) -> Result<PathBuf, BrowserError> {
    CHROMIUM_NAMES
        .iter()
        .find_map(|name| resolve_program(Path::new(name), path_var))
        .or_else(|| fixed.iter().find(|path| is_executable(path)).cloned())
        .ok_or_else(|| BrowserError::ExecutableNotFound {
            searched: searched_list(fixed),
        })
}

/// Resolves the configured executable: a path is checked, a bare name is searched on `PATH`.
///
/// # Errors
/// [`BrowserError::ExecutableNotFound`] when it does not exist or is not executable.
pub fn resolve_executable(configured: &Path) -> Result<PathBuf, BrowserError> {
    resolve_program(configured, std::env::var_os("PATH").as_deref()).ok_or_else(|| {
        BrowserError::ExecutableNotFound {
            searched: configured.display().to_string(),
        }
    })
}

fn searched_list(fixed: &[PathBuf]) -> String {
    CHROMIUM_NAMES
        .iter()
        .map(|name| format!("{name} on PATH"))
        .chain(fixed.iter().map(|path| path.display().to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write_file(path: &Path, mode: u32) -> TestResult {
        std::fs::write(path, "#!/bin/sh\n")?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        Ok(())
    }

    #[test]
    fn finds_first_executable_name_on_path() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        write_file(&first.path().join("google-chrome"), 0o644)?;
        write_file(&second.path().join("google-chrome"), 0o755)?;
        write_file(&second.path().join("chromium-browser"), 0o755)?;
        let path_var = std::env::join_paths([first.path(), second.path()])?;
        let found = discover_in(Some(&path_var), &[])?;
        assert_eq!(found, second.path().join("chromium-browser"));
        Ok(())
    }

    #[test]
    fn falls_back_to_fixed_paths_then_reports_what_was_tried() -> TestResult {
        let dir = tempfile::tempdir()?;
        let app = dir.path().join("Google Chrome");
        write_file(&app, 0o755)?;
        let empty = tempfile::tempdir()?;
        let path_var = std::env::join_paths([empty.path()])?;
        assert_eq!(
            discover_in(Some(&path_var), std::slice::from_ref(&app))?,
            app
        );
        let missing = dir.path().join("missing");
        match discover_in(Some(&path_var), &[missing]) {
            Err(BrowserError::ExecutableNotFound { searched }) => {
                assert!(searched.contains("chromium on PATH"), "{searched}");
                assert!(searched.contains("missing"), "{searched}");
            }
            other => return Err(format!("unexpected {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn explicit_paths_must_be_executable() -> TestResult {
        let dir = tempfile::tempdir()?;
        let plain = dir.path().join("chrome");
        write_file(&plain, 0o644)?;
        assert!(matches!(
            resolve_executable(&plain),
            Err(BrowserError::ExecutableNotFound { .. })
        ));
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755))?;
        assert_eq!(resolve_executable(&plain)?, plain);
        Ok(())
    }
}
