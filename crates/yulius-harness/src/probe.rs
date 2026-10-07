//! Capability probes of the installed agent CLIs (flags differ between releases).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Deadline for one `--help` probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest help text read.
const MAX_HELP_BYTES: u64 = 256 * 1024;

/// Optional Codex CLI flags supported by the installed release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexFeatures {
    /// `--no-daemon` (keeps the session inside the spawned process; added after 0.154).
    pub no_daemon: bool,
}

impl Default for CodexFeatures {
    /// The newest known release.
    fn default() -> Self {
        Self { no_daemon: true }
    }
}

impl CodexFeatures {
    /// Features advertised by a `codex --help` text.
    #[must_use]
    pub fn from_help(help: &str) -> Self {
        Self {
            no_daemon: help
                .split_whitespace()
                .any(|word| word.trim_end_matches(',') == "--no-daemon"),
        }
    }
}

/// Runs `<program> --help` (with an empty `CODEX_HOME` so no user configuration is read) and
/// returns the advertised features. A failed or timed-out probe reports no optional features,
/// because passing an unknown flag makes the CLI exit immediately.
#[must_use]
pub fn probe_codex(program: &Path, scratch_home: &Path) -> CodexFeatures {
    let none = CodexFeatures { no_daemon: false };
    if std::fs::create_dir_all(scratch_home).is_err() {
        return none;
    }
    let Ok(mut child) = Command::new(program)
        .arg("--help")
        .env("CODEX_HOME", scratch_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return none;
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return none;
            }
        }
    }
    let mut help = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_HELP_BYTES).read_to_string(&mut help);
    }
    CodexFeatures::from_help(&help)
}

#[cfg(test)]
mod tests {
    use super::CodexFeatures;

    #[test]
    fn detects_no_daemon_in_help_text() {
        let new = "  -C, --cd <DIR>\n      --no-daemon\n          Run in process\n";
        let old = "      --no-alt-screen\n  -h, --help\n";
        assert!(CodexFeatures::from_help(new).no_daemon);
        assert!(!CodexFeatures::from_help(old).no_daemon);
        assert!(!CodexFeatures::from_help("--no-daemonize").no_daemon);
    }
}
