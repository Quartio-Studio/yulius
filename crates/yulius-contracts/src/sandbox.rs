//! Records and relay frames between the server and a sandbox's namespace holder (SPEC-002
//! ADR-22, §2.8).
//!
//! A holder lives in `runtime/sandboxes/<sandbox>/` (mode 0700, never bound into runs):
//!
//! | File | Writer | Content |
//! | --- | --- | --- |
//! | `holder.json` | server | [`HolderSpec`] |
//! | `host.json` | `yulius sandbox-host start` | [`HolderRecord`] once the holder runs |
//! | `holder.log` | the holder | diagnostics (no secrets) |
//! | `in.sock` | the holder (inside the sandbox's network) | relay into the sandbox ([`INBOUND_CONNECT`], [`INBOUND_PORTS`]) |
//! | `out.sock` | the server | relay out of the sandbox (a [`u16`] port, then bytes) |
//! | `root/` | the holder's mount namespace | the merged system layer (empty on the host) |
//!
//! Relay frames: a client of `in.sock` sends [`INBOUND_CONNECT`] and the port (big-endian `u16`);
//! the holder answers one status byte ([`RELAY_OK`], [`RELAY_REFUSED`], [`RELAY_FORBIDDEN`]) and
//! then copies bytes both ways. [`INBOUND_PORTS`] is answered with one JSON line, a
//! [`ListeningPorts`], and the connection ends. A connection the holder accepts on a forwarded
//! port inside the sandbox goes to `out.sock` as the port (big-endian `u16`) followed by the
//! bytes; the server answers one status byte as above.

use serde::{Deserialize, Serialize};

use crate::ptyhost::LaunchSpecError;

/// Version of the holder records.
pub const HOLDER_RECORD_VERSION: u32 = 1;
/// The server's holder spec in the state directory.
pub const HOLDER_SPEC_FILE: &str = "holder.json";
/// The running holder's record in the state directory.
pub const HOLDER_RECORD_FILE: &str = "host.json";
/// The holder's diagnostics.
pub const HOLDER_LOG_FILE: &str = "holder.log";
/// The relay into the sandbox, served by the holder.
pub const INBOUND_SOCKET: &str = "in.sock";
/// The relay out of the sandbox, served by the server.
pub const OUTBOUND_SOCKET: &str = "out.sock";
/// Mount point of the merged system layer inside the holder's mount namespace.
pub const SYSTEM_ROOT_DIR: &str = "root";

/// `in.sock` request: connect to a port inside the sandbox.
pub const INBOUND_CONNECT: u8 = b'C';
/// `in.sock` request: the ports listening inside the sandbox within its range.
pub const INBOUND_PORTS: u8 = b'P';
/// Relay status: connected; bytes follow.
pub const RELAY_OK: u8 = 0;
/// Relay status: nothing listens there.
pub const RELAY_REFUSED: u8 = 1;
/// Relay status: the port is not relayed.
pub const RELAY_FORBIDDEN: u8 = 2;

/// Host ids mapped into a sandbox's user namespace: root inside is `host`, ids `1..=count`
/// inside are `sub_start..sub_start + count` on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdMap {
    /// The service user's (or group's) id, which root inside maps to.
    pub host: u32,
    /// First id of the subordinate range (`/etc/subuid` or `/etc/subgid`).
    pub sub_start: u32,
    /// Number of subordinate ids mapped (inside ids `1..=count`).
    pub count: u32,
}

/// What the server asks of a holder (`holder.json`). Paths are absolute host paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderSpec {
    /// Record version ([`HOLDER_RECORD_VERSION`]).
    pub version: u32,
    /// Sandbox identifier.
    pub sandbox: String,
    /// Host name inside the sandbox (its slug).
    pub hostname: String,
    /// The state directory (`runtime/sandboxes/<sandbox>`).
    pub state_dir: String,
    /// Lower layer: the base copy of the host system.
    pub base: String,
    /// Identifier of the base (`base-<n>`).
    pub base_id: String,
    /// The sandbox's upper layer.
    pub upper: String,
    /// The overlay's work directory (same filesystem as `upper`).
    pub work: String,
    /// The `yulius` executable the holder runs inside.
    pub exe: String,
    /// `bwrap`.
    pub bwrap: String,
    /// `newuidmap`.
    pub newuidmap: String,
    /// `newgidmap`.
    pub newgidmap: String,
    /// `slirp4netns` for egress; `None`: no egress besides the relays.
    #[serde(default)]
    pub slirp4netns: Option<String>,
    /// User ids mapped into the namespace.
    pub uid: IdMap,
    /// Group ids mapped into the namespace.
    pub gid: IdMap,
    /// The sandbox's cgroup directory, joined before anything starts.
    #[serde(default)]
    pub cgroup: Option<String>,
    /// First port of the sandbox's dev-server range (relayed out to the host's loopback).
    pub port_first: u16,
    /// Last port of that range (inclusive).
    pub port_last: u16,
    /// Ports inside the sandbox relayed to the server's `out.sock` (the internal listener, the
    /// operator's extra forwards).
    #[serde(default)]
    pub forwards: Vec<u16>,
}

fn spec_err(field: &'static str, reason: &'static str) -> LaunchSpecError {
    LaunchSpecError { field, reason }
}

fn abs(path: &str) -> bool {
    path.starts_with('/') && path.len() > 1 && !path.contains('\0') && path.len() <= 4096
}

/// Whether `text` is a sandbox identifier usable in paths (1–64 of `[A-Za-z0-9_-]`).
#[must_use]
pub fn valid_sandbox_id(text: &str) -> bool {
    crate::ptyhost::valid_identifier(text)
}

/// Whether `text` is a host name (1–63 letters, digits or dashes).
#[must_use]
pub fn valid_hostname(text: &str) -> bool {
    (1..=63).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl HolderSpec {
    /// Validates every field.
    ///
    /// # Errors
    /// The first offending field.
    pub fn validate(&self) -> Result<(), LaunchSpecError> {
        if self.version != HOLDER_RECORD_VERSION {
            return Err(spec_err("version", "unsupported record version"));
        }
        if !valid_sandbox_id(&self.sandbox) {
            return Err(spec_err("sandbox", "must be 1-64 of [A-Za-z0-9_-]"));
        }
        if !valid_hostname(&self.hostname) {
            return Err(spec_err(
                "hostname",
                "must be 1..=63 letters, digits or dashes",
            ));
        }
        if !valid_sandbox_id(&self.base_id) {
            return Err(spec_err("base_id", "must be 1-64 of [A-Za-z0-9_-]"));
        }
        let paths = [
            &self.state_dir,
            &self.base,
            &self.upper,
            &self.work,
            &self.exe,
            &self.bwrap,
            &self.newuidmap,
            &self.newgidmap,
        ];
        if !paths.iter().all(|p| abs(p)) {
            return Err(spec_err("paths", "must be absolute and not /"));
        }
        if self.slirp4netns.as_deref().is_some_and(|p| !abs(p))
            || self.cgroup.as_deref().is_some_and(|p| !abs(p))
        {
            return Err(spec_err("paths", "must be absolute and not /"));
        }
        for map in [self.uid, self.gid] {
            if map.count == 0 || map.count > 65_535 || map.sub_start == 0 {
                return Err(spec_err("uid/gid", "need 1..=65535 subordinate ids"));
            }
            if map.sub_start.checked_add(map.count).is_none() {
                return Err(spec_err("uid/gid", "subordinate range overflows"));
            }
        }
        if self.port_first > self.port_last {
            return Err(spec_err("ports", "first port after last port"));
        }
        if self.forwards.len() > 16 || self.forwards.contains(&0) {
            return Err(spec_err("forwards", "at most 16 ports, none 0"));
        }
        Ok(())
    }
}

/// A running holder (`host.json`), written once it is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderRecord {
    /// Record version ([`HOLDER_RECORD_VERSION`]).
    pub version: u32,
    /// Sandbox identifier.
    pub sandbox: String,
    /// The base the system layer is mounted on.
    pub base_id: String,
    /// The holder process inside the namespaces (the process runs join).
    pub inner_pid: u32,
    /// The `bwrap` process that created the namespaces (the holder's parent).
    pub bwrap_pid: u32,
    /// `slirp4netns`, when the sandbox has egress.
    #[serde(default)]
    pub slirp_pid: Option<u32>,
    /// Inode of the holder's user namespace.
    pub user_ns: u64,
    /// Inode of the holder's network namespace.
    pub net_ns: u64,
    /// Inode of the holder's mount namespace.
    pub mnt_ns: u64,
    /// Boot the holder belongs to.
    pub boot_id: String,
    /// When the holder became ready (ms since the epoch).
    pub started_at_ms: i64,
    /// The merged system layer inside the holder's mount namespace.
    pub system_root: String,
}

/// Answer to [`INBOUND_PORTS`]: ports of the sandbox's range something listens on inside.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningPorts {
    /// Sorted, unique.
    pub ports: Vec<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HolderSpec {
        HolderSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: "s1".into(),
            hostname: "fixture".into(),
            state_dir: "/data/runtime/sandboxes/s1".into(),
            base: "/data/system/base-1".into(),
            base_id: "base-1".into(),
            upper: "/data/sysroots/s1/upper".into(),
            work: "/data/sysroots/s1/work".into(),
            exe: "/usr/local/bin/yulius".into(),
            bwrap: "/usr/bin/bwrap".into(),
            newuidmap: "/usr/bin/newuidmap".into(),
            newgidmap: "/usr/bin/newgidmap".into(),
            slirp4netns: Some("/usr/bin/slirp4netns".into()),
            uid: IdMap {
                host: 999,
                sub_start: 231_072,
                count: 65_535,
            },
            gid: IdMap {
                host: 988,
                sub_start: 231_072,
                count: 65_535,
            },
            cgroup: None,
            port_first: 20_000,
            port_last: 20_019,
            forwards: vec![8081],
        }
    }

    #[test]
    fn a_complete_spec_validates_and_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let spec = spec();
        spec.validate()?;
        let json = serde_json::to_string(&spec)?;
        assert_eq!(serde_json::from_str::<HolderSpec>(&json)?, spec);
        Ok(())
    }

    #[test]
    fn bad_fields_are_refused() {
        let mut s = spec();
        s.base = "relative".into();
        assert_eq!(s.validate().map_err(|e| e.field), Err("paths"));
        let mut s = spec();
        s.hostname = "has space".into();
        assert_eq!(s.validate().map_err(|e| e.field), Err("hostname"));
        let mut s = spec();
        s.uid.count = 0;
        assert_eq!(s.validate().map_err(|e| e.field), Err("uid/gid"));
        let mut s = spec();
        s.port_first = 20_020;
        assert_eq!(s.validate().map_err(|e| e.field), Err("ports"));
        let mut s = spec();
        s.forwards = vec![0];
        assert_eq!(s.validate().map_err(|e| e.field), Err("forwards"));
        let mut s = spec();
        s.sandbox = "../x".into();
        assert_eq!(s.validate().map_err(|e| e.field), Err("sandbox"));
    }
}
