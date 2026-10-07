//! `yulius doctor`: checks the prerequisites of a Yulius host without changing anything.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// Deadline for one external `--version` call.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Satisfied.
    Ok,
    /// Optional feature unavailable or degraded.
    Warn,
    /// Yulius cannot work correctly.
    Fail,
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: &'static str,
    /// Result.
    pub level: Level,
    /// Human-readable detail (never secrets).
    pub detail: String,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
    }
}

/// First line of `<program> <args>` output, bounded in time and size.
fn version_of(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child
        .stdout
        .take()?
        .take(64 * 1024)
        .read_to_string(&mut out)
        .ok()?;
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

fn data_dir_checks(data_dir: &Path) -> Vec<Check> {
    use std::os::unix::fs::PermissionsExt;
    let mut checks = Vec::new();
    match std::fs::metadata(data_dir) {
        Ok(meta) if meta.is_dir() => {
            let mode = meta.permissions().mode() & 0o777;
            checks.push(if mode & 0o077 == 0 {
                check(
                    "data directory",
                    Level::Ok,
                    format!("{} (mode {mode:o})", data_dir.display()),
                )
            } else {
                check(
                    "data directory",
                    Level::Warn,
                    format!(
                        "{} is mode {mode:o}; `yulius serve` restricts it to 0700",
                        data_dir.display()
                    ),
                )
            });
        }
        Ok(_) => checks.push(check(
            "data directory",
            Level::Fail,
            format!("{} is not a directory", data_dir.display()),
        )),
        Err(_) => checks.push(check(
            "data directory",
            Level::Warn,
            format!(
                "{} does not exist yet; `yulius serve` creates it",
                data_dir.display()
            ),
        )),
    }
    checks.push(match crate::serve::check_socket_paths(data_dir) {
        Ok(()) => check(
            "session socket paths",
            Level::Ok,
            "within the Unix socket path limit",
        ),
        Err(err) => check("session socket paths", Level::Fail, err),
    });
    checks
}

/// Runs every check. `contained` requires bubblewrap.
#[must_use]
pub fn run_checks(data_dir: &Path, contained: bool) -> Vec<Check> {
    let mut checks = data_dir_checks(data_dir);
    checks.push(match version_of(Path::new("git"), &["--version"]) {
        Some(v) => check("git", Level::Ok, v),
        None => check("git", Level::Fail, "`git` not found on PATH"),
    });
    checks.push(match version_of(Path::new("claude"), &["--version"]) {
        Some(v) => check("Claude Code", Level::Ok, v),
        None => check(
            "Claude Code",
            Level::Warn,
            "`claude --version` failed or `claude` is not on PATH; Claude tasks cannot start",
        ),
    });
    match version_of(Path::new("codex"), &["--version"]) {
        Some(v) => {
            let scratch =
                std::env::temp_dir().join(format!("yulius-doctor-{}", std::process::id()));
            let features = yulius_harness::probe::probe_codex(Path::new("codex"), &scratch);
            let _ = std::fs::remove_dir_all(&scratch);
            let detail = if features.no_daemon {
                format!("{v} (supports --no-daemon)")
            } else {
                format!(
                    "{v} (no --no-daemon: sessions may use a shared daemon; upgrade recommended)"
                )
            };
            checks.push(check(
                "Codex",
                if features.no_daemon {
                    Level::Ok
                } else {
                    Level::Warn
                },
                detail,
            ));
        }
        None => checks.push(check(
            "Codex",
            Level::Warn,
            "`codex --version` failed or `codex` is not on PATH; Codex tasks cannot start",
        )),
    }
    let bwrap = version_of(Path::new(yulius_ptyhost::BWRAP_PROGRAM), &["--version"]);
    checks.push(match (bwrap, contained) {
        (Some(v), _) => check("bubblewrap", Level::Ok, v),
        (None, true) => check(
            "bubblewrap",
            Level::Fail,
            "containment requested but `bwrap` is not available",
        ),
        (None, false) => check(
            "bubblewrap",
            Level::Warn,
            "not available; runs are not contained",
        ),
    });
    checks.push(match yulius_browser::discover_chromium() {
        Ok(path) => check("Chromium", Level::Ok, path.display().to_string()),
        Err(_) => check(
            "Chromium",
            Level::Warn,
            "not found; the sandbox browser is unavailable",
        ),
    });
    checks.push(match which("openvscode-server") {
        Some(path) => check("openvscode-server", Level::Ok, path.display().to_string()),
        None => check(
            "openvscode-server",
            Level::Warn,
            "not found; the editor panel is unavailable",
        ),
    });
    checks
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// `yulius doctor`: prints the checks; exits 1 when any check fails.
pub fn doctor(data_dir: &Path, contained: bool) -> ExitCode {
    let checks = run_checks(data_dir, contained);
    for c in &checks {
        let tag = match c.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        println!("{tag}  {:<22} {}", c.name, c.detail);
    }
    if checks.iter().any(|c| c.level == Level::Fail) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, run_checks, version_of};
    use std::path::Path;

    #[test]
    fn reports_a_too_long_data_dir_and_missing_programs() {
        let long = Path::new("/tmp").join("x".repeat(80));
        let checks = run_checks(&long, false);
        let sockets = checks.iter().find(|c| c.name == "session socket paths");
        assert_eq!(sockets.map(|c| c.level), Some(Level::Fail));
        assert_eq!(
            version_of(Path::new("/nonexistent/yulius-test-binary"), &["--version"]),
            None
        );
        assert!(checks.iter().any(|c| c.name == "git"));
    }
}
