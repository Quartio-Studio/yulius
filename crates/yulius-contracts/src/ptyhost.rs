//! ptyhost wire protocol and run-directory records, version 1 (SPEC-002 §7.1). Owned by work
//! package A.
//!
//! # Binary framing
//!
//! Every frame is `u32` big-endian length of (type + payload), one `u8` type and the payload.
//! All integers are big-endian. A length above [`MAX_FRAME_BYTES`] is rejected as soon as the
//! four length bytes are available, before any payload is buffered.
//!
//! | Type | Direction | Frame | Payload |
//! | --- | --- | --- | --- |
//! | `0x01` | C→H | [`ClientFrame::Hello`] | `u16` version, 32-byte token, `u8` want_snapshot (0/1) |
//! | `0x02` | C→H | [`ClientFrame::Input`] | bytes (≤ [`MAX_INPUT_CHUNK`]) |
//! | `0x03` | C→H | [`ClientFrame::Resize`] | `u16` cols, `u16` rows (each 1..=1000) |
//! | `0x04` | C→H | [`ClientFrame::Signal`] | `u8` 1 interrupt, 2 terminate, 3 kill |
//! | `0x05` | C→H | [`ClientFrame::Shutdown`] | `u32` grace milliseconds |
//! | `0x06` | C→H | [`ClientFrame::Ping`] | `u64` nonce |
//! | `0x81` | H→C | [`HostFrame::Welcome`] | `u16` id length, id UTF-8, `u32` child pid, `u64` seq, `u16` cols, `u16` rows |
//! | `0x82` | H→C | [`HostFrame::Snapshot`] | `u64` seq, formatted screen bytes |
//! | `0x83` | H→C | [`HostFrame::Output`] | `u64` seq, bytes (≤ [`MAX_OUTPUT_CHUNK`]) |
//! | `0x84` | H→C | [`HostFrame::Exit`] | `u8` flags (bit 0 code, bit 1 signal), `i32` code, `i32` signal, `i64` ended_at_ms, `u8` reason, detail UTF-8 |
//! | `0x85` | H→C | [`HostFrame::Pong`] | `u64` nonce |
//! | `0x86` | H→C | [`HostFrame::Error`] | `u16` code, message UTF-8 |
//!
//! `seq` counts output chunks of one attempt: each `Output` carries the previous value plus one,
//! `Welcome.seq` and `Snapshot.seq` are the sequence of the last chunk already reflected in the
//! screen (0 before any output). A client therefore sees a gap-free sequence after its snapshot.
//!
//! # Run directory records
//!
//! The server writes [`LaunchSpec`] as `launch.json`; the host writes [`StatusRecord`] as
//! `status.json` once the child runs and [`ExitFile`] as `exit.json` once the child has been
//! reaped (or spawning failed). Each JSON record carries `version` ([`RECORD_VERSION`]); readers
//! reject newer versions and ignore unknown fields. Files are replaced atomically (temporary file,
//! fsync, rename, directory fsync).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Wire protocol version sent in [`ClientFrame::Hello`].
pub const PROTOCOL_VERSION: u16 = 1;
/// Version of the JSON run-directory records ([`LaunchSpec`], [`StatusRecord`], [`ExitFile`]).
pub const RECORD_VERSION: u32 = 1;
/// Largest accepted value of the length prefix (type byte plus payload).
pub const MAX_FRAME_BYTES: usize = 1 << 20;
/// Largest payload of one [`HostFrame::Output`]; the host splits longer reads.
pub const MAX_OUTPUT_CHUNK: usize = 64 * 1024;
/// Largest payload of one [`ClientFrame::Input`]; split longer input with [`input_frames`].
pub const MAX_INPUT_CHUNK: usize = 64 * 1024;
/// Largest formatted screen carried by one [`HostFrame::Snapshot`].
pub const MAX_SNAPSHOT_BYTES: usize = MAX_FRAME_BYTES - 1 - 8;
/// Smallest terminal dimension accepted for `cols` and `rows`.
pub const MIN_DIMENSION: u16 = 1;
/// Largest terminal dimension accepted for `cols` and `rows`.
pub const MAX_DIMENSION: u16 = 1000;
/// Length of the per-attempt client token in bytes.
pub const TOKEN_BYTES: usize = 32;
/// Time the host grants a new connection to deliver a valid `Hello`.
pub const HELLO_TIMEOUT_MS: u64 = 5_000;
/// Default per-client output queue bound (SPEC-002 §9: 8 MiB, overflow drops the client).
pub const DEFAULT_CLIENT_QUEUE_BYTES: usize = 8 << 20;
/// Smallest accepted explicit client queue bound; it must hold a full snapshot frame.
pub const MIN_CLIENT_QUEUE_BYTES: usize = 2 << 20;
/// Largest accepted explicit client queue bound.
pub const MAX_CLIENT_QUEUE_BYTES: usize = 64 << 20;
/// Default and maximum time the host waits after child exit for a client to receive `Exit`.
pub const DEFAULT_LINGER_MS: u32 = 30_000;
/// Largest accepted [`LaunchSpec::linger_ms`].
pub const MAX_LINGER_MS: u32 = 30_000;
/// Largest [`ExitRecord::detail`] the host writes; longer diagnostics are truncated.
pub const MAX_EXIT_DETAIL_BYTES: usize = 1024;
/// Largest [`HostFrame::Error`] message the host writes.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;

/// File name of the launch record inside a run directory.
pub const LAUNCH_FILE: &str = "launch.json";
/// File name of the status record inside a run directory.
pub const STATUS_FILE: &str = "status.json";
/// File name of the exit record inside a run directory.
pub const EXIT_FILE: &str = "exit.json";
/// File name of the host's Unix socket inside a run directory.
pub const SOCKET_FILE: &str = "pty.sock";

/// Codes carried by [`HostFrame::Error`].
pub mod error_code {
    /// Another authenticated client took control; this connection is closed.
    pub const REPLACED: u16 = 1;
    /// The `Hello` token did not match the attempt token.
    pub const AUTH_FAILED: u16 = 2;
    /// The `Hello` protocol version is not supported.
    pub const UNSUPPORTED_VERSION: u16 = 3;
    /// A malformed or unexpected frame was received; this connection is closed.
    pub const PROTOCOL: u16 = 4;
    /// No valid `Hello` arrived within [`super::HELLO_TIMEOUT_MS`].
    pub const HELLO_TIMEOUT: u16 = 5;
    /// Too many connections are waiting for authentication.
    pub const BUSY: u16 = 6;
    /// The host could not perform a valid request (for example a PTY resize failed).
    pub const INTERNAL: u16 = 7;
}

const T_HELLO: u8 = 0x01;
const T_INPUT: u8 = 0x02;
const T_RESIZE: u8 = 0x03;
const T_SIGNAL: u8 = 0x04;
const T_SHUTDOWN: u8 = 0x05;
const T_PING: u8 = 0x06;
const T_WELCOME: u8 = 0x81;
const T_SNAPSHOT: u8 = 0x82;
const T_OUTPUT: u8 = 0x83;
const T_EXIT: u8 = 0x84;
const T_PONG: u8 = 0x85;
const T_ERROR: u8 = 0x86;

/// Signal requested by [`ClientFrame::Signal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// Write the interrupt character (`0x03`) to the PTY.
    Interrupt,
    /// Send `SIGTERM` to the child's process group.
    Terminate,
    /// Send `SIGKILL` to the child's process group.
    Kill,
}

impl SignalKind {
    fn to_byte(self) -> u8 {
        match self {
            Self::Interrupt => 1,
            Self::Terminate => 2,
            Self::Kill => 3,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Interrupt),
            2 => Some(Self::Terminate),
            3 => Some(Self::Kill),
            _ => None,
        }
    }
}

/// Frame sent by the controlling client (the server) to the host.
#[derive(Clone, PartialEq, Eq)]
pub enum ClientFrame {
    /// First frame of every connection: authenticates and requests the initial state.
    Hello {
        /// Protocol version spoken by the client ([`PROTOCOL_VERSION`]).
        version: u16,
        /// The attempt token whose SHA-256 is recorded in [`LaunchSpec::token_sha256`].
        token: [u8; TOKEN_BYTES],
        /// Whether the host sends a [`HostFrame::Snapshot`] after the welcome.
        want_snapshot: bool,
    },
    /// Bytes written to the PTY (at most [`MAX_INPUT_CHUNK`]).
    Input(Vec<u8>),
    /// New terminal size; each dimension in `MIN_DIMENSION..=MAX_DIMENSION`.
    Resize {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
    /// Deliver an interrupt or a signal to the child.
    Signal(SignalKind),
    /// Graceful stop: interrupt, wait `grace_ms`, terminate, wait `grace_ms`, kill.
    Shutdown {
        /// Grace period between escalation steps, in milliseconds.
        grace_ms: u32,
    },
    /// Liveness probe answered by [`HostFrame::Pong`] with the same nonce.
    Ping(u64),
}

impl fmt::Debug for ClientFrame {
    // The token is a credential: never print it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello {
                version,
                want_snapshot,
                ..
            } => f
                .debug_struct("Hello")
                .field("version", version)
                .field("token", &"<redacted>")
                .field("want_snapshot", want_snapshot)
                .finish(),
            Self::Input(bytes) => f.debug_tuple("Input").field(&bytes.len()).finish(),
            Self::Resize { cols, rows } => f
                .debug_struct("Resize")
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            Self::Signal(kind) => f.debug_tuple("Signal").field(kind).finish(),
            Self::Shutdown { grace_ms } => f
                .debug_struct("Shutdown")
                .field("grace_ms", grace_ms)
                .finish(),
            Self::Ping(nonce) => f.debug_tuple("Ping").field(nonce).finish(),
        }
    }
}

impl ClientFrame {
    /// Checks the limits the decoder enforces, so a sender never emits a frame the host rejects.
    ///
    /// # Errors
    /// Returns [`ProtocolError::InvalidValue`] for oversize input or out-of-range dimensions.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Input(bytes) if bytes.len() > MAX_INPUT_CHUNK => {
                Err(invalid("Input", "input chunk exceeds MAX_INPUT_CHUNK"))
            }
            Self::Resize { cols, rows } if !valid_dimension(*cols) || !valid_dimension(*rows) => {
                Err(invalid("Resize", "dimension outside 1..=1000"))
            }
            _ => Ok(()),
        }
    }
}

/// Splits arbitrary input into [`ClientFrame::Input`] frames of at most [`MAX_INPUT_CHUNK`].
pub fn input_frames(bytes: &[u8]) -> impl Iterator<Item = ClientFrame> + '_ {
    bytes
        .chunks(MAX_INPUT_CHUNK)
        .map(|chunk| ClientFrame::Input(chunk.to_vec()))
}

/// Host greeting sent after a successful `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
    /// Attempt served by this host.
    pub attempt_id: String,
    /// Process id of the child (the agent, or `bwrap` when contained).
    pub child_pid: u32,
    /// Sequence of the last output chunk already reflected in the screen.
    pub seq: u64,
    /// Current terminal columns.
    pub cols: u16,
    /// Current terminal rows.
    pub rows: u16,
}

/// Full screen redraw: clear the terminal, then write `bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Sequence of the last output chunk reflected in `bytes`.
    pub seq: u64,
    /// Escape sequences reproducing the visible screen, input modes and cursor.
    pub bytes: Vec<u8>,
}

/// Frame sent by the host to its controlling client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFrame {
    /// Greeting after authentication.
    Welcome(Welcome),
    /// Screen snapshot (only when requested in `Hello`).
    Snapshot(Snapshot),
    /// Live PTY output.
    Output {
        /// Monotonic per-attempt sequence of this chunk.
        seq: u64,
        /// Raw PTY bytes (at most [`MAX_OUTPUT_CHUNK`]).
        bytes: Vec<u8>,
    },
    /// The child has been reaped; also written to `exit.json` before this frame is sent.
    Exit(ExitRecord),
    /// Answer to [`ClientFrame::Ping`].
    Pong(u64),
    /// Error report; see [`error_code`]. Fatal codes are followed by connection close.
    Error {
        /// One of the [`error_code`] constants.
        code: u16,
        /// Human-readable diagnostic without secrets.
        message: String,
    },
}

impl HostFrame {
    /// Checks the limits the decoder enforces, so the host never emits an undecodable frame.
    ///
    /// # Errors
    /// Returns [`ProtocolError::InvalidValue`] when a payload exceeds its bound.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Welcome(w) if w.attempt_id.len() > usize::from(u16::MAX) => {
                Err(invalid("Welcome", "attempt id too long"))
            }
            Self::Welcome(w) if !valid_dimension(w.cols) || !valid_dimension(w.rows) => {
                Err(invalid("Welcome", "dimension outside 1..=1000"))
            }
            Self::Snapshot(s) if s.bytes.len() > MAX_SNAPSHOT_BYTES => {
                Err(invalid("Snapshot", "snapshot exceeds MAX_SNAPSHOT_BYTES"))
            }
            Self::Output { bytes, .. } if bytes.len() > MAX_OUTPUT_CHUNK => {
                Err(invalid("Output", "output chunk exceeds MAX_OUTPUT_CHUNK"))
            }
            Self::Exit(record)
                if record.detail.as_ref().map_or(0, String::len) > MAX_FRAME_BYTES - 19 =>
            {
                Err(invalid("Exit", "detail too long"))
            }
            Self::Error { message, .. } if message.len() > MAX_FRAME_BYTES - 3 => {
                Err(invalid("Error", "message too long"))
            }
            _ => Ok(()),
        }
    }
}

/// Why the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    /// The child exited with a status code.
    Exited,
    /// The child was terminated by a signal.
    Signalled,
    /// The child could not be started (missing executable, invalid cwd, unsupported containment).
    SpawnFailed,
    /// The host itself failed (unreadable launch record, PTY creation, socket bind).
    HostError,
}

impl ExitReason {
    fn to_byte(self) -> u8 {
        match self {
            Self::Exited => 1,
            Self::Signalled => 2,
            Self::SpawnFailed => 3,
            Self::HostError => 4,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Exited),
            2 => Some(Self::Signalled),
            3 => Some(Self::SpawnFailed),
            4 => Some(Self::HostError),
            _ => None,
        }
    }
}

/// Terminal outcome of one run, sent as [`HostFrame::Exit`] and stored in `exit.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitRecord {
    /// Exit status code when the child exited normally.
    pub code: Option<i32>,
    /// Signal number when the child was killed by a signal.
    pub signal: Option<i32>,
    /// Wall-clock time the host observed the outcome, in Unix milliseconds.
    pub ended_at_ms: i64,
    /// Classification of the outcome.
    pub reason: ExitReason,
    /// Optional diagnostic without secrets (at most [`MAX_EXIT_DETAIL_BYTES`] from the host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// What a run's process tree used, measured by its host when it reaped the child (SPEC-154
/// USE-001): every descendant reaped down the child's chain counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessUsage {
    /// User plus system CPU time, in milliseconds.
    pub cpu_ms: u64,
    /// The largest peak resident set of any one of its processes, in bytes.
    pub max_rss_bytes: u64,
    /// Bytes its processes caused to be written to storage; `None` where the host does not
    /// account for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_bytes: Option<u64>,
}

/// Content of `exit.json`: a versioned wrapper around the [`ExitRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitFile {
    /// Record version ([`RECORD_VERSION`]).
    pub version: u32,
    /// Attempt the outcome belongs to.
    pub attempt_id: String,
    /// The outcome.
    pub exit: ExitRecord,
    /// What the run used, when its child ran and was measured (absent from older hosts' files;
    /// not part of the `Exit` frame, so the socket protocol is unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ProcessUsage>,
}

/// Bubblewrap containment of one run (SPEC-002 ADR-03). Paths are absolute host paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BubblewrapSpec {
    /// Sandbox directory, bound writable at the same path.
    pub sandbox_root: String,
    /// `HOME` inside the sandbox (bound writable when outside `sandbox_root`).
    pub home: String,
    /// Additional writable paths, bound at the same path.
    #[serde(default)]
    pub writable: Vec<String>,
    /// Additional read-only paths, bound at the same path after the writable ones.
    #[serde(default)]
    pub readonly: Vec<String>,
    /// Keep the host network namespace (agents need network); `false` unshares it.
    #[serde(default)]
    pub share_net: bool,
    /// Bind mounts whose path inside the sandbox differs from the host path, applied after
    /// the same-path binds (a bind whose destination is `home` replaces the home bind).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binds: Vec<BindMount>,
    /// Run as this identity in a new user namespace (user shells and editors, SPEC-002
    /// ADR-12); `None` keeps the caller's identity. Ignored when `join` is set (the holder's
    /// namespace already makes the run uid 0; its host name is `join.hostname`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<SandboxIdentity>,
    /// Join the sandbox's namespace holder (SPEC-002 ADR-22) instead of creating a user and
    /// network namespace: the run gets the sandbox's network, its writable system layer, its
    /// cgroup, a reduced capability set and the seccomp filter. `None`: the previous containment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<Box<SandboxJoin>>,
}

/// The sandbox namespace holder a contained run joins (SPEC-002 ADR-22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxJoin {
    /// The holder process inside the namespaces.
    pub holder_pid: u32,
    /// Expected inode of its user namespace (a reused pid is refused).
    pub user_ns: u64,
    /// Expected inode of its network namespace.
    pub net_ns: u64,
    /// Expected inode of its mount namespace.
    pub mnt_ns: u64,
    /// The sandbox's cgroup directory, joined first; `None` without delegation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<String>,
    /// The merged system layer in the holder's mount namespace.
    pub system_root: String,
    /// Host name inside the run (the sandbox slug).
    pub hostname: String,
    /// `yulius` executable whose `sandbox-exec` installs the seccomp filter and execs `bwrap`;
    /// `None`: no filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_shim: Option<String>,
    /// Directories covered by an empty tmpfs before the run's own binds (Yulius's data
    /// directory), so their mount points never land in the sandbox's system layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private_dirs: Vec<String>,
    /// Run on the host's system instead of the sandbox's system layer (a production CD run,
    /// SPEC-134): the host's `/usr`, `/etc` and `/opt` read-only, `/var`, `/root`, `/home`,
    /// `/srv`, `/mnt` and `/media` empty, in the holder's network. The value is the resolver file
    /// shown at the host's resolver path (where `/etc/resolv.conf` really points).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_system: Option<String>,
}

/// A bind mount at a different path inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindMount {
    /// Host path.
    pub source: String,
    /// Path inside the sandbox.
    pub dest: String,
    /// Writable (otherwise read-only).
    #[serde(default)]
    pub writable: bool,
}

/// Identity of the contained process in its own user namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxIdentity {
    /// User id inside the namespace (0: root of the sandbox).
    pub uid: u32,
    /// Group id inside the namespace.
    pub gid: u32,
    /// Host name inside the namespace (the sandbox slug).
    pub hostname: String,
}

/// Process containment of the child.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Containment {
    /// Host mode: no containment (development and tests; flagged "not contained").
    #[default]
    None,
    /// Linux bubblewrap namespace.
    Bubblewrap(BubblewrapSpec),
}

fn default_linger_ms() -> u32 {
    DEFAULT_LINGER_MS
}

/// Content of `launch.json`, written by the server before spawning the host.
///
/// Secret values never appear here: [`LaunchSpec::secret_env_names`] lists variables the host
/// copies from its own environment into the child's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    /// Record version ([`RECORD_VERSION`]).
    pub version: u32,
    /// Attempt identifier (1–64 ASCII letters, digits, `-`, `_`).
    pub attempt_id: String,
    /// Task identifier (same syntax as `attempt_id`).
    pub task_id: String,
    /// Executable and arguments of the agent; never interpreted by a shell.
    pub argv: Vec<String>,
    /// Absolute working directory of the child (must exist).
    pub cwd: String,
    /// Complete non-secret environment of the child.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names of secret variables copied from the host environment into the child.
    #[serde(default)]
    pub secret_env_names: Vec<String>,
    /// Initial terminal columns.
    pub cols: u16,
    /// Initial terminal rows.
    pub rows: u16,
    /// Process containment.
    #[serde(default)]
    pub containment: Containment,
    /// Bound of the output queue (ring) between the PTY reader and the controlling client, in
    /// bytes; overflow drops the client. `0` selects [`DEFAULT_CLIENT_QUEUE_BYTES`]; otherwise
    /// `MIN_CLIENT_QUEUE_BYTES..=MAX_CLIENT_QUEUE_BYTES`.
    #[serde(default)]
    pub ring_bytes: usize,
    /// Lower-case hex SHA-256 of the 32-byte client token.
    pub token_sha256: String,
    /// How long the host waits after child exit for a client to receive `Exit`
    /// (at most [`MAX_LINGER_MS`]).
    #[serde(default = "default_linger_ms")]
    pub linger_ms: u32,
}

/// A rejected [`LaunchSpec`] field. Carries no field values, so it is safe to log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid launch spec field `{field}`: {reason}")]
pub struct LaunchSpecError {
    /// Offending field.
    pub field: &'static str,
    /// Why it was rejected.
    pub reason: &'static str,
}

const MAX_ARGV_ITEMS: usize = 4096;
const MAX_ARGV_BYTES: usize = 256 * 1024;
const MAX_ENV_ITEMS: usize = 1024;
const MAX_ENV_BYTES: usize = 256 * 1024;
const MAX_SECRET_NAMES: usize = 64;
const MAX_PATHS: usize = 256;

fn spec_err(field: &'static str, reason: &'static str) -> LaunchSpecError {
    LaunchSpecError { field, reason }
}

/// Whether `text` is a valid attempt/task identifier (1–64 ASCII letters, digits, `-`, `_`).
#[must_use]
pub fn valid_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `name` is a usable environment variable name (non-empty, no `=` or NUL).
#[must_use]
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 256 && !name.contains(['=', '\0'])
}

fn valid_abs_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains('\0') && path.len() <= 4096
}

fn valid_dimension(value: u16) -> bool {
    (MIN_DIMENSION..=MAX_DIMENSION).contains(&value)
}

/// Whether `text` is 64 lower-case hexadecimal characters.
#[must_use]
pub fn valid_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl LaunchSpec {
    /// Validates every field against the bounds of this contract.
    ///
    /// # Errors
    /// Returns the first offending field.
    pub fn validate(&self) -> Result<(), LaunchSpecError> {
        if self.version != RECORD_VERSION {
            return Err(spec_err("version", "unsupported record version"));
        }
        if !valid_identifier(&self.attempt_id) {
            return Err(spec_err("attempt_id", "must be 1-64 of [A-Za-z0-9_-]"));
        }
        if !valid_identifier(&self.task_id) {
            return Err(spec_err("task_id", "must be 1-64 of [A-Za-z0-9_-]"));
        }
        self.validate_argv()?;
        if !valid_abs_path(&self.cwd) {
            return Err(spec_err("cwd", "must be an absolute path without NUL"));
        }
        self.validate_env()?;
        if !valid_dimension(self.cols) {
            return Err(spec_err("cols", "must be within 1..=1000"));
        }
        if !valid_dimension(self.rows) {
            return Err(spec_err("rows", "must be within 1..=1000"));
        }
        if let Containment::Bubblewrap(spec) = &self.containment {
            validate_bubblewrap(spec)?;
        }
        if self.ring_bytes != 0
            && !(MIN_CLIENT_QUEUE_BYTES..=MAX_CLIENT_QUEUE_BYTES).contains(&self.ring_bytes)
        {
            return Err(spec_err("ring_bytes", "must be 0 or within 2 MiB..=64 MiB"));
        }
        if !valid_sha256_hex(&self.token_sha256) {
            return Err(spec_err("token_sha256", "must be 64 lower-case hex digits"));
        }
        if self.linger_ms > MAX_LINGER_MS {
            return Err(spec_err("linger_ms", "must be at most 30000"));
        }
        Ok(())
    }

    /// The effective client queue bound: [`LaunchSpec::ring_bytes`] or the default.
    #[must_use]
    pub fn client_queue_bytes(&self) -> usize {
        if self.ring_bytes == 0 {
            DEFAULT_CLIENT_QUEUE_BYTES
        } else {
            self.ring_bytes
        }
    }

    fn validate_argv(&self) -> Result<(), LaunchSpecError> {
        let Some(program) = self.argv.first() else {
            return Err(spec_err("argv", "must name an executable"));
        };
        if program.is_empty() {
            return Err(spec_err("argv", "executable must not be empty"));
        }
        if self.argv.len() > MAX_ARGV_ITEMS {
            return Err(spec_err("argv", "too many arguments"));
        }
        if self.argv.iter().map(String::len).sum::<usize>() > MAX_ARGV_BYTES {
            return Err(spec_err("argv", "arguments too long"));
        }
        if self.argv.iter().any(|arg| arg.contains('\0')) {
            return Err(spec_err("argv", "arguments must not contain NUL"));
        }
        Ok(())
    }

    fn validate_env(&self) -> Result<(), LaunchSpecError> {
        if self.env.len() > MAX_ENV_ITEMS {
            return Err(spec_err("env", "too many variables"));
        }
        let mut total = 0usize;
        for (name, value) in &self.env {
            if !valid_env_name(name) {
                return Err(spec_err("env", "invalid variable name"));
            }
            if value.contains('\0') {
                return Err(spec_err("env", "values must not contain NUL"));
            }
            total = total.saturating_add(name.len()).saturating_add(value.len());
        }
        if total > MAX_ENV_BYTES {
            return Err(spec_err("env", "environment too large"));
        }
        if self.secret_env_names.len() > MAX_SECRET_NAMES {
            return Err(spec_err("secret_env_names", "too many names"));
        }
        for (index, name) in self.secret_env_names.iter().enumerate() {
            if !valid_env_name(name) {
                return Err(spec_err("secret_env_names", "invalid variable name"));
            }
            if self.env.contains_key(name) {
                return Err(spec_err(
                    "secret_env_names",
                    "name also present in the non-secret env",
                ));
            }
            if self.secret_env_names[..index].contains(name) {
                return Err(spec_err("secret_env_names", "duplicate name"));
            }
        }
        Ok(())
    }
}

fn validate_bubblewrap(spec: &BubblewrapSpec) -> Result<(), LaunchSpecError> {
    if !valid_abs_path(&spec.sandbox_root) || spec.sandbox_root == "/" {
        return Err(spec_err(
            "containment.sandbox_root",
            "must be an absolute path other than /",
        ));
    }
    if !valid_abs_path(&spec.home) {
        return Err(spec_err("containment.home", "must be an absolute path"));
    }
    if spec.writable.len() > MAX_PATHS || spec.readonly.len() > MAX_PATHS {
        return Err(spec_err("containment", "too many bind paths"));
    }
    if !spec.writable.iter().all(|p| valid_abs_path(p) && p != "/") {
        return Err(spec_err(
            "containment.writable",
            "paths must be absolute and not /",
        ));
    }
    if !spec.readonly.iter().all(|p| valid_abs_path(p)) {
        return Err(spec_err("containment.readonly", "paths must be absolute"));
    }
    if spec.binds.len() > MAX_PATHS
        || !spec
            .binds
            .iter()
            .all(|b| valid_abs_path(&b.source) && valid_abs_path(&b.dest) && b.dest != "/")
    {
        return Err(spec_err(
            "containment.binds",
            "sources and destinations must be absolute; the destination is not /",
        ));
    }
    if let Some(join) = &spec.join {
        let paths_ok = valid_abs_path(&join.system_root)
            && join.system_root != "/"
            && join.cgroup.as_deref().is_none_or(valid_abs_path)
            && join.exec_shim.as_deref().is_none_or(valid_abs_path)
            && join.host_system.as_deref().is_none_or(valid_abs_path)
            && join.private_dirs.len() <= 8
            && join
                .private_dirs
                .iter()
                .all(|d| valid_abs_path(d) && d != "/");
        if join.holder_pid <= 1 || !paths_ok {
            return Err(spec_err(
                "containment.join",
                "needs a holder pid and absolute paths",
            ));
        }
        if !crate::sandbox::valid_hostname(&join.hostname) {
            return Err(spec_err(
                "containment.join.hostname",
                "must be 1..=63 letters, digits or dashes",
            ));
        }
    }
    if let Some(identity) = &spec.identity {
        let host_ok = (1..=63).contains(&identity.hostname.len())
            && identity
                .hostname
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !host_ok {
            return Err(spec_err(
                "containment.identity.hostname",
                "must be 1..=63 letters, digits or dashes",
            ));
        }
    }
    Ok(())
}

/// Content of `status.json`, written by the host once the child is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusRecord {
    /// Record version ([`RECORD_VERSION`]).
    pub version: u32,
    /// Attempt served by the host.
    pub attempt_id: String,
    /// Process id of the host (leader of its own process group).
    pub ptyhost_pid: u32,
    /// Process id of the child (session leader of its own session).
    pub child_pid: u32,
    /// Wall-clock time the record was written, in Unix milliseconds. Both processes started at
    /// or before this instant, which lets reconciliation reject reused process ids.
    pub started_at_ms: i64,
    /// Kernel boot identifier at start; a different value means the processes are gone.
    pub boot_id: String,
    /// Lower-case hex SHA-256 of the client token (copied from the launch record).
    pub token_sha256: String,
}

/// Malformed frame or stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// The length prefix exceeds [`MAX_FRAME_BYTES`].
    #[error("frame length {len} exceeds the {MAX_FRAME_BYTES}-byte limit")]
    FrameTooLarge {
        /// Announced length.
        len: u32,
    },
    /// The length prefix is zero (no type byte).
    #[error("empty frame")]
    EmptyFrame,
    /// The type byte is not defined for this direction.
    #[error("unknown frame type 0x{0:02x}")]
    UnknownType(u8),
    /// The payload is shorter than the frame requires.
    #[error("truncated {frame} payload")]
    Truncated {
        /// Frame name.
        frame: &'static str,
    },
    /// The payload is longer than the frame allows.
    #[error("trailing bytes after {frame} payload")]
    TrailingBytes {
        /// Frame name.
        frame: &'static str,
    },
    /// A field holds a value outside its domain.
    #[error("invalid {frame} frame: {reason}")]
    InvalidValue {
        /// Frame name.
        frame: &'static str,
        /// Which constraint failed.
        reason: &'static str,
    },
    /// The stream ended inside a frame.
    #[error("stream ended inside a frame")]
    UnexpectedEof,
}

fn invalid(frame: &'static str, reason: &'static str) -> ProtocolError {
    ProtocolError::InvalidValue { frame, reason }
}

struct FrameWriter {
    buf: Vec<u8>,
}

impl FrameWriter {
    fn new(kind: u8, capacity: usize) -> Self {
        let mut buf = Vec::with_capacity(capacity.saturating_add(5));
        buf.extend_from_slice(&[0, 0, 0, 0, kind]);
        Self { buf }
    }

    fn u8(mut self, value: u8) -> Self {
        self.buf.push(value);
        self
    }

    fn u16(mut self, value: u16) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    fn u32(mut self, value: u32) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    fn u64(mut self, value: u64) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    fn i32(mut self, value: i32) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    fn i64(mut self, value: i64) -> Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    fn bytes(mut self, value: &[u8]) -> Self {
        self.buf.extend_from_slice(value);
        self
    }

    fn finish(mut self) -> Vec<u8> {
        // A length that does not fit is encoded as u32::MAX, which every decoder rejects.
        let len = u32::try_from(self.buf.len() - 4).unwrap_or(u32::MAX);
        self.buf[..4].copy_from_slice(&len.to_be_bytes());
        self.buf
    }
}

/// Encodes a client frame. Call [`ClientFrame::validate`] first when the content is untrusted.
#[must_use]
pub fn encode_client(frame: &ClientFrame) -> Vec<u8> {
    match frame {
        ClientFrame::Hello {
            version,
            token,
            want_snapshot,
        } => FrameWriter::new(T_HELLO, 35)
            .u16(*version)
            .bytes(token)
            .u8(u8::from(*want_snapshot))
            .finish(),
        ClientFrame::Input(bytes) => FrameWriter::new(T_INPUT, bytes.len()).bytes(bytes).finish(),
        ClientFrame::Resize { cols, rows } => {
            FrameWriter::new(T_RESIZE, 4).u16(*cols).u16(*rows).finish()
        }
        ClientFrame::Signal(kind) => FrameWriter::new(T_SIGNAL, 1).u8(kind.to_byte()).finish(),
        ClientFrame::Shutdown { grace_ms } => {
            FrameWriter::new(T_SHUTDOWN, 4).u32(*grace_ms).finish()
        }
        ClientFrame::Ping(nonce) => FrameWriter::new(T_PING, 8).u64(*nonce).finish(),
    }
}

/// Encodes a host frame. Call [`HostFrame::validate`] first when sizes are not already bounded.
#[must_use]
pub fn encode_host(frame: &HostFrame) -> Vec<u8> {
    match frame {
        HostFrame::Welcome(w) => {
            let id = w.attempt_id.as_bytes();
            let id_len = u16::try_from(id.len()).unwrap_or(u16::MAX);
            let id = id.get(..usize::from(id_len)).unwrap_or(id);
            FrameWriter::new(T_WELCOME, 18 + id.len())
                .u16(id_len)
                .bytes(id)
                .u32(w.child_pid)
                .u64(w.seq)
                .u16(w.cols)
                .u16(w.rows)
                .finish()
        }
        HostFrame::Snapshot(s) => FrameWriter::new(T_SNAPSHOT, 8 + s.bytes.len())
            .u64(s.seq)
            .bytes(&s.bytes)
            .finish(),
        HostFrame::Output { seq, bytes } => FrameWriter::new(T_OUTPUT, 8 + bytes.len())
            .u64(*seq)
            .bytes(bytes)
            .finish(),
        HostFrame::Exit(record) => {
            let flags = u8::from(record.code.is_some()) | (u8::from(record.signal.is_some()) << 1);
            let detail = record.detail.as_deref().unwrap_or("");
            FrameWriter::new(T_EXIT, 18 + detail.len())
                .u8(flags)
                .i32(record.code.unwrap_or(0))
                .i32(record.signal.unwrap_or(0))
                .i64(record.ended_at_ms)
                .u8(record.reason.to_byte())
                .bytes(detail.as_bytes())
                .finish()
        }
        HostFrame::Pong(nonce) => FrameWriter::new(T_PONG, 8).u64(*nonce).finish(),
        HostFrame::Error { code, message } => FrameWriter::new(T_ERROR, 2 + message.len())
            .u16(*code)
            .bytes(message.as_bytes())
            .finish(),
    }
}

struct Payload<'a> {
    bytes: &'a [u8],
    frame: &'static str,
}

impl<'a> Payload<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        if self.bytes.len() < n {
            return Err(ProtocolError::Truncated { frame: self.frame });
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, ProtocolError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.bytes)
    }

    fn utf8_rest(&mut self) -> Result<String, ProtocolError> {
        let frame = self.frame;
        String::from_utf8(self.rest().to_vec()).map_err(|_| invalid(frame, "text is not UTF-8"))
    }

    fn finish(&self) -> Result<(), ProtocolError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes { frame: self.frame })
        }
    }

    fn dimension(&mut self) -> Result<u16, ProtocolError> {
        let value = self.u16()?;
        if valid_dimension(value) {
            Ok(value)
        } else {
            Err(invalid(self.frame, "dimension outside 1..=1000"))
        }
    }
}

fn decode_client(kind: u8, bytes: &[u8]) -> Result<ClientFrame, ProtocolError> {
    let frame = match kind {
        T_HELLO => "Hello",
        T_INPUT => "Input",
        T_RESIZE => "Resize",
        T_SIGNAL => "Signal",
        T_SHUTDOWN => "Shutdown",
        T_PING => "Ping",
        other => return Err(ProtocolError::UnknownType(other)),
    };
    let mut p = Payload { bytes, frame };
    let decoded = match kind {
        T_HELLO => {
            let version = p.u16()?;
            let token = p.array::<TOKEN_BYTES>()?;
            let want_snapshot = match p.u8()? {
                0 => false,
                1 => true,
                _ => return Err(invalid(frame, "want_snapshot must be 0 or 1")),
            };
            ClientFrame::Hello {
                version,
                token,
                want_snapshot,
            }
        }
        T_INPUT => {
            let bytes = p.rest();
            if bytes.len() > MAX_INPUT_CHUNK {
                return Err(invalid(frame, "input chunk exceeds MAX_INPUT_CHUNK"));
            }
            ClientFrame::Input(bytes.to_vec())
        }
        T_RESIZE => ClientFrame::Resize {
            cols: p.dimension()?,
            rows: p.dimension()?,
        },
        T_SIGNAL => ClientFrame::Signal(
            SignalKind::from_byte(p.u8()?).ok_or_else(|| invalid(frame, "unknown signal kind"))?,
        ),
        T_SHUTDOWN => ClientFrame::Shutdown { grace_ms: p.u32()? },
        _ => ClientFrame::Ping(p.u64()?),
    };
    p.finish()?;
    Ok(decoded)
}

fn decode_host(kind: u8, bytes: &[u8]) -> Result<HostFrame, ProtocolError> {
    let frame = match kind {
        T_WELCOME => "Welcome",
        T_SNAPSHOT => "Snapshot",
        T_OUTPUT => "Output",
        T_EXIT => "Exit",
        T_PONG => "Pong",
        T_ERROR => "Error",
        other => return Err(ProtocolError::UnknownType(other)),
    };
    let mut p = Payload { bytes, frame };
    let decoded = match kind {
        T_WELCOME => {
            let len = usize::from(p.u16()?);
            let id = p.take(len)?;
            let attempt_id = String::from_utf8(id.to_vec())
                .map_err(|_| invalid(frame, "attempt id is not UTF-8"))?;
            HostFrame::Welcome(Welcome {
                attempt_id,
                child_pid: p.u32()?,
                seq: p.u64()?,
                cols: p.dimension()?,
                rows: p.dimension()?,
            })
        }
        T_SNAPSHOT => {
            let seq = p.u64()?;
            HostFrame::Snapshot(Snapshot {
                seq,
                bytes: p.rest().to_vec(),
            })
        }
        T_OUTPUT => {
            let seq = p.u64()?;
            let bytes = p.rest();
            if bytes.len() > MAX_OUTPUT_CHUNK {
                return Err(invalid(frame, "output chunk exceeds MAX_OUTPUT_CHUNK"));
            }
            HostFrame::Output {
                seq,
                bytes: bytes.to_vec(),
            }
        }
        T_EXIT => {
            let flags = p.u8()?;
            if flags & !0b11 != 0 {
                return Err(invalid(frame, "unknown flag bits"));
            }
            let code = p.i32()?;
            let signal = p.i32()?;
            let ended_at_ms = p.i64()?;
            let reason = ExitReason::from_byte(p.u8()?)
                .ok_or_else(|| invalid(frame, "unknown exit reason"))?;
            let detail = p.utf8_rest()?;
            HostFrame::Exit(ExitRecord {
                code: (flags & 1 != 0).then_some(code),
                signal: (flags & 2 != 0).then_some(signal),
                ended_at_ms,
                reason,
                detail: (!detail.is_empty()).then_some(detail),
            })
        }
        T_PONG => HostFrame::Pong(p.u64()?),
        _ => {
            let code = p.u16()?;
            HostFrame::Error {
                code,
                message: p.utf8_rest()?,
            }
        }
    };
    p.finish()?;
    Ok(decoded)
}

/// Incremental decoder for a byte stream of frames in either direction.
///
/// Feed received bytes with [`FrameDecoder::push`] and drain frames with
/// [`FrameDecoder::next_client`] or [`FrameDecoder::next_host`] until they return `Ok(None)`.
/// Memory stays bounded by one partial frame (at most [`MAX_FRAME_BYTES`] + 4) plus the bytes
/// of the last push, provided frames are drained after each push. After an error the stream is
/// desynchronized: every later call returns the same error and the connection must be dropped.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    start: usize,
    failed: Option<ProtocolError>,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start >= self.buf.len() / 2 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes received but not yet returned as frames.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Returns [`ProtocolError::UnexpectedEof`] when the stream ended inside a frame.
    ///
    /// # Errors
    /// Fails when buffered bytes remain at end of stream.
    pub fn finish(&self) -> Result<(), ProtocolError> {
        match &self.failed {
            Some(error) => Err(error.clone()),
            None if self.buffered() > 0 => Err(ProtocolError::UnexpectedEof),
            None => Ok(()),
        }
    }

    /// Decodes the next complete client-to-host frame.
    ///
    /// # Errors
    /// Fails on oversize or empty frames, unknown types, truncated or invalid payloads.
    pub fn next_client(&mut self) -> Result<Option<ClientFrame>, ProtocolError> {
        self.next_with(decode_client)
    }

    /// Decodes the next complete host-to-client frame.
    ///
    /// # Errors
    /// Fails on oversize or empty frames, unknown types, truncated or invalid payloads.
    pub fn next_host(&mut self) -> Result<Option<HostFrame>, ProtocolError> {
        self.next_with(decode_host)
    }

    fn next_with<T>(
        &mut self,
        decode: fn(u8, &[u8]) -> Result<T, ProtocolError>,
    ) -> Result<Option<T>, ProtocolError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.next_raw().and_then(|raw| match raw {
            Some((kind, start, end)) => {
                let payload = self.buf.get(start..end).unwrap_or_default();
                decode(kind, payload).map(Some)
            }
            None => Ok(None),
        });
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    /// Locates the next complete frame: `(type, payload start, payload end)`; consumes it.
    fn next_raw(&mut self) -> Result<Option<(u8, usize, usize)>, ProtocolError> {
        let available = &self.buf[self.start..];
        let Some(header) = available.get(..4) else {
            return Ok(None);
        };
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(header);
        let len = u32::from_be_bytes(len_bytes);
        let len_usize = usize::try_from(len).unwrap_or(usize::MAX);
        if len_usize > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge { len });
        }
        if len == 0 {
            return Err(ProtocolError::EmptyFrame);
        }
        if available.len() < 4 + len_usize {
            return Ok(None);
        }
        let kind = available[4];
        let payload_start = self.start + 5;
        let payload_end = self.start + 4 + len_usize;
        self.start = payload_end;
        Ok(Some((kind, payload_start, payload_end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Deterministic xorshift generator for property-style tests.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        fn bytes(&mut self, max: usize) -> Vec<u8> {
            let len = self.below(max + 1);
            (0..len).map(|_| (self.next() & 0xff) as u8).collect()
        }

        fn dimension(&mut self) -> u16 {
            u16::try_from(1 + self.below(1000)).unwrap_or(1)
        }

        fn text(&mut self, max: usize) -> String {
            let len = self.below(max + 1);
            (0..len)
                .map(|_| match self.below(4) {
                    0 => 'é',
                    1 => '✓',
                    _ => char::from(b'a' + (self.next() % 26) as u8),
                })
                .collect()
        }
    }

    fn random_client(rng: &mut Rng) -> ClientFrame {
        match rng.below(6) {
            0 => {
                let mut token = [0u8; TOKEN_BYTES];
                token
                    .iter_mut()
                    .for_each(|b| *b = (rng.next() & 0xff) as u8);
                ClientFrame::Hello {
                    version: (rng.next() & 0xffff) as u16,
                    token,
                    want_snapshot: rng.next() & 1 == 1,
                }
            }
            1 => {
                let max = if rng.below(10) == 0 {
                    MAX_INPUT_CHUNK
                } else {
                    64
                };
                ClientFrame::Input(rng.bytes(max))
            }
            2 => ClientFrame::Resize {
                cols: rng.dimension(),
                rows: rng.dimension(),
            },
            3 => ClientFrame::Signal(match rng.below(3) {
                0 => SignalKind::Interrupt,
                1 => SignalKind::Terminate,
                _ => SignalKind::Kill,
            }),
            4 => ClientFrame::Shutdown {
                grace_ms: (rng.next() & 0xffff_ffff) as u32,
            },
            _ => ClientFrame::Ping(rng.next()),
        }
    }

    fn random_host(rng: &mut Rng) -> HostFrame {
        match rng.below(6) {
            0 => HostFrame::Welcome(Welcome {
                attempt_id: rng.text(64),
                child_pid: (rng.next() & 0xffff_ffff) as u32,
                seq: rng.next(),
                cols: rng.dimension(),
                rows: rng.dimension(),
            }),
            1 => HostFrame::Snapshot(Snapshot {
                seq: rng.next(),
                bytes: rng.bytes(4096),
            }),
            2 => {
                let max = if rng.below(10) == 0 {
                    MAX_OUTPUT_CHUNK
                } else {
                    256
                };
                HostFrame::Output {
                    seq: rng.next(),
                    bytes: rng.bytes(max),
                }
            }
            3 => {
                let reason = match rng.below(4) {
                    0 => ExitReason::Exited,
                    1 => ExitReason::Signalled,
                    2 => ExitReason::SpawnFailed,
                    _ => ExitReason::HostError,
                };
                let detail = rng.text(40);
                HostFrame::Exit(ExitRecord {
                    code: (rng.next() & 1 == 1).then_some((rng.next() & 0xff) as i32 - 128),
                    signal: (rng.next() & 1 == 1).then_some((rng.next() & 0x3f) as i32),
                    ended_at_ms: (rng.next() >> 1) as i64,
                    reason,
                    detail: (!detail.is_empty()).then_some(detail),
                })
            }
            4 => HostFrame::Pong(rng.next()),
            _ => HostFrame::Error {
                code: (rng.next() & 0xffff) as u16,
                message: rng.text(80),
            },
        }
    }

    #[test]
    fn client_frames_round_trip_with_arbitrary_chunking() -> TestResult {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..200 {
            let frames: Vec<ClientFrame> = (0..1 + rng.below(8))
                .map(|_| random_client(&mut rng))
                .collect();
            let stream: Vec<u8> = frames.iter().flat_map(encode_client).collect();
            let mut decoder = FrameDecoder::new();
            let mut decoded = Vec::new();
            let mut offset = 0;
            while offset < stream.len() {
                let step = 1 + rng.below(97);
                let end = (offset + step).min(stream.len());
                decoder.push(&stream[offset..end]);
                offset = end;
                while let Some(frame) = decoder.next_client()? {
                    decoded.push(frame);
                }
            }
            decoder.finish()?;
            assert_eq!(decoded, frames);
        }
        Ok(())
    }

    #[test]
    fn host_frames_round_trip_with_arbitrary_chunking() -> TestResult {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for _ in 0..200 {
            let frames: Vec<HostFrame> = (0..1 + rng.below(8))
                .map(|_| random_host(&mut rng))
                .collect();
            for frame in &frames {
                frame.validate()?;
            }
            let stream: Vec<u8> = frames.iter().flat_map(encode_host).collect();
            let mut decoder = FrameDecoder::new();
            let mut decoded = Vec::new();
            for chunk in stream.chunks(1 + rng.below(300)) {
                decoder.push(chunk);
                while let Some(frame) = decoder.next_host()? {
                    decoded.push(frame);
                }
            }
            decoder.finish()?;
            assert_eq!(decoded, frames);
        }
        Ok(())
    }

    #[test]
    fn hello_layout_is_stable() {
        let frame = ClientFrame::Hello {
            version: PROTOCOL_VERSION,
            token: [0xab; TOKEN_BYTES],
            want_snapshot: true,
        };
        let bytes = encode_client(&frame);
        assert_eq!(&bytes[..7], &[0, 0, 0, 36, T_HELLO, 0, 1]);
        assert_eq!(bytes.len(), 40);
        assert_eq!(bytes[39], 1);
    }

    #[test]
    fn oversize_length_is_rejected_before_payload_arrives() {
        let mut decoder = FrameDecoder::new();
        let len = u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or(u32::MAX);
        decoder.push(&len.to_be_bytes());
        assert_eq!(
            decoder.next_client(),
            Err(ProtocolError::FrameTooLarge { len })
        );
        // The error is sticky: the stream is desynchronized.
        decoder.push(&encode_client(&ClientFrame::Ping(1)));
        assert!(decoder.next_client().is_err());
    }

    #[test]
    fn maximum_frame_length_is_accepted() -> TestResult {
        let frame = HostFrame::Snapshot(Snapshot {
            seq: 9,
            bytes: vec![b'x'; MAX_SNAPSHOT_BYTES],
        });
        frame.validate()?;
        let bytes = encode_host(&frame);
        assert_eq!(bytes.len(), MAX_FRAME_BYTES + 4);
        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_host()?, Some(frame));
        Ok(())
    }

    #[test]
    fn empty_and_unknown_frames_are_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0, 0, 0, 0]);
        assert_eq!(decoder.next_host(), Err(ProtocolError::EmptyFrame));

        let mut decoder = FrameDecoder::new();
        decoder.push(&[0, 0, 0, 1, 0x7f]);
        assert_eq!(decoder.next_client(), Err(ProtocolError::UnknownType(0x7f)));

        // A host frame type is unknown in the client direction and vice versa.
        let mut decoder = FrameDecoder::new();
        decoder.push(&encode_host(&HostFrame::Pong(3)));
        assert_eq!(
            decoder.next_client(),
            Err(ProtocolError::UnknownType(T_PONG))
        );
        let mut decoder = FrameDecoder::new();
        decoder.push(&encode_client(&ClientFrame::Ping(3)));
        assert_eq!(decoder.next_host(), Err(ProtocolError::UnknownType(T_PING)));
    }

    #[test]
    fn truncated_and_trailing_payloads_are_rejected() {
        let cases: [(&[u8], ProtocolError); 4] = [
            (
                &[0, 0, 0, 3, T_HELLO, 0, 1],
                ProtocolError::Truncated { frame: "Hello" },
            ),
            (
                &[0, 0, 0, 4, T_RESIZE, 0, 80, 0],
                ProtocolError::Truncated { frame: "Resize" },
            ),
            (
                &[0, 0, 0, 3, T_SIGNAL, 1, 9],
                ProtocolError::TrailingBytes { frame: "Signal" },
            ),
            (
                &[0, 0, 0, 10, T_PING, 0, 0, 0, 0, 0, 0, 0, 1, 2],
                ProtocolError::TrailingBytes { frame: "Ping" },
            ),
        ];
        for (bytes, expected) in cases {
            let mut decoder = FrameDecoder::new();
            decoder.push(bytes);
            assert_eq!(decoder.next_client(), Err(expected));
        }
    }

    #[test]
    fn out_of_domain_values_are_rejected() {
        let mut bad_resize = FrameDecoder::new();
        bad_resize.push(&[0, 0, 0, 5, T_RESIZE, 0, 0, 0, 24]);
        assert!(matches!(
            bad_resize.next_client(),
            Err(ProtocolError::InvalidValue { .. })
        ));
        let mut too_wide = FrameDecoder::new();
        too_wide.push(&[0, 0, 0, 5, T_RESIZE, 0x03, 0xe9, 0, 24]);
        assert!(too_wide.next_client().is_err());
        let mut bad_signal = FrameDecoder::new();
        bad_signal.push(&[0, 0, 0, 2, T_SIGNAL, 4]);
        assert!(bad_signal.next_client().is_err());
        let mut bad_bool = FrameDecoder::new();
        let mut hello = encode_client(&ClientFrame::Hello {
            version: 1,
            token: [0; TOKEN_BYTES],
            want_snapshot: false,
        });
        if let Some(last) = hello.last_mut() {
            *last = 2;
        }
        bad_bool.push(&hello);
        assert!(bad_bool.next_client().is_err());
        let mut big_input = FrameDecoder::new();
        big_input.push(&encode_client(&ClientFrame::Input(vec![
            0;
            MAX_INPUT_CHUNK + 1
        ])));
        assert!(big_input.next_client().is_err());
        let mut big_output = FrameDecoder::new();
        big_output.push(&encode_host(&HostFrame::Output {
            seq: 1,
            bytes: vec![0; MAX_OUTPUT_CHUNK + 1],
        }));
        assert!(big_output.next_host().is_err());
        let mut bad_utf8 = FrameDecoder::new();
        bad_utf8.push(&[0, 0, 0, 5, T_ERROR, 0, 1, 0xff, 0xfe]);
        assert!(bad_utf8.next_host().is_err());
        let mut bad_reason = FrameDecoder::new();
        let mut exit = encode_host(&HostFrame::Exit(ExitRecord {
            code: Some(0),
            signal: None,
            ended_at_ms: 1,
            reason: ExitReason::Exited,
            detail: None,
        }));
        if let Some(last) = exit.last_mut() {
            *last = 9;
        }
        bad_reason.push(&exit);
        assert!(bad_reason.next_host().is_err());
    }

    #[test]
    fn random_corruption_never_panics_and_stays_bounded() {
        let mut rng = Rng(0xdead_beef_cafe_f00d);
        for _ in 0..2000 {
            let mut bytes = encode_host(&random_host(&mut rng));
            bytes.extend(encode_client(&random_client(&mut rng)));
            for _ in 0..1 + rng.below(4) {
                let index = rng.below(bytes.len());
                if let Some(byte) = bytes.get_mut(index) {
                    *byte = (rng.next() & 0xff) as u8;
                }
            }
            let cut = rng.below(bytes.len() + 1);
            let mut host = FrameDecoder::new();
            host.push(&bytes[..cut]);
            while let Ok(Some(_)) = host.next_host() {}
            assert!(host.buffered() <= bytes.len());
            let mut client = FrameDecoder::new();
            client.push(&bytes[..cut]);
            while let Ok(Some(_)) = client.next_client() {}
        }
    }

    #[test]
    fn eof_inside_a_frame_is_reported() {
        let mut decoder = FrameDecoder::new();
        let bytes = encode_client(&ClientFrame::Ping(5));
        decoder.push(&bytes[..6]);
        assert_eq!(decoder.next_client(), Ok(None));
        assert_eq!(decoder.finish(), Err(ProtocolError::UnexpectedEof));
    }

    #[test]
    fn validate_matches_decoder_limits() {
        assert!(
            ClientFrame::Input(vec![0; MAX_INPUT_CHUNK])
                .validate()
                .is_ok()
        );
        assert!(
            ClientFrame::Input(vec![0; MAX_INPUT_CHUNK + 1])
                .validate()
                .is_err()
        );
        assert!(ClientFrame::Resize { cols: 0, rows: 5 }.validate().is_err());
        assert!(
            ClientFrame::Resize {
                cols: 1000,
                rows: 1
            }
            .validate()
            .is_ok()
        );
        let frames: Vec<ClientFrame> = input_frames(&vec![7; MAX_INPUT_CHUNK * 2 + 1]).collect();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.validate().is_ok()));
    }

    #[test]
    fn hello_debug_redacts_token() {
        let text = format!(
            "{:?}",
            ClientFrame::Hello {
                version: 1,
                token: [0x5a; TOKEN_BYTES],
                want_snapshot: true
            }
        );
        assert!(text.contains("redacted"));
        assert!(!text.contains("90"));
    }

    fn sample_launch() -> LaunchSpec {
        LaunchSpec {
            version: RECORD_VERSION,
            attempt_id: "att-1".into(),
            task_id: "task_1".into(),
            argv: vec!["/usr/bin/agent".into(), "--flag".into()],
            cwd: "/srv/work".into(),
            env: BTreeMap::from([("TERM".into(), "xterm-256color".into())]),
            secret_env_names: vec!["PROVIDER_TOKEN".into()],
            cols: 120,
            rows: 40,
            containment: Containment::None,
            ring_bytes: 0,
            token_sha256: "a".repeat(64),
            linger_ms: 500,
        }
    }

    #[test]
    fn launch_spec_json_round_trip_and_defaults() -> TestResult {
        let mut spec = sample_launch();
        spec.containment = Containment::Bubblewrap(BubblewrapSpec {
            sandbox_root: "/data/sandboxes/s1".into(),
            home: "/data/sandboxes/s1/home".into(),
            writable: vec!["/data/sandboxes/s1/worktrees/t1".into()],
            readonly: vec!["/opt/tools".into()],
            share_net: true,
            binds: vec![BindMount {
                source: "/data/users/u1/home".into(),
                dest: "/home/alice".into(),
                writable: true,
            }],
            identity: Some(SandboxIdentity {
                uid: 0,
                gid: 0,
                hostname: "s1".into(),
            }),
            join: Some(Box::new(SandboxJoin {
                holder_pid: 4242,
                user_ns: 4_026_531_837,
                net_ns: 4_026_531_840,
                mnt_ns: 4_026_531_841,
                cgroup: Some("/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1".into()),
                system_root: "/data/runtime/sandboxes/s1/root".into(),
                hostname: "s1".into(),
                exec_shim: Some("/usr/local/bin/yulius".into()),
                private_dirs: vec!["/data".into()],
                host_system: None,
            })),
        });
        spec.validate()?;
        let json = serde_json::to_string(&spec)?;
        assert!(json.contains("\"kind\":\"bubblewrap\""));
        let back: LaunchSpec = serde_json::from_str(&json)?;
        assert_eq!(back, spec);

        let minimal = r#"{"version":1,"attempt_id":"a","task_id":"t","argv":["x"],
            "cwd":"/","cols":80,"rows":24,"token_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "future_field":true}"#;
        let parsed: LaunchSpec = serde_json::from_str(minimal)?;
        assert_eq!(parsed.containment, Containment::None);
        assert_eq!(parsed.linger_ms, DEFAULT_LINGER_MS);
        assert_eq!(parsed.client_queue_bytes(), DEFAULT_CLIENT_QUEUE_BYTES);
        parsed.validate()?;
        Ok(())
    }

    #[test]
    fn launch_spec_validation_rejects_each_bad_field() {
        type Mutation = fn(&mut LaunchSpec);
        let cases: Vec<(Mutation, &str)> = vec![
            (|s| s.version = 2, "version"),
            (|s| s.attempt_id = "../x".into(), "attempt_id"),
            (|s| s.task_id = String::new(), "task_id"),
            (|s| s.argv.clear(), "argv"),
            (|s| s.argv = vec![String::new()], "argv"),
            (|s| s.argv.push("a\0b".into()), "argv"),
            (|s| s.cwd = "relative".into(), "cwd"),
            (
                |s| {
                    s.env.insert("A=B".into(), "v".into());
                },
                "env",
            ),
            (
                |s| {
                    s.env.insert("PROVIDER_TOKEN".into(), "v".into());
                },
                "secret_env_names",
            ),
            (
                |s| s.secret_env_names.push("PROVIDER_TOKEN".into()),
                "secret_env_names",
            ),
            (|s| s.cols = 0, "cols"),
            (|s| s.rows = 1001, "rows"),
            (|s| s.ring_bytes = 1024, "ring_bytes"),
            (|s| s.token_sha256 = "A".repeat(64), "token_sha256"),
            (|s| s.linger_ms = MAX_LINGER_MS + 1, "linger_ms"),
            (
                |s| {
                    s.containment = Containment::Bubblewrap(BubblewrapSpec {
                        sandbox_root: "/".into(),
                        home: "/h".into(),
                        writable: vec![],
                        readonly: vec![],
                        share_net: false,
                        binds: vec![],
                        identity: None,
                        join: None,
                    });
                },
                "containment.sandbox_root",
            ),
            (
                |s| {
                    s.containment = Containment::Bubblewrap(BubblewrapSpec {
                        sandbox_root: "/data/s1".into(),
                        home: "/data/s1/home".into(),
                        writable: vec![],
                        readonly: vec![],
                        share_net: true,
                        binds: vec![],
                        identity: None,
                        join: Some(Box::new(SandboxJoin {
                            holder_pid: 1,
                            user_ns: 1,
                            net_ns: 2,
                            mnt_ns: 3,
                            cgroup: None,
                            system_root: "/data/runtime/sandboxes/s1/root".into(),
                            hostname: "s1".into(),
                            exec_shim: None,
                            private_dirs: Vec::new(),
                            host_system: None,
                        })),
                    });
                },
                "containment.join",
            ),
        ];
        for (mutate, field) in cases {
            let mut spec = sample_launch();
            mutate(&mut spec);
            assert_eq!(spec.validate().map_err(|e| e.field), Err(field));
        }
        assert!(sample_launch().validate().is_ok());
    }

    #[test]
    fn records_serialize_with_documented_field_names() -> TestResult {
        let exit = ExitFile {
            version: RECORD_VERSION,
            attempt_id: "att-1".into(),
            exit: ExitRecord {
                code: None,
                signal: Some(9),
                ended_at_ms: 42,
                reason: ExitReason::Signalled,
                detail: None,
            },
            usage: None,
        };
        let json = serde_json::to_value(&exit)?;
        assert_eq!(json["exit"]["reason"], "signalled");
        assert_eq!(json["exit"]["signal"], 9);
        assert!(json["exit"].get("detail").is_none());
        assert!(json.get("usage").is_none(), "unknown use is absent");
        assert_eq!(serde_json::from_value::<ExitFile>(json)?, exit);

        // SPEC-154 USE-001: the use a host measured, and an older host's file without it.
        let measured = ExitFile {
            usage: Some(ProcessUsage {
                cpu_ms: 1_250,
                max_rss_bytes: 64 << 20,
                written_bytes: None,
            }),
            ..exit.clone()
        };
        let json = serde_json::to_value(&measured)?;
        assert_eq!(json["usage"]["cpu_ms"], 1_250);
        assert_eq!(json["usage"]["max_rss_bytes"], 64 << 20);
        assert!(json["usage"].get("written_bytes").is_none());
        assert_eq!(serde_json::from_value::<ExitFile>(json)?, measured);
        let older = serde_json::json!({
            "version": RECORD_VERSION,
            "attempt_id": "att-1",
            "exit": { "code": 0, "signal": null, "ended_at_ms": 1, "reason": "exited" },
        });
        assert_eq!(serde_json::from_value::<ExitFile>(older)?.usage, None);

        let status = StatusRecord {
            version: RECORD_VERSION,
            attempt_id: "att-1".into(),
            ptyhost_pid: 10,
            child_pid: 11,
            started_at_ms: 12,
            boot_id: "boot".into(),
            token_sha256: "b".repeat(64),
        };
        let json = serde_json::to_string(&status)?;
        assert_eq!(serde_json::from_str::<StatusRecord>(&json)?, status);
        Ok(())
    }
}
