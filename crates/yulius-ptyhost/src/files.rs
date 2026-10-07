//! Run-directory file primitives: crash-safe replacement and bounded record reads (DATA-001).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nix::fcntl::OFlag;
use serde::de::DeserializeOwned;
use yulius_contracts::ptyhost::{EXIT_FILE, ExitFile, ExitRecord, RECORD_VERSION, StatusRecord};

/// Largest record file the host or the server reads.
pub(crate) const MAX_RECORD_BYTES: u64 = 1 << 20;

/// Replaces `dir/name` atomically: write `dir/.name.tmp` (mode `mode`, no symlink following),
/// fsync it, rename over the target and fsync the directory.
pub(crate) fn atomic_write(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp"));
    let target = dir.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(&tmp)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    fs::rename(&tmp, &target)?;
    File::open(dir)?.sync_all()
}

/// Errors reading a JSON record.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RecordError {
    #[error("record unreadable: {0}")]
    Io(#[from] io::Error),
    #[error("record exceeds {MAX_RECORD_BYTES} bytes")]
    TooLarge,
    #[error("record is not valid JSON for its schema: {0}")]
    Json(#[from] serde_json::Error),
    #[error("record version {0} is newer than supported version {RECORD_VERSION}")]
    Version(u32),
}

/// Reads and parses a JSON record; `Ok(None)` when the file does not exist.
pub(crate) fn read_record<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, RecordError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(RecordError::TooLarge);
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Reads `status.json`, rejecting newer record versions.
pub(crate) fn read_status(path: &Path) -> Result<Option<StatusRecord>, RecordError> {
    match read_record::<StatusRecord>(path)? {
        Some(status) if status.version > RECORD_VERSION => Err(RecordError::Version(status.version)),
        other => Ok(other),
    }
}

/// Reads the [`ExitRecord`] from `exit.json`, rejecting newer record versions.
pub(crate) fn read_exit(run_dir: &Path) -> Result<Option<ExitRecord>, RecordError> {
    match read_record::<ExitFile>(&run_dir.join(EXIT_FILE))? {
        Some(file) if file.version > RECORD_VERSION => Err(RecordError::Version(file.version)),
        other => Ok(other.map(|file| file.exit)),
    }
}

/// Wall-clock Unix milliseconds.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Truncates `text` to at most `max` bytes on a character boundary.
pub(crate) fn truncate_utf8(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn atomic_write_replaces_content_with_mode_and_leaves_no_temporary() -> TestResult {
        let dir = tempfile::tempdir()?;
        atomic_write(dir.path(), "r.json", b"one", 0o600)?;
        atomic_write(dir.path(), "r.json", b"two", 0o600)?;
        assert_eq!(fs::read(dir.path().join("r.json"))?, b"two");
        let mode = fs::metadata(dir.path().join("r.json"))?.permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!dir.path().join(".r.json.tmp").exists());
        Ok(())
    }

    #[test]
    fn atomic_write_refuses_a_symlinked_temporary() -> TestResult {
        let dir = tempfile::tempdir()?;
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, b"keep")?;
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(".r.json.tmp"))?;
        assert!(atomic_write(dir.path(), "r.json", b"x", 0o600).is_err());
        assert_eq!(fs::read(&elsewhere)?, b"keep");
        Ok(())
    }

    #[test]
    fn records_are_bounded_versioned_and_missing_is_none() -> TestResult {
        let dir = tempfile::tempdir()?;
        assert!(read_exit(dir.path())?.is_none());
        fs::write(
            dir.path().join(EXIT_FILE),
            r#"{"version":2,"attempt_id":"a","exit":{"code":0,"signal":null,"ended_at_ms":1,"reason":"exited"}}"#,
        )?;
        assert!(matches!(read_exit(dir.path()), Err(RecordError::Version(2))));
        fs::write(dir.path().join(EXIT_FILE), vec![b' '; 2 << 20])?;
        assert!(matches!(read_exit(dir.path()), Err(RecordError::TooLarge)));
        fs::write(dir.path().join(EXIT_FILE), b"{not json")?;
        assert!(matches!(read_exit(dir.path()), Err(RecordError::Json(_))));
        Ok(())
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_utf8("aé".into(), 2), "a");
        assert_eq!(truncate_utf8("abc".into(), 5), "abc");
    }
}
