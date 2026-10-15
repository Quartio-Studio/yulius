//! Process-host port: detached PTY hosts that own one agent run each (SPEC-002 ADR-02, §7.1).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::AppResult;

/// A file the run needs (configuration, hooks, credentials) written before spawn.
#[derive(Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Contents.
    pub contents: Vec<u8>,
    /// Unix mode (e.g. `0o600` for secrets).
    pub mode: u32,
}

impl std::fmt::Debug for PlannedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlannedFile")
            .field("path", &self.path)
            .field("bytes", &self.contents.len())
            .finish()
    }
}

/// OS containment of a run (SPEC-002 ADR-03).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Containment {
    /// Host process (development; not a security boundary).
    None,
    /// Linux bubblewrap namespace.
    Bubblewrap {
        /// Sandbox directory (writable).
        sandbox_root: PathBuf,
        /// HOME inside the namespace.
        home: PathBuf,
        /// Extra writable paths.
        writable: Vec<PathBuf>,
        /// Extra read-only paths (e.g. the yulius executable).
        readonly: Vec<PathBuf>,
        /// Mounts at another path inside the sandbox (user home, identity files).
        binds: Vec<Bind>,
        /// Identity in a new user namespace (user shells and editors); `None` for agents.
        identity: Option<Identity>,
        /// The sandbox's namespace holder to join (SPEC-002 ADR-22); `None`: the previous
        /// containment.
        join: Option<Box<crate::ports::isolation::SandboxJoin>>,
    },
}

/// A bind mount at a different path inside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    /// Host path.
    pub source: PathBuf,
    /// Path inside the sandbox.
    pub dest: PathBuf,
    /// Writable (otherwise read-only).
    pub writable: bool,
}

/// Process identity inside its own user namespace (SPEC-002 ADR-12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// User id inside the namespace.
    pub uid: u32,
    /// Group id inside the namespace.
    pub gid: u32,
    /// Host name inside the namespace.
    pub hostname: String,
}

/// Everything needed to start one process run.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// Executable and arguments (no shell).
    pub argv: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Non-secret environment.
    pub env: Vec<(String, String)>,
    /// Secret environment (passed only through process environments).
    pub secret_env: Vec<(String, String)>,
    /// Files to write before spawn.
    pub files: Vec<PlannedFile>,
    /// Containment.
    pub containment: Containment,
}

impl std::fmt::Debug for LaunchPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchPlan")
            .field("argv", &self.argv)
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field(
                "secret_env_names",
                &self.secret_env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("files", &self.files)
            .field("containment", &self.containment)
            .finish()
    }
}

/// Identity of a spawned run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedRun {
    /// Host process id.
    pub host_pid: u32,
    /// Agent process id.
    pub child_pid: u32,
}

/// Signals that can be sent to a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSignal {
    /// Ctrl-C to the TUI.
    Interrupt,
    /// SIGTERM to the process group.
    Terminate,
    /// SIGKILL to the process group.
    Kill,
}

/// What the host reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    /// Terminal output.
    Output {
        /// Sequence number.
        seq: u64,
        /// Bytes.
        bytes: Vec<u8>,
    },
    /// Full-screen snapshot.
    Snapshot {
        /// Sequence number of the last included output.
        seq: u64,
        /// Formatted screen bytes.
        bytes: Vec<u8>,
    },
    /// The agent exited.
    Exit(RunExit),
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunExit {
    /// Exit code.
    pub code: Option<i32>,
    /// Terminating signal.
    pub signal: Option<i32>,
    /// The process could not be spawned.
    pub spawn_failed: bool,
}

impl RunExit {
    /// Clean exit (status 0, no signal).
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.code == Some(0) && self.signal.is_none() && !self.spawn_failed
    }
}

/// Facts returned when attaching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWelcome {
    /// Agent pid.
    pub child_pid: u32,
    /// Current sequence.
    pub seq: u64,
    /// PTY columns.
    pub cols: u16,
    /// PTY rows.
    pub rows: u16,
    /// Current screen.
    pub snapshot: Option<Vec<u8>>,
}

/// State of a recorded run directory (restart reconciliation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunInspection {
    /// Nothing recorded.
    Missing,
    /// The host recorded an exit.
    Exited(RunExit),
    /// A host was recorded.
    Recorded {
        /// Host alive.
        host_alive: bool,
        /// Agent alive.
        child_alive: bool,
        /// Same machine boot.
        same_boot: bool,
    },
}

/// Receiving half of an attached run.
#[async_trait]
pub trait RunReader: Send {
    /// Next event; `Ok(None)` when the connection closed.
    async fn next(&mut self) -> AppResult<Option<RunEvent>>;
}

/// Sending half of an attached run.
#[async_trait]
pub trait RunWriter: Send {
    /// Keyboard input.
    async fn input(&mut self, bytes: &[u8]) -> AppResult<()>;
    /// PTY size.
    async fn resize(&mut self, cols: u16, rows: u16) -> AppResult<()>;
    /// Signal.
    async fn signal(&mut self, signal: RunSignal) -> AppResult<()>;
    /// Graceful shutdown with escalation after `grace_ms`.
    async fn shutdown(&mut self, grace_ms: u32) -> AppResult<()>;
}

/// An attached run.
pub struct AttachedRun {
    /// Facts at attach time.
    pub welcome: RunWelcome,
    /// Events.
    pub reader: Box<dyn RunReader>,
    /// Commands.
    pub writer: Box<dyn RunWriter>,
}

/// The process-host adapter.
#[async_trait]
pub trait SessionHost: Send + Sync {
    /// Writes planned files, spawns a detached host for the run and waits until it is serving.
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun>;
    /// Connects to a run's host.
    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun>;
    /// Inspects a run directory without side effects.
    fn inspect(&self, run_dir: &Path) -> RunInspection;
    /// Kills a recorded host and its agent (stale reconciliation).
    fn terminate(&self, run_dir: &Path) -> AppResult<()>;
    /// Background shells the agent `child_pid` left running (TSK-014: Claude Code's Bash tool
    /// shells still alive after the turn). Blocking and bounded; `None` when the process table
    /// cannot be read.
    fn background_shells(&self, _child_pid: u32) -> Option<u32> {
        None
    }
    /// [`Self::background_shells`], and apart the shells that only run `yulius wait` (SPEC-085
    /// STT-002): the agent waits for Yulius and goes on by itself. A host that cannot tell them
    /// apart reports none.
    fn background_probe(&self, child_pid: u32) -> Option<BackgroundProbe> {
        self.background_shells(child_pid)
            .map(|shells| BackgroundProbe { shells, waits: 0 })
    }
    /// Freezes the run's agent and everything it started, never its host, so that nothing in it
    /// can tell (SPEC-160 OPS-010). Blocking and bounded; idempotent. A host that cannot freeze
    /// this run says [`RunFreeze::Unsupported`] and the run is paused at its turn boundary.
    fn freeze(&self, _run_dir: &Path) -> RunFreeze {
        RunFreeze::Unsupported
    }
    /// Whether [`Self::freeze`] could freeze the run, without freezing anything (OPS-011).
    fn can_freeze(&self, _run_dir: &Path) -> bool {
        false
    }
    /// Thaws a run [`Self::freeze`] froze; nothing for a run that is not frozen (OPS-013).
    fn thaw(&self, _run_dir: &Path) {}
    /// Thaws every run this host left frozen, also those of a server that stopped before it
    /// thawed them; returns how many (OPS-013).
    fn thaw_all(&self) -> usize {
        0
    }
}

/// What freezing a run did (SPEC-160 OPS-010/011).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFreeze {
    /// The kernel reports the run frozen.
    Frozen,
    /// Asked; the kernel has not confirmed it yet.
    Freezing,
    /// This run cannot be frozen.
    Unsupported,
}

/// What one probe of an agent's process tree found (SPEC-085 STT-002).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackgroundProbe {
    /// Background shells that run something (TSK-014).
    pub shells: u32,
    /// Background shells whose only process is `yulius wait` (SPEC-050 DSK-102).
    pub waits: u32,
}
