//! `yulius instance export` and `yulius instance import` (SPEC-329, ADR-0222): the whole
//! instance as one exact-size stream, read while the service is frozen, and installed on another
//! host from a file or a pipe.

use std::io::IsTerminal as _;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod archive;
pub mod export;
pub mod freeze;
pub mod walk;

/// The manifest's archive path, the first entry.
pub const MANIFEST: &[u8] = b"yulius-export.json";
/// The manifest's fixed length: its JSON padded with spaces, so the total can be written into it.
pub const MANIFEST_LEN: u64 = 8192;
/// The archive format this build writes and reads.
pub const FORMAT: u32 = 1;

/// Exit code of a command refused before it changed anything.
pub const REFUSED: u8 = 2;

/// The exported instance's facts (SPEC-329 EXP-005), the archive's first entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`FORMAT`].
    pub format: u32,
    /// The archive's exact length in bytes.
    pub total_bytes: u64,
    /// Entries under `data`.
    pub entries: u64,
    /// When the export started, seconds since the epoch.
    pub created_at: u64,
    /// The exporting build's commit.
    pub yulius_build: Option<String>,
    /// The newest database schema the exporting build knows.
    pub schema: u32,
    /// The data directory, which the copy must keep (absolute paths inside it).
    pub data_dir: String,
    /// The unit's `YULIUS_PUBLIC_URL`.
    pub public_url: Option<String>,
    /// The host's `ID VERSION_ID` from `/etc/os-release`.
    pub os_release: Option<String>,
    /// The service.
    pub service: Service,
    /// The service user's subordinate uids.
    pub subuid: Option<IdRange>,
    /// The service user's subordinate gids.
    pub subgid: Option<IdRange>,
}

/// The service and its user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    /// The systemd unit.
    pub unit: String,
    /// The user's name.
    pub user: String,
    /// Its uid.
    pub uid: u32,
    /// Its group.
    pub gid: u32,
}

/// A subordinate id range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdRange {
    /// First id.
    pub start: u32,
    /// How many.
    pub count: u32,
}

impl Manifest {
    /// Its archive content: the JSON padded with spaces to [`MANIFEST_LEN`].
    ///
    /// # Errors
    /// It does not fit.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let len = usize::try_from(MANIFEST_LEN).unwrap_or(usize::MAX);
        if bytes.len() >= len {
            return Err("the manifest is too long (a very long data directory or URL?)".into());
        }
        bytes.resize(len - 1, b' ');
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// The user called `uid` in `/etc/passwd`'s text, with their primary group.
#[must_use]
pub fn user_by_uid(passwd: &str, uid: u32) -> Option<(String, u32)> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 3 && f[2].parse::<u32>().ok()? == uid)
            .then(|| Some((f[0].to_owned(), f[3].parse().ok()?)))?
    })
}

/// The user `name` in `/etc/passwd`'s text: uid and primary group.
#[must_use]
pub fn user_by_name(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 3 && f[0] == name).then(|| Some((f[2].parse().ok()?, f[3].parse().ok()?)))?
    })
}

/// `ID VERSION_ID` of an `/etc/os-release` text.
#[must_use]
pub fn os_release(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_owned())
    };
    Some(format!(
        "{} {}",
        field("ID")?,
        field("VERSION_ID").unwrap_or_default()
    ))
}

/// Progress on stderr: a line rewritten in place on a terminal, else one every 30 seconds.
#[derive(Debug)]
pub struct Progress {
    verb: &'static str,
    total: u64,
    start: Instant,
    last: Option<Instant>,
    tty: bool,
}

impl Progress {
    /// Progress of `total` bytes, `verb` ("Exported") naming what is done.
    #[must_use]
    pub fn new(verb: &'static str, total: u64) -> Self {
        Self {
            verb,
            total,
            start: Instant::now(),
            last: None,
            tty: std::io::stderr().is_terminal(),
        }
    }

    /// The line for `done` bytes after `elapsed`.
    #[must_use]
    pub fn line(&self, done: u64, elapsed: Duration) -> String {
        let secs = elapsed.as_secs_f64().max(0.001);
        let rate = done as f64 / secs;
        let percent = if self.total == 0 {
            100.0
        } else {
            done as f64 * 100.0 / self.total as f64
        };
        let left = if rate >= 1.0 {
            let s = (self.total.saturating_sub(done) as f64 / rate) as u64;
            format!(", {}h{:02}m{:02}s left", s / 3600, s / 60 % 60, s % 60)
        } else {
            String::new()
        };
        format!(
            "{} {} of {} ({percent:.0}%), {}/s{left}",
            self.verb,
            yulius_import::human_bytes(done),
            yulius_import::human_bytes(self.total),
            yulius_import::human_bytes(rate as u64),
        )
    }

    /// Reports `done` bytes when it is time.
    pub fn tick(&mut self, done: u64) {
        let now = Instant::now();
        let every = if self.tty {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(30)
        };
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < every)
        {
            return;
        }
        self.last = Some(now);
        let line = self.line(done, now.duration_since(self.start));
        if self.tty {
            eprint!("\r\x1b[2K{line}");
        } else {
            eprintln!("{line}");
        }
    }

    /// Ends the progress line.
    pub fn end(&self) {
        if self.tty && self.last.is_some() {
            eprintln!();
        }
    }

    /// Time since the start.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Whether this process runs as root.
#[must_use]
pub fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

/// `path`'s text, `None` when unreadable.
#[must_use]
pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests;
