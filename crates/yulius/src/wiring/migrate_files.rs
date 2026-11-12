//! The server's side of the move helper's file protocol (SPEC-470,
//! `yulius_contracts::migrate`): it writes `<data>/migrate/request.json` (owner-only,
//! atomically) and reads, bounded and well-formed or ignored, the helper's report, the command's
//! checklist and the checklist a move left on this host, all in the helper's state directory.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use yulius_app::ports::migration::{MoveHelper, MoveStatus};
use yulius_contracts::migrate::{
    ARRIVED_FILE, HELPER_STATUS_FILE, MAX_CHECKLIST_BYTES, MAX_HELPER_STATUS_BYTES,
    MAX_REQUEST_BYTES, MoveChecklist, MoveHelperStatus, MoveRequest, REQUEST_DIR, REQUEST_FILE,
    STATUS_FILE, VERSION,
};

use super::update_files::{read_bounded, write_atomic};

/// How long systemd's answer about the path unit is kept: the page reads every few seconds.
const INSTALLED_FOR: Duration = Duration::from_secs(10);

/// The request file of a data directory.
#[must_use]
pub fn request_path(data_dir: &Path) -> PathBuf {
    data_dir.join(REQUEST_DIR).join(REQUEST_FILE)
}

/// Files shared with the root move helper.
#[derive(Debug)]
pub struct FileMoveHelper {
    request: PathBuf,
    state_dir: PathBuf,
    /// The helper's path unit, named after the server's own unit; `None` outside a unit.
    path_unit: Option<String>,
    systemctl: PathBuf,
    installed: Mutex<Option<(Instant, bool)>>,
    /// Held while the request file is written or removed: a removal is of the request it
    /// looked at, never of one written in between.
    writes: Mutex<()>,
}

impl FileMoveHelper {
    /// The helper of `data_dir` whose state lives in `state_dir` (`/var/lib/yulius-migrate`)
    /// and whose requests `path_unit` waits for.
    #[must_use]
    pub fn new(data_dir: &Path, state_dir: PathBuf, path_unit: Option<String>) -> Self {
        Self {
            request: request_path(data_dir),
            state_dir,
            path_unit,
            systemctl: PathBuf::from("systemctl"),
            installed: Mutex::new(None),
            writes: Mutex::new(()),
        }
    }

    /// Asks `systemctl` instead of the one on `PATH`.
    #[must_use]
    pub fn with_systemctl(mut self, systemctl: PathBuf) -> Self {
        self.systemctl = systemctl;
        self
    }

    fn read<T: serde::de::DeserializeOwned>(&self, name: &str, limit: u64) -> Option<T> {
        let bytes = read_bounded(&self.state_dir.join(name), limit).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

impl MoveHelper for FileMoveHelper {
    fn installed(&self) -> bool {
        let Some(unit) = &self.path_unit else {
            return false;
        };
        if let Ok(kept) = self.installed.lock()
            && let Some((at, installed)) = *kept
            && at.elapsed() < INSTALLED_FOR
        {
            return installed;
        }
        // "active (waiting)": enabled or started by hand, it takes requests.
        let installed = crate::doctor::output_of(
            &self.systemctl,
            &["show", "--property=ActiveState", "--", unit],
        )
        .is_some_and(|show| {
            crate::restart_safety::property(&show, "ActiveState") == Some("active")
        });
        if let Ok(mut kept) = self.installed.lock() {
            *kept = Some((Instant::now(), installed));
        }
        installed
    }

    fn request(&self, request: &MoveRequest) -> Result<(), String> {
        let dir = self
            .request
            .parent()
            .ok_or_else(|| "no request directory".to_owned())?;
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot restrict {}: {e}", dir.display()))?;
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let _held = self.writes.lock();
        write_atomic(&self.request, &bytes, 0o600)
            .map_err(|e| format!("cannot write {}: {e}", self.request.display()))
    }

    fn pending(&self) -> Option<MoveRequest> {
        let bytes = read_bounded(&self.request, MAX_REQUEST_BYTES).ok()?;
        serde_json::from_slice::<MoveRequest>(&bytes)
            .ok()
            .filter(|request| request.version == VERSION)
    }

    fn clear_request(&self, id: &str) {
        let _held = self.writes.lock();
        if self.pending().is_some_and(|request| request.id == id) {
            let _ = std::fs::remove_file(&self.request);
        }
    }

    fn helper_status(&self) -> Option<MoveHelperStatus> {
        self.read::<MoveHelperStatus>(HELPER_STATUS_FILE, MAX_HELPER_STATUS_BYTES)
            .filter(|status| status.version == VERSION)
    }

    fn status(&self) -> Option<MoveStatus> {
        let status: crate::instance::migrate::Status =
            self.read(STATUS_FILE, MAX_CHECKLIST_BYTES)?;
        (status.version == 1).then_some(MoveStatus {
            id: status.id,
            target: status.target,
            phase: status.phase,
            lines: status.lines,
            updated_at: status.updated_at,
            public_key: status.public_key,
            new_url: status.new_url,
        })
    }

    fn arrived(&self) -> Option<MoveChecklist> {
        self.read::<MoveChecklist>(ARRIVED_FILE, MAX_CHECKLIST_BYTES)
            .filter(|arrived| arrived.version == VERSION)
    }
}

#[cfg(test)]
mod tests {
    use yulius_contracts::migrate::{MoveAction, MoveHelperState, Values};
    use yulius_contracts::update::{HostLine, HostOutcome};

    use super::*;
    use crate::instance::migrate::Status;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn request() -> MoveRequest {
        MoveRequest {
            version: VERSION,
            id: "0123abcd-plan".into(),
            action: MoveAction::Plan,
            target: Some("root@new.example".into()),
            ssh_port: Some(22),
            public_url: None,
            data_disks: vec!["/dev/vdc".into()],
            requested_at: 5,
        }
    }

    /// SPEC-470 MVB-001: the request is the service user's alone, written whole, and read back
    /// by the server that wrote it.
    #[test]
    fn a_request_is_owner_only_and_read_back() -> TestResult {
        let dir = tempfile::tempdir()?;
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data)?;
        let helper = FileMoveHelper::new(&data, dir.path().join("state"), None);
        assert_eq!(helper.pending(), None);
        helper.request(&request())?;
        let path = request_path(&data);
        assert_eq!(path, data.join("migrate/request.json"));
        let mode = |path: &Path| std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode(&path)?, 0o600);
        assert_eq!(mode(path.parent().ok_or("parent")?)?, 0o700);
        // Exactly what the root helper parses.
        let parsed = crate::migrate_helper::parse_request(&std::fs::read(&path)?, 5)?;
        assert_eq!(parsed, request());
        assert_eq!(helper.pending(), Some(request()));
        // Nothing half written is left beside it.
        let names: Vec<String> = std::fs::read_dir(path.parent().ok_or("parent")?)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["request.json"]);

        // A file that is not a request of this protocol is no request.
        std::fs::write(&path, "{")?;
        assert_eq!(helper.pending(), None);
        let mut other = serde_json::to_value(request())?;
        other["version"] = 2.into();
        std::fs::write(&path, serde_json::to_vec(&other)?)?;
        assert_eq!(helper.pending(), None);
        std::fs::write(&path, vec![b' '; 17 * 1024])?;
        assert_eq!(helper.pending(), None);

        // Removed by its id: a newer request that took its place meanwhile stays.
        helper.request(&request())?;
        helper.clear_request("0123abcd-older");
        assert_eq!(helper.pending(), Some(request()));
        helper.clear_request("0123abcd-plan");
        assert!(!path.exists());
        helper.clear_request("0123abcd-plan");
        Ok(())
    }

    /// SPEC-470 MVB-020: what root's helper and command wrote is read bounded, and ignored
    /// when it is not what this protocol writes.
    #[test]
    fn reports_are_read_bounded_and_checked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let state = dir.path().join("state");
        let helper = FileMoveHelper::new(dir.path(), state.clone(), None);
        assert!(helper.helper_status().is_none() && helper.status().is_none());
        assert!(helper.arrived().is_none());
        std::fs::create_dir_all(&state)?;

        let report = MoveHelperStatus {
            version: VERSION,
            id: "0123abcd-plan".into(),
            action: MoveAction::Plan,
            values: Values {
                target: Some("root@new.example".into()),
                ssh_port: Some(22),
                ..Values::default()
            },
            state: MoveHelperState::Running,
            exit_code: None,
            error: None,
            started_at: 5,
            updated_at: 9,
            ended_at: None,
        };
        let path = state.join(HELPER_STATUS_FILE);
        std::fs::write(&path, serde_json::to_vec(&report)?)?;
        assert_eq!(helper.helper_status(), Some(report.clone()));
        std::fs::write(
            &path,
            serde_json::to_vec(&MoveHelperStatus {
                version: 2,
                ..report
            })?,
        )?;
        assert!(helper.helper_status().is_none(), "another protocol version");
        std::fs::write(&path, vec![b' '; 300 * 1024])?;
        assert!(helper.helper_status().is_none(), "oversized");

        // The command's own status file, as the command writes it.
        let lines = vec![HostLine {
            name: "one-time key".into(),
            outcome: HostOutcome::Manual,
            detail: "add this line".into(),
        }];
        let status = Status {
            version: 1,
            id: "ab12cd34".into(),
            target: "root@new.example".into(),
            phase: "Plan".into(),
            lines: lines.clone(),
            updated_at: 1_794_000_000,
            public_key: Some("ssh-ed25519 AAAA yulius-migrate-ab12cd34".into()),
            new_url: Some("https://new.example".into()),
        };
        let path = state.join(STATUS_FILE);
        std::fs::write(&path, serde_json::to_vec(&status)?)?;
        assert_eq!(
            helper.status(),
            Some(MoveStatus {
                id: "ab12cd34".into(),
                target: "root@new.example".into(),
                phase: "Plan".into(),
                lines: lines.clone(),
                updated_at: 1_794_000_000,
                public_key: Some("ssh-ed25519 AAAA yulius-migrate-ab12cd34".into()),
                new_url: Some("https://new.example".into()),
            })
        );
        // A status of the command before these fields: read, with nothing to add.
        std::fs::write(
            &path,
            r#"{"version":1,"id":"ab12cd34","target":"t","phase":"done","lines":[],"updated_at":7}"#,
        )?;
        let old = helper.status().ok_or("an older status was not read")?;
        assert_eq!(
            (old.public_key, old.new_url, old.updated_at),
            (None, None, 7)
        );
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1])?;
        assert!(helper.status().is_none(), "oversized");
        std::fs::write(
            &path,
            r#"{"version":3,"id":"x","target":"t","phase":"Plan","lines":[],"updated_at":7}"#,
        )?;
        assert!(helper.status().is_none(), "another version");

        let arrived = MoveChecklist {
            version: VERSION,
            id: "ab12cd34".into(),
            target: "root@new.example".into(),
            from: Some("https://old.example".into()),
            to: Some("https://new.example".into()),
            lines,
            finished_at: 1_794_000_000,
        };
        let path = state.join(ARRIVED_FILE);
        std::fs::write(&path, serde_json::to_vec(&arrived)?)?;
        assert_eq!(helper.arrived(), Some(arrived));
        std::fs::write(&path, "[]")?;
        assert!(helper.arrived().is_none());
        Ok(())
    }

    /// SPEC-470 MVB-020: the helper is installed when systemd says its path unit waits; the
    /// answer is kept for a while, since the page asks every few seconds.
    #[test]
    fn installed_is_what_systemd_says_of_the_path_unit() -> TestResult {
        let dir = tempfile::tempdir()?;
        let systemctl = dir.path().join("systemctl");
        std::fs::write(
            &systemctl,
            "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/asked\"\n\
             case \"$*\" in *yulius-migrate.path) echo ActiveState=active ;; \
             *) echo ActiveState=inactive ;; esac\n",
        )?;
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755))?;
        let helper = |unit: Option<&str>| {
            FileMoveHelper::new(
                dir.path(),
                dir.path().join("state"),
                unit.map(str::to_owned),
            )
            .with_systemctl(systemctl.clone())
        };
        let asked = || {
            std::fs::read_to_string(dir.path().join("asked"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        // Outside a unit there is no helper to ask about.
        assert!(!helper(None).installed());
        assert!(asked().is_empty());

        let waiting = helper(Some("yulius-migrate.path"));
        assert!(waiting.installed());
        assert!(waiting.installed());
        assert_eq!(
            asked(),
            ["show --property=ActiveState -- yulius-migrate.path"],
            "asked once, then kept"
        );
        assert!(!helper(Some("other-migrate.path")).installed());
        // A systemctl that is not there says nothing: not installed.
        let none = FileMoveHelper::new(dir.path(), dir.path().join("state"), Some("x.path".into()))
            .with_systemctl(dir.path().join("missing"));
        assert!(!none.installed());
        Ok(())
    }
}
