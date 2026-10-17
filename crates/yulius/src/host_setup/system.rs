//! The real [`Host`]: this machine's files and commands, run as root.

use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Cmd, Host, Out};

/// This machine. `quiet` keeps every command's output captured (for `--json`, where standard
/// output is the report).
pub struct RealHost {
    /// Never let a command's output through.
    pub quiet: bool,
}

/// Milliseconds since the epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Host for RealHost {
    fn read(&self, path: &Path) -> Option<String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(4 << 20)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn write(&mut self, path: &Path, content: &str, mode: u32) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        crate::wiring::update_files::write_atomic(path, content.as_bytes(), mode)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    fn stat(&self, path: &Path) -> Option<(u32, u32)> {
        std::fs::symlink_metadata(path)
            .ok()
            .map(|m| (m.uid(), m.mode()))
    }

    fn gid(&self, path: &Path) -> Option<u32> {
        std::fs::symlink_metadata(path).ok().map(|m| m.gid())
    }

    fn canonical(&self, path: &Path) -> Option<std::path::PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    /// Through one descriptor opened without following a link, so a link swapped in after a
    /// check never makes root change its target (review of SPEC-203).
    fn own_by_root(&mut self, path: &Path) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags((nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK).bits())
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = file
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        std::os::unix::fs::fchown(&file, Some(0), Some(0))
            .map_err(|e| format!("chown {}: {e}", path.display()))?;
        let mode = meta.mode() & 0o7777 & !0o022;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {}: {e}", path.display()))
    }

    fn fingerprint(&self) -> String {
        yulius_sandbox::base::fingerprint(Path::new("/"), yulius_app::identity::SUDO_SHIM)
    }

    fn run(&mut self, cmd: &Cmd) -> Result<Out, String> {
        let (program, args) = cmd.argv.split_first().ok_or("an empty command")?;
        let show = cmd.show && !self.quiet;
        let mut command = Command::new(program);
        // Its own process group, so a timeout ends what it started too (apt's dpkg).
        command.args(args).stdin(Stdio::null()).process_group(0);
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        if show {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        let reader = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.by_ref().take(1 << 20).read_to_string(&mut text);
                }
                text
            })
        };
        let out = reader(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let err = reader(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let deadline = Instant::now() + cmd.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    if let Ok(pid) = i32::try_from(child.id()) {
                        let _ = nix::sys::signal::killpg(
                            nix::unistd::Pid::from_raw(pid),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        Ok(match status {
            Some(status) => Out {
                code: status.code().unwrap_or(-1),
                stdout,
                stderr,
            },
            None => Out {
                code: -1,
                stdout,
                stderr: format!(
                    "{program} did not finish within {} s",
                    cmd.timeout.as_secs()
                ),
            },
        })
    }
}
