//! The server's side of the update helper's file protocol (SPEC-002 ADR-18,
//! `yulius_contracts::update`): it writes `<data>/update/request.json` (owner-only, atomically)
//! and reads `<helper state>/status.json` (bounded, well-formed or ignored).

use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use yulius_app::ports::updates::UpdateHelper;
use yulius_contracts::update::{HelperRequest, HelperStatus, MAX_STATUS_BYTES, VERSION};

/// Name of the request file in `<data>/update/`.
pub const REQUEST_FILE: &str = "request.json";
/// Name of the status file in the helper's state directory.
pub const STATUS_FILE: &str = "status.json";

/// The request file of a data directory.
#[must_use]
pub fn request_path(data_dir: &Path) -> PathBuf {
    data_dir.join("update").join(REQUEST_FILE)
}

/// Files shared with the root helper.
#[derive(Debug, Clone)]
pub struct FileUpdateHelper {
    request: PathBuf,
    state_dir: PathBuf,
}

impl FileUpdateHelper {
    /// The helper of `data_dir` whose state lives in `state_dir` (`/var/lib/yulius-update`).
    #[must_use]
    pub fn new(data_dir: &Path, state_dir: PathBuf) -> Self {
        Self {
            request: request_path(data_dir),
            state_dir,
        }
    }
}

/// Reads at most `limit` bytes of a regular file.
pub fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(std::io::Error::other("too large"));
    }
    Ok(bytes)
}

/// Writes `bytes` to `path` with `mode` through a temporary file in the same directory.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, path)
}

impl UpdateHelper for FileUpdateHelper {
    fn installed(&self) -> bool {
        self.state_dir.is_dir()
    }

    fn request(&self, request: &HelperRequest) -> Result<(), String> {
        let dir = self
            .request
            .parent()
            .ok_or_else(|| "no request directory".to_owned())?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot restrict {}: {e}", dir.display()))?;
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        write_atomic(&self.request, &bytes, 0o600)
            .map_err(|e| format!("cannot write {}: {e}", self.request.display()))
    }

    fn status(&self) -> Option<HelperStatus> {
        let bytes = read_bounded(&self.state_dir.join(STATUS_FILE), MAX_STATUS_BYTES).ok()?;
        serde_json::from_slice::<HelperStatus>(&bytes)
            .ok()
            .filter(|s| s.version == VERSION)
    }

    fn clear_request(&self) {
        let _ = std::fs::remove_file(&self.request);
    }
}

#[cfg(test)]
mod tests {
    use yulius_contracts::update::{HelperAction, HelperState};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn requests_are_owner_only_and_statuses_are_checked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let data = dir.path().join("data");
        let state = dir.path().join("helper");
        std::fs::create_dir_all(&data)?;
        let helper = FileUpdateHelper::new(&data, state.clone());
        assert!(!helper.installed());
        std::fs::create_dir_all(&state)?;
        assert!(helper.installed());
        let request = HelperRequest {
            version: VERSION,
            id: "run-1-build".into(),
            action: HelperAction::Build,
            commit: "a".repeat(40),
            token: Some("ghs_synthetic".into()),
            requested_at: 1,
        };
        helper.request(&request)?;
        let path = request_path(&data);
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().ok_or("parent")?)?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let back: HelperRequest = serde_json::from_slice(&std::fs::read(&path)?)?;
        assert_eq!(back, request);
        assert!(!format!("{back:?}").contains("ghs_synthetic"));
        helper.clear_request();
        assert!(!path.exists());

        assert!(helper.status().is_none());
        let status = HelperStatus {
            version: VERSION,
            id: "run-1-build".into(),
            action: HelperAction::Build,
            commit: "a".repeat(40),
            state: HelperState::Running,
            step: "compiling".into(),
            progress: None,
            error: None,
            previous: None,
            log_tail: Vec::new(),
            host_check: None,
            updated_at: 5,
        };
        std::fs::write(state.join(STATUS_FILE), serde_json::to_vec(&status)?)?;
        assert_eq!(helper.status(), Some(status.clone()));
        std::fs::write(
            state.join(STATUS_FILE),
            serde_json::to_vec(&HelperStatus {
                version: 2,
                ..status
            })?,
        )?;
        assert!(
            helper.status().is_none(),
            "another protocol version is ignored"
        );
        std::fs::write(state.join(STATUS_FILE), vec![b' '; 300 * 1024])?;
        assert!(helper.status().is_none(), "an oversized status is ignored");
        Ok(())
    }
}
