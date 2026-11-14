//! The move helper's file protocol (SPEC-470, version 1).
//!
//! The server, running as the unprivileged service user, writes one [`MoveRequest`] to
//! `<data>/migrate/request.json` (mode 0600, written atomically). A root-owned systemd path unit
//! starts `yulius migrate-helper`, which checks every field again and runs `yulius instance
//! migrate` (SPEC-460): its plan, the move, or its abort. The helper reports in
//! [`MoveHelperStatus`] at `<state>/helper.json`; the command keeps its own checklist in
//! `<state>/status.json`. A finished move leaves the checklist on the new host as
//! [`MoveChecklist`] in `<state>/arrived.json`. No file holds a secret value: a request has no
//! field for one.

use serde::{Deserialize, Serialize};

use crate::update::HostLine;

/// Protocol version.
pub const VERSION: u32 = 1;
/// Largest request file.
pub const MAX_REQUEST_BYTES: u64 = 16 * 1024;
/// Largest helper status file.
pub const MAX_HELPER_STATUS_BYTES: u64 = 256 * 1024;
/// Largest checklist: the command's status, and the one a move leaves on the new host.
pub const MAX_CHECKLIST_BYTES: u64 = 1024 * 1024;
/// A request older than this is stale and refused (a reboot, or a copy to another host, must not
/// replay it).
pub const REQUEST_TTL_MS: i64 = 15 * 60 * 1000;
/// A request may be dated at most this far ahead of the helper's clock.
pub const REQUEST_AHEAD_MS: i64 = 60_000;
/// The helper rewrites its status at least this often while the command runs.
pub const HEARTBEAT_MS: i64 = 5_000;
/// The request's directory in the data directory.
pub const REQUEST_DIR: &str = "migrate";
/// The request's file in [`REQUEST_DIR`].
pub const REQUEST_FILE: &str = "request.json";
/// The helper's status in its state directory.
pub const HELPER_STATUS_FILE: &str = "helper.json";
/// The command's status (its phase and checklist) in the state directory.
pub const STATUS_FILE: &str = "status.json";
/// The finished checklist a move leaves in the state directory of the new host, for its
/// server: root's file in root's directory, so nothing is written where the service user could
/// redirect it.
pub const ARRIVED_FILE: &str = "arrived.json";
/// Longest target, and longest public URL.
pub const MAX_TARGET_BYTES: usize = 255;
/// Longest name of a data disk.
pub const MAX_DISK_BYTES: usize = 128;
/// Data disks at most: one, or two for a mirror.
pub const MAX_DATA_DISKS: usize = 2;

/// What the helper is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveAction {
    /// Show the plan: nothing is changed but the flow's own files, nothing freezes.
    Plan,
    /// Move, or continue a move.
    Move,
    /// Go back to this host.
    Abort,
}

impl MoveAction {
    /// Stable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Move => "move",
            Self::Abort => "abort",
        }
    }
}

/// `<data>/migrate/request.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    /// [`VERSION`].
    pub version: u32,
    /// Request id (8–64 of `[A-Za-z0-9-]`); the status echoes it, and an id is acted on once.
    pub id: String,
    /// Plan, move or abort.
    pub action: MoveAction,
    /// The new host as ssh takes it (`root@new-host`); absent for an abort.
    #[serde(default)]
    pub target: Option<String>,
    /// Its SSH port; absent for an abort.
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// The new public URL (`https://host`), when the person names one.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Blank disks of the new host for the data directory: none, one, or two for a mirror.
    #[serde(default)]
    pub data_disks: Vec<String>,
    /// When it was written (ms since the epoch).
    pub requested_at: i64,
}

impl MoveRequest {
    /// Its values.
    #[must_use]
    pub fn values(&self) -> Values {
        Values {
            target: self.target.clone(),
            ssh_port: self.ssh_port,
            public_url: self.public_url.clone(),
            data_disks: self.data_disks.clone(),
        }
    }
}

/// What a request names beside its action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Values {
    /// The new host.
    #[serde(default)]
    pub target: Option<String>,
    /// Its SSH port.
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// The new public URL.
    #[serde(default)]
    pub public_url: Option<String>,
    /// The data disks.
    #[serde(default)]
    pub data_disks: Vec<String>,
}

/// Where a helper run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveHelperState {
    /// The command runs.
    Running,
    /// The command ended; see the exit code.
    Ended,
    /// The request was refused; nothing ran.
    Refused,
}

/// `<state>/helper.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveHelperStatus {
    /// [`VERSION`].
    pub version: u32,
    /// The request it reports on.
    pub id: String,
    /// Its action.
    pub action: MoveAction,
    /// Its values.
    #[serde(flatten)]
    pub values: Values,
    /// Running, ended or refused.
    pub state: MoveHelperState,
    /// The command's exit code once it ended: 0 nothing failed, 1 a step failed, 2 refused
    /// before anything changed; `None` when a signal ended it.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Why the request was refused, or the command's last words when it ended before its
    /// checklist (no secret values).
    #[serde(default)]
    pub error: Option<String>,
    /// When the helper took the request (ms since the epoch).
    pub started_at: i64,
    /// Last write (ms since the epoch): a heartbeat while the command runs.
    pub updated_at: i64,
    /// When the command ended or the request was refused.
    #[serde(default)]
    pub ended_at: Option<i64>,
}

/// `<state>/arrived.json` on the new host: the checklist of the move that brought the instance
/// there (SPEC-470 MVB-011).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveChecklist {
    /// [`VERSION`].
    pub version: u32,
    /// The move's id.
    pub id: String,
    /// The new host, as the old one named it.
    pub target: String,
    /// The old public URL.
    #[serde(default)]
    pub from: Option<String>,
    /// The new one.
    #[serde(default)]
    pub to: Option<String>,
    /// The finished checklist.
    #[serde(default)]
    pub lines: Vec<HostLine>,
    /// When the move finished (seconds since the epoch).
    pub finished_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// SPEC-470 MVB-001: the request has exactly these fields, so nothing else (a secret, an
    /// option of the command) can ride in it.
    #[test]
    fn a_request_has_no_other_field() -> TestResult {
        let text = r#"{"version":1,"id":"0123abcd-plan","action":"plan","target":"root@new.example",
            "ssh_port":2222,"public_url":"https://new.example","data_disks":["/dev/vdc"],
            "requested_at":5}"#;
        let request: MoveRequest = serde_json::from_str(text)?;
        assert_eq!(request.action, MoveAction::Plan);
        assert_eq!(request.ssh_port, Some(2222));
        assert_eq!(request.data_disks, ["/dev/vdc"]);
        let again: MoveRequest = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
        assert_eq!(again, request);
        // An abort names nothing.
        let abort: MoveRequest = serde_json::from_str(
            r#"{"version":1,"id":"0123abcd-abort","action":"abort","requested_at":5}"#,
        )?;
        assert_eq!(abort.values(), Values::default());
        for extra in [r#""yes":true"#, r#""token":"x""#, r#""state_dir":"/tmp""#] {
            let with = text.replacen('{', &format!("{{{extra},"), 1);
            assert!(
                serde_json::from_str::<MoveRequest>(&with).is_err(),
                "{extra}"
            );
        }
        assert!(serde_json::from_str::<MoveRequest>(&text.replace("plan", "install")).is_err());
        assert!(serde_json::from_str::<MoveRequest>(&text.replace("2222", "70000")).is_err());
        Ok(())
    }

    /// The helper's status carries the request's values beside its state, in one object.
    #[test]
    fn a_helper_status_holds_the_requests_values() -> TestResult {
        let request = MoveRequest {
            version: VERSION,
            id: "0123abcd-move".into(),
            action: MoveAction::Move,
            target: Some("root@new.example".into()),
            ssh_port: Some(22),
            public_url: None,
            data_disks: vec!["/dev/vdc".into(), "/dev/vdd".into()],
            requested_at: 5,
        };
        let status = MoveHelperStatus {
            version: VERSION,
            id: request.id.clone(),
            action: request.action,
            values: request.values(),
            state: MoveHelperState::Ended,
            exit_code: Some(1),
            error: None,
            started_at: 6,
            updated_at: 9,
            ended_at: Some(9),
        };
        let json = serde_json::to_value(&status)?;
        assert_eq!(json["target"], "root@new.example");
        assert_eq!(json["state"], "ended");
        assert_eq!(json["action"], "move");
        let back: MoveHelperStatus = serde_json::from_value(json)?;
        assert_eq!(back, status);
        assert_eq!(back.values, request.values());
        Ok(())
    }
}
