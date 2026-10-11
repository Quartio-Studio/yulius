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

/// Whether `path` (resolved) is visible inside containment.
fn visible_when_contained(path: &Path, extra: &[PathBuf]) -> bool {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    yulius_ptyhost::SYSTEM_READONLY_DIRS
        .iter()
        .map(Path::new)
        .chain(extra.iter().map(PathBuf::as_path))
        .any(|root| real.starts_with(root))
}

fn containment_visibility(contained: bool, extra: &[PathBuf]) -> Vec<Check> {
    if !contained {
        return Vec::new();
    }
    ["claude", "git"]
        .into_iter()
        .filter_map(|program| which(program).map(|path| (program, path)))
        .filter(|(_, path)| !visible_when_contained(path, extra))
        .map(|(program, path)| {
            check(
                "contained visibility",
                Level::Warn,
                format!(
                    "`{program}` resolves to {} which contained runs cannot see; install it under /usr/local or /opt or add its directories with --contained-readonly",
                    path.canonicalize().unwrap_or(path).display()
                ),
            )
        })
        .collect()
}

/// Runs every check. `contained` requires bubblewrap; `extra` are the configured
/// `--contained-readonly` paths.
#[must_use]
pub fn run_checks(data_dir: &Path, contained: bool, extra: &[PathBuf]) -> Vec<Check> {
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
    checks.extend(browser_display_checks());
    checks.extend(containment_visibility(contained, extra));
    if contained && cfg!(target_os = "linux") {
        checks.extend(isolation_checks(data_dir));
    }
    checks
}

/// Sandbox isolation (SPEC-002 ADR-22): the namespace holder's prerequisites, the base, the
/// identity module and the cgroup delegation. Missing pieces are warnings: contained runs then
/// keep the previous containment.
fn isolation_checks(data_dir: &Path) -> Vec<Check> {
    use crate::wiring::sandbox::{IsolationArgs, resolve};
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("yulius"));
    let mut checks = Vec::new();
    match resolve(&IsolationArgs::default(), data_dir, &exe, 8081) {
        Ok(config) => checks.push(check(
            "sandbox isolation",
            Level::Ok,
            format!(
                "namespace holder available (ids {}–{} mapped, egress through slirp4netns)",
                config.uid.sub_start,
                config.uid.sub_start + config.uid.count - 1
            ),
        )),
        Err(missing) => checks.push(check(
            "sandbox isolation",
            Level::Warn,
            format!(
                "previous containment (no apt, shared network, no limits): {}",
                missing.join("; ")
            ),
        )),
    }
    let bases = yulius_sandbox::base::Bases::new(data_dir);
    match bases.current() {
        Some(base) => {
            let fresh = base.fingerprint
                == yulius_sandbox::base::fingerprint(
                    Path::new("/"),
                    yulius_app::identity::SUDO_SHIM,
                );
            checks.push(check(
                "sandbox base",
                Level::Ok,
                format!(
                    "{}{}",
                    base.id,
                    if fresh {
                        ""
                    } else {
                        " (the host changed; the server rebuilds it)"
                    }
                ),
            ));
        }
        None => checks.push(check(
            "sandbox base",
            Level::Warn,
            "none yet: the server builds it in the background, or run `yulius sandbox-base build` as the service user",
        )),
    }
    checks.push(delegation_check());
    checks
}

fn delegation_check() -> Check {
    if let Ok(found) = crate::wiring::sandbox::delegation_detect() {
        return check("sandbox limits", Level::Ok, format!("delegated: {found}"));
    }
    let unit = output_of(
        Path::new("systemctl"),
        &[
            "show",
            "yulius.service",
            "--property=Delegate,DelegateSubgroup",
        ],
    )
    .unwrap_or_default();
    if unit.contains("Delegate=yes") && unit.contains("DelegateSubgroup=server") {
        check(
            "sandbox limits",
            Level::Ok,
            "yulius.service delegates its cgroup (Delegate=yes, DelegateSubgroup=server)",
        )
    } else {
        check(
            "sandbox limits",
            Level::Warn,
            "no per-sandbox CPU, memory and task limits: add Delegate=yes and \
             DelegateSubgroup=server to yulius.service",
        )
    }
}

/// The virtual display's prerequisites (SPEC-002 ADR-15): Xvfb, and ffmpeg with x11grab and
/// libx264. Without them the sandbox browser runs headless and shows JPEG frames.
fn browser_display_checks() -> Vec<Check> {
    let headless = "the sandbox browser runs headless (JPEG frames, no real input for captchas)";
    let xvfb = match which("Xvfb") {
        Some(path) => check("Xvfb", Level::Ok, path.display().to_string()),
        None => check("Xvfb", Level::Warn, format!("not found; {headless}")),
    };
    let ffmpeg = match which("ffmpeg") {
        None => check("ffmpeg", Level::Warn, format!("not found; {headless}")),
        Some(path) => {
            let encoders = output_of(&path, &["-hide_banner", "-encoders"]).unwrap_or_default();
            let devices = output_of(&path, &["-hide_banner", "-devices"]).unwrap_or_default();
            let missing: Vec<&str> = [
                ("libx264", encoders.contains("libx264")),
                ("x11grab", devices.contains("x11grab")),
            ]
            .into_iter()
            .filter(|(_, present)| !present)
            .map(|(name, _)| name)
            .collect();
            if missing.is_empty() {
                check(
                    "ffmpeg",
                    Level::Ok,
                    format!("{} (libx264, x11grab)", path.display()),
                )
            } else {
                check(
                    "ffmpeg",
                    Level::Warn,
                    format!(
                        "{} lacks {}; {headless}",
                        path.display(),
                        missing.join(", ")
                    ),
                )
            }
        }
    };
    vec![xvfb, ffmpeg]
}

/// The whole stdout of a short command (at most 1 MiB), when it succeeds in time.
fn output_of(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on a thread so a large output never fills the pipe while we wait.
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = (&mut stdout).take(1 << 20).read_to_string(&mut out);
        out
    });
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    success.then_some(out)
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The reverse proxy's secret file (SPEC-001 ACC-013), when configured: usable or not, never
/// its content.
#[must_use]
pub fn proxy_secret_check(path: &Path) -> Check {
    match crate::wiring::proxy::read_proxy_secret(path) {
        Ok(_) => check(
            "proxy secret",
            Level::Ok,
            format!(
                "{} (owner-only); sign-in limits trust the proxy's client address",
                path.display()
            ),
        ),
        Err(err) => check(
            "proxy secret",
            Level::Fail,
            format!("{err}; sign-in limits would key on the TCP peer"),
        ),
    }
}

/// `yulius doctor`: prints the checks; exits 1 when any check fails.
pub fn doctor(
    data_dir: &Path,
    contained: bool,
    extra: &[PathBuf],
    proxy_secret_file: Option<&Path>,
) -> ExitCode {
    let mut checks = run_checks(data_dir, contained, extra);
    checks.extend(proxy_secret_file.map(proxy_secret_check));
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
    use super::{Level, proxy_secret_check, run_checks, version_of, visible_when_contained};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_proxy_secret_check_never_shows_the_secret() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let secret = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("proxy.secret");
        std::fs::write(&path, secret)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let ok = proxy_secret_check(&path);
        assert_eq!(ok.level, Level::Ok, "{}", ok.detail);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        let open = proxy_secret_check(&path);
        assert_eq!(open.level, Level::Fail);
        assert!(open.detail.contains("mode 644"), "{}", open.detail);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        std::fs::write(&path, &secret[..20])?;
        let short = proxy_secret_check(&path);
        assert_eq!(short.level, Level::Fail);
        for c in [ok, open, short] {
            assert!(!c.detail.contains(&secret[..20]), "{}", c.detail);
        }
        Ok(())
    }

    #[test]
    fn containment_visibility_follows_system_dirs_and_extra_paths() {
        assert!(visible_when_contained(Path::new("/usr/bin/env"), &[]));
        assert!(!visible_when_contained(
            Path::new("/home/u/.local/bin/claude"),
            &[]
        ));
        assert!(visible_when_contained(
            Path::new("/home/u/.local/bin/claude"),
            &[PathBuf::from("/home/u/.local")]
        ));
    }

    #[test]
    fn reports_a_too_long_data_dir_and_missing_programs() {
        let long = Path::new("/tmp").join("x".repeat(80));
        let checks = run_checks(&long, false, &[]);
        let sockets = checks.iter().find(|c| c.name == "session socket paths");
        assert_eq!(sockets.map(|c| c.level), Some(Level::Fail));
        assert_eq!(
            version_of(Path::new("/nonexistent/yulius-test-binary"), &["--version"]),
            None
        );
        assert!(checks.iter().any(|c| c.name == "git"));
    }
}
