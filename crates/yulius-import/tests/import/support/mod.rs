//! Shared fixtures: temporary Git repositories with a GitHub-looking remote, synthetic session
//! stores and an in-process fake Yulius server.

pub mod server;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Window start used by the tests: 2026-09-21T14:13:20Z.
pub fn since() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

/// A recent timestamp (inside the window).
pub const RECENT: &str = "2026-10-01T10:00:00.000Z";
/// A later recent timestamp.
pub const RECENT_LATER: &str = "2026-10-02T11:30:00.000Z";
/// A timestamp before the window.
pub const OLD: &str = "2026-08-01T10:00:00.000Z";
/// The GitHub URL configured as `origin`.
pub const GITHUB_URL: &str = "https://github.com/Quartio-Studio/demo.git";

/// Runs `git` for fixture setup, isolated from the user's global configuration.
pub fn git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Yulius Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
            // Fixture repositories are throwaway: skip fsync and background maintenance.
            "-c",
            "core.fsync=none",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A working repository whose `origin` looks like GitHub but is rewritten (via
/// `url.<bare>.insteadOf`) to a local bare repository, with `main` pushed.
pub struct RepoFixture {
    pub temp: TempDir,
    pub bare: PathBuf,
    pub repo: PathBuf,
}

impl RepoFixture {
    pub fn new() -> TestResult<Self> {
        let temp = tempfile::tempdir()?;
        let bare = temp.path().join("remote.git");
        let repo = temp.path().join("work").join("demo");
        fs::create_dir_all(&bare)?;
        fs::create_dir_all(&repo)?;
        git(&bare, &["init", "--bare", "-q", "-b", "main"])?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        git(&repo, &["remote", "add", "origin", GITHUB_URL])?;
        let rewrite = format!("url.{}.insteadOf", bare.display());
        git(&repo, &["config", &rewrite, GITHUB_URL])?;
        write(&repo.join("README.md"), "# demo\n")?;
        write(&repo.join("b.txt"), "to be deleted\n")?;
        fs::create_dir_all(repo.join("dir"))?;
        write(&repo.join("dir").join("inner.txt"), "inner\n")?;
        git(&repo, &["add", "-A"])?;
        git(&repo, &["commit", "-q", "-m", "initial"])?;
        git(&repo, &["push", "-q", "origin", "main"])?;
        Ok(Self { temp, bare, repo })
    }

    /// The canonical repository path (what `discover_repo` reports).
    pub fn root(&self) -> TestResult<PathBuf> {
        Ok(fs::canonicalize(&self.repo)?)
    }

    pub fn commit_file(&self, name: &str, content: &str, message: &str) -> TestResult<String> {
        write(&self.repo.join(name), content)?;
        git(&self.repo, &["add", name])?;
        git(&self.repo, &["commit", "-q", "-m", message])?;
        git(&self.repo, &["rev-parse", "HEAD"])
    }
}

pub fn write(path: &Path, content: &str) -> TestResult {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

/// One Claude JSONL user/assistant line.
pub fn claude_line(kind: &str, cwd: &Path, timestamp: &str, content: serde_json::Value) -> String {
    serde_json::json!({
        "type": kind,
        "cwd": cwd.to_string_lossy(),
        "sessionId": "ignored-here",
        "timestamp": timestamp,
        "gitBranch": "main",
        "version": "2.1.292",
        "message": {"role": kind, "content": content},
    })
    .to_string()
}

/// Writes a Claude session file under `<claude>/projects/<slug>/<id>.jsonl`.
pub fn write_claude_session(
    claude: &Path,
    cwd: &Path,
    id: &str,
    lines: &[String],
) -> TestResult<PathBuf> {
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let path = claude
        .join("projects")
        .join(slug)
        .join(format!("{id}.jsonl"));
    write(&path, &(lines.join("\n") + "\n"))?;
    Ok(path)
}

/// A minimal Claude session: one real prompt and one assistant reply.
pub fn simple_claude_session(
    claude: &Path,
    cwd: &Path,
    id: &str,
    prompt: &str,
    start: &str,
    end: &str,
) -> TestResult<PathBuf> {
    write_claude_session(
        claude,
        cwd,
        id,
        &[
            claude_line("user", cwd, start, serde_json::json!(prompt)),
            claude_line(
                "assistant",
                cwd,
                end,
                serde_json::json!([{"type": "text", "text": "ok"}]),
            ),
        ],
    )
}
