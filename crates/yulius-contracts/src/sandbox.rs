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
//! | `docker.json` | server | [`DockerFrontSpec`] (ADR-0158) |
//! | `docker-front.json` | server | [`DockerFrontRecord`] once the front serves |
//! | `docker/docker.sock` | the Docker front | the sandbox's Docker socket, `docker/` bound into runs |
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

/// System directories a joined run (and the engine) gets writable from the sandbox's system
/// layer.
pub const SYSTEM_LAYER_DIRS: [&str; 9] = [
    "/usr", "/etc", "/var", "/opt", "/root", "/home", "/srv", "/mnt", "/media",
];

/// The merged-`/usr` links of a joined run's root: (target, link).
pub const ROOT_LINKS: [(&str, &str); 4] = [
    ("usr/bin", "/bin"),
    ("usr/sbin", "/sbin"),
    ("usr/lib", "/lib"),
    ("usr/lib64", "/lib64"),
];

/// The sandbox's Docker socket in the state directory, served by the holder (ADR-0151).
pub const ENGINE_SOCKET: &str = "docker.sock";
/// The engine's own control directory in the state directory (its `docker.sock`).
pub const ENGINE_CONTROL_DIR: &str = "engine";
/// Mount point of the engine's sysfs in the state directory (holder's mount namespace).
pub const ENGINE_SYSFS_DIR: &str = "sys";
/// The engine's output.
pub const ENGINE_LOG_FILE: &str = "engine.log";
/// Where runs and the engine find the sandbox's Docker socket by default (`/var/run` links to
/// `/run`): a link to [`DOCKER_SOCKET_LINK`].
pub const ENGINE_SOCKET_IN_RUNS: &str = "/run/docker.sock";
/// The sandbox's Docker directory in the state directory (ADR-0158): it holds only the socket the
/// Docker front serves, and is bound into runs as a directory, so a recreated socket reaches
/// them.
pub const DOCKER_DIR: &str = "docker";
/// Where runs and the engine see the Docker directory.
pub const DOCKER_DIR_IN_RUNS: &str = "/run/yulius/docker";
/// Target of the link at [`ENGINE_SOCKET_IN_RUNS`], relative to `/run`.
pub const DOCKER_SOCKET_LINK: &str = "yulius/docker/docker.sock";
/// `DOCKER_HOST` of runs.
pub const DOCKER_HOST_IN_RUNS: &str = "unix:///run/yulius/docker/docker.sock";
/// The server's spec of the sandbox's Docker front.
pub const DOCKER_FRONT_SPEC_FILE: &str = "docker.json";
/// The running front's record.
pub const DOCKER_FRONT_RECORD_FILE: &str = "docker-front.json";
/// The front's diagnostics.
pub const DOCKER_FRONT_LOG_FILE: &str = "docker-front.log";
/// Where the engine sees its control directory.
pub const ENGINE_CONTROL_IN_ENGINE: &str = "/run/yulius-engine";

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
    /// Containers may run in the sandbox (ADR-0151): forwarded ports listen on every IPv4
    /// address of its network, so containers reach them at `host.docker.internal`. The engine
    /// itself is the Docker front's (ADR-0158).
    #[serde(default)]
    pub containers: bool,
}

/// How the holder starts the sandbox's Docker engine on demand (ADR-0151).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSpec {
    /// Program (absolute) and arguments: `yulius sandbox-exec --engine --cgroup … -- bwrap …`.
    pub command: Vec<String>,
    /// The engine's cgroup directory; a process in a group below it is a running container.
    pub cgroup: String,
    /// Seconds without a client and without a container before the engine stops.
    pub idle_secs: u64,
}

impl EngineSpec {
    fn validate(&self) -> Result<(), LaunchSpecError> {
        let program_ok = self.command.first().is_some_and(|p| abs(p));
        if !program_ok
            || self.command.len() > 512
            || self
                .command
                .iter()
                .any(|a| a.contains('\0') || a.len() > 4096)
        {
            return Err(spec_err(
                "engine.command",
                "an absolute program and at most 512 arguments",
            ));
        }
        if !abs(&self.cgroup) {
            return Err(spec_err("engine.cgroup", "must be absolute and not /"));
        }
        if !(60..=86_400).contains(&self.idle_secs) {
            return Err(spec_err("engine.idle_secs", "between 60 and 86400"));
        }
        Ok(())
    }
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

/// What a sandbox's production holder adds to the sandbox's id (SPEC-279, ADR-0208): `.` is
/// outside the ids' alphabet, so no sandbox has the name.
pub const PRODUCTION_SUFFIX: &str = ".prod";

/// The name of `sandbox`'s production holder: its directories under `runtime/sandboxes/` and
/// `sysroots/`, and its cgroup.
#[must_use]
pub fn production_holder(sandbox: &str) -> String {
    format!("{sandbox}{PRODUCTION_SUFFIX}")
}

/// The sandbox whose production holder `name` is; `None` for any other name.
#[must_use]
pub fn production_sandbox(name: &str) -> Option<&str> {
    name.strip_suffix(PRODUCTION_SUFFIX)
        .filter(|sandbox| valid_sandbox_id(sandbox))
}

/// `<data>/cd/<sandbox>/runs`: where production-stage runs of `sandbox` have their directories,
/// bound into its production engine at the same path (SPEC-279 PDE-004).
#[must_use]
pub fn production_runs_dir(data_dir: &std::path::Path, sandbox: &str) -> std::path::PathBuf {
    data_dir.join("cd").join(sandbox).join("runs")
}

/// Whether `name` names a holder: a sandbox's id or its production holder's name. Either is one
/// path component that cannot leave its directory.
#[must_use]
pub fn valid_holder_name(name: &str) -> bool {
    valid_sandbox_id(name) || production_sandbox(name).is_some()
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
        if !valid_holder_name(&self.sandbox) {
            return Err(spec_err(
                "sandbox",
                "must be 1-64 of [A-Za-z0-9_-], or that and .prod",
            ));
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

/// How the sandbox's Docker front serves its socket (ADR-0158).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DockerServe {
    /// The sandbox's own engine, started on the first connection.
    Engine {
        /// How it starts.
        engine: EngineSpec,
    },
    /// A holder that serves an engine itself (SPEC-187): bytes go to its socket.
    Relay {
        /// That socket (`<state>/docker.sock`).
        socket: String,
    },
    /// No engine is possible: every request is answered with this reason.
    Refuse {
        /// Why, and the operator's fix.
        reason: String,
    },
}

impl DockerServe {
    /// `engine`, `relay` or `refuse`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Engine { .. } => "engine",
            Self::Relay { .. } => "relay",
            Self::Refuse { .. } => "refuse",
        }
    }
}

/// What the server asks of a sandbox's Docker front (`docker.json`). Paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerFrontSpec {
    /// Record version ([`HOLDER_RECORD_VERSION`]).
    pub version: u32,
    /// Sandbox identifier.
    pub sandbox: String,
    /// The holder's state directory.
    pub state_dir: String,
    /// The holder process, whose user, network and mount namespaces the front joins.
    pub holder_pid: u32,
    /// Expected inode of its user namespace.
    pub user_ns: u64,
    /// Expected inode of its network namespace.
    pub net_ns: u64,
    /// Expected inode of its mount namespace.
    pub mnt_ns: u64,
    /// The sandbox's `system` group, joined first; `None` without delegation.
    #[serde(default)]
    pub cgroup: Option<String>,
    /// The mode.
    pub serve: DockerServe,
    /// Forwarded ports the front also serves on the engine's bridge gateway, for a holder whose
    /// forwards listen on loopback only (started before engines).
    #[serde(default)]
    pub bridge_forwards: Vec<u16>,
}

impl DockerFrontSpec {
    /// Validates every field.
    ///
    /// # Errors
    /// The first offending field.
    pub fn validate(&self) -> Result<(), LaunchSpecError> {
        if self.version != HOLDER_RECORD_VERSION {
            return Err(spec_err("version", "unsupported record version"));
        }
        if !valid_holder_name(&self.sandbox) {
            return Err(spec_err(
                "sandbox",
                "must be 1-64 of [A-Za-z0-9_-], or that and .prod",
            ));
        }
        if !abs(&self.state_dir) || self.cgroup.as_deref().is_some_and(|p| !abs(p)) {
            return Err(spec_err("paths", "must be absolute and not /"));
        }
        if self.holder_pid <= 1 {
            return Err(spec_err("holder_pid", "must be a process"));
        }
        if self.bridge_forwards.len() > 16 || self.bridge_forwards.contains(&0) {
            return Err(spec_err("bridge_forwards", "at most 16 ports, none 0"));
        }
        match &self.serve {
            DockerServe::Engine { engine } => engine.validate(),
            DockerServe::Relay { socket } if !abs(socket) => {
                Err(spec_err("serve.socket", "must be absolute and not /"))
            }
            DockerServe::Refuse { reason } if reason.is_empty() || reason.len() > 2000 => {
                Err(spec_err("serve.reason", "1 to 2000 bytes"))
            }
            _ => Ok(()),
        }
    }
}

/// A running Docker front (`docker-front.json`), written by the server once it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerFrontRecord {
    /// The front process.
    pub pid: u32,
    /// Its mode ([`DockerServe::kind`]).
    pub mode: String,
    /// The refusal's reason in refuse mode.
    #[serde(default)]
    pub reason: Option<String>,
    /// When it started serving (ms since the epoch).
    pub started_at_ms: i64,
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
    /// The holder itself serves an engine at `<state>/docker.sock` (SPEC-187 holders): their
    /// Docker front relays to it. `false` for holders from before engines and since ADR-0158,
    /// whose front runs the engine.
    #[serde(default)]
    pub engine: bool,
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
            containers: false,
        }
    }

    fn engine() -> EngineSpec {
        EngineSpec {
            command: vec![
                "/usr/local/bin/yulius".into(),
                "sandbox-exec".into(),
                "--engine".into(),
            ],
            cgroup: "/sys/fs/cgroup/yulius.service/sandboxes/s1/engine".into(),
            idle_secs: 300,
        }
    }

    fn front(serve: DockerServe) -> DockerFrontSpec {
        DockerFrontSpec {
            version: HOLDER_RECORD_VERSION,
            sandbox: "s1".into(),
            state_dir: "/data/runtime/sandboxes/s1".into(),
            holder_pid: 4242,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: Some("/sys/fs/cgroup/yulius.service/sandboxes/s1/system".into()),
            serve,
            bridge_forwards: vec![8081],
        }
    }

    /// SPEC-196 DKL-002: a front spec round-trips in each mode, its engine is validated as in
    /// SPEC-187, and a SPEC-187 `holder.json` (with `engine`) still reads.
    #[test]
    fn a_front_spec_is_validated_in_each_mode() -> Result<(), Box<dyn std::error::Error>> {
        for serve in [
            DockerServe::Engine { engine: engine() },
            DockerServe::Relay {
                socket: "/data/runtime/sandboxes/s1/docker.sock".into(),
            },
            DockerServe::Refuse {
                reason: "off".into(),
            },
        ] {
            let spec = front(serve);
            spec.validate()?;
            let json = serde_json::to_string(&spec)?;
            assert!(
                json.contains(&format!("\"mode\":\"{}\"", spec.serve.kind())),
                "{json}"
            );
            assert_eq!(serde_json::from_str::<DockerFrontSpec>(&json)?, spec);
        }
        for (field, change) in [
            (
                "engine.command",
                Box::new(|e: &mut EngineSpec| e.command.clear()) as Box<dyn Fn(&mut EngineSpec)>,
            ),
            (
                "engine.command",
                Box::new(|e: &mut EngineSpec| e.command[0] = "yulius".into()),
            ),
            (
                "engine.cgroup",
                Box::new(|e: &mut EngineSpec| e.cgroup = "relative".into()),
            ),
            (
                "engine.idle_secs",
                Box::new(|e: &mut EngineSpec| e.idle_secs = 5),
            ),
        ] {
            let mut e = engine();
            change(&mut e);
            let spec = front(DockerServe::Engine { engine: e });
            assert_eq!(spec.validate().map_err(|e| e.field), Err(field));
        }
        let mut bad = front(DockerServe::Refuse {
            reason: String::new(),
        });
        assert_eq!(bad.validate().map_err(|e| e.field), Err("serve.reason"));
        bad.serve = DockerServe::Relay {
            socket: "rel".into(),
        };
        assert_eq!(bad.validate().map_err(|e| e.field), Err("serve.socket"));
        bad.serve = DockerServe::Refuse { reason: "r".into() };
        bad.holder_pid = 1;
        assert_eq!(bad.validate().map_err(|e| e.field), Err("holder_pid"));

        let mut old = serde_json::to_value(spec())?;
        old["engine"] = serde_json::to_value(engine())?;
        if let Some(map) = old.as_object_mut() {
            map.remove("containers");
        }
        let read: HolderSpec = serde_json::from_value(old)?;
        assert!(!read.containers);
        Ok(())
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

    /// SPEC-279 PDE-001: a sandbox's production holder has a name no sandbox can have, and the
    /// holder and front specs take it; nothing else with a `.` passes.
    #[test]
    fn a_production_holder_is_named_apart_from_every_sandbox() {
        let name = production_holder("s1");
        assert_eq!(name, "s1.prod");
        assert!(!valid_sandbox_id(&name), "no sandbox id has a dot");
        assert!(valid_holder_name(&name));
        assert_eq!(production_sandbox(&name), Some("s1"));
        assert_eq!(production_sandbox("s1"), None);
        for bad in [".prod", "s1.prod.prod", "a.b", "../x.prod", "s1.prod/", ""] {
            assert!(!valid_holder_name(bad), "{bad}");
            assert_eq!(production_sandbox(bad), None, "{bad}");
        }
        assert!(valid_holder_name(&"a".repeat(64)));
        assert!(valid_holder_name(&production_holder(&"a".repeat(64))));
        let mut s = spec();
        s.sandbox = name.clone();
        assert_eq!(s.validate().map_err(|e| e.field), Ok(()));
        s.sandbox = "a.b".into();
        assert_eq!(s.validate().map_err(|e| e.field), Err("sandbox"));
    }
}
