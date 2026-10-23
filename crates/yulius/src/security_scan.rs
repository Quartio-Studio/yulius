//! `yulius security-scan` (SPEC-195 VSC-001): Vidoc's vidoc-1 security scan of the repository in
//! the current directory, for every project's agents and CI jobs. The scanner is one stdlib-only
//! Python file, `scripts/vidoc_scan.py`, embedded here so each instance ships the scanner of its
//! release (ADR-0157); this process becomes `python3 -I -c <scanner> <args>`, so signals and the
//! exit code (0 passed, 1 failed, 2 not run, 3 incomplete) are the scanner's own.

use std::ffi::OsString;
use std::process::{Command, ExitCode};

/// The scanner's source.
pub const SCANNER: &str = include_str!("../../../scripts/vidoc_scan.py");

/// The interpreter (`YULIUS_PYTHON` overrides it, for a host whose `python3` is older than 3.11).
fn python() -> OsString {
    std::env::var_os("YULIUS_PYTHON").unwrap_or_else(|| "python3".into())
}

/// The scanner's exit code when Python cannot be started: not run (2), or the skip code in CI
/// mode, as the scanner itself answers.
fn not_run_code(args: &[OsString]) -> u8 {
    if args.iter().any(|a| a == "--ci") {
        println!(
            "[yulius] security scan skipped: it could not run (no python3), so nothing was scanned"
        );
        return std::env::var("YULIUS_CI_SKIP_CODE")
            .ok()
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
    }
    2
}

/// Runs the scanner with `args`.
#[must_use]
pub fn run(args: &[OsString]) -> ExitCode {
    let mut command = Command::new(python());
    command.arg("-I").arg("-c").arg(SCANNER).args(args);
    #[cfg(unix)]
    let err = {
        use std::os::unix::process::CommandExt;
        command.exec()
    };
    #[cfg(not(unix))]
    let err = match command.status() {
        Ok(status) => {
            return ExitCode::from(
                status
                    .code()
                    .and_then(|c| u8::try_from(c).ok())
                    .unwrap_or(1),
            );
        }
        Err(err) => err,
    };
    eprintln!("Security scan not run: python3 (3.11 or later) cannot be started: {err}");
    ExitCode::from(not_run_code(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single argument may hold at most 128 KiB on Linux (`MAX_ARG_STRLEN`).
    #[test]
    fn the_scanner_fits_in_one_argument() {
        assert!(SCANNER.len() < 100 * 1024, "{} bytes", SCANNER.len());
        assert!(SCANNER.contains("model.vidocsecurity.com"));
        assert!(!SCANNER.contains('\0'));
    }
}
