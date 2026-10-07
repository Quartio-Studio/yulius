//! Bounded subprocess runner for delivery environment commands (SPEC-001 DLV-003).
//!
//! Contract:
//! - the command is spawned from an argv (never through an implicit shell) with a cleared
//!   environment, stdin closed, in a new process group;
//! - stdout and stderr are captured into one [`OutputRing`] that keeps the last bytes only;
//! - at the deadline the whole process group receives `SIGKILL` *before* the leader is reaped
//!   (so the group id cannot have been reused), then the leader is reaped;
//! - dropping the future while the leader runs kills the group as well (cancellation);
//! - after the leader exits, group members that keep the output pipes open get
//!   [`DRAIN_GRACE`], after which the pipes are no longer read. They are not killed: a
//!   deployment may deliberately leave a daemon running, and the leader is gone, so the group
//!   id is no longer safe to signal.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

/// Time the output pipes are still read after the leader exited.
pub const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// Interval between output snapshots sent to an observer.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

const CHUNK: usize = 8 * 1024;

/// One bounded command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Executable and arguments.
    pub argv: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// The complete environment (nothing is inherited).
    pub env: Vec<(String, String)>,
    /// Wall-clock limit; the process group is killed when it elapses.
    pub timeout: Duration,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEnd {
    /// The leader exited with this status code.
    Exited(i32),
    /// The leader was terminated by this signal.
    Signaled(i32),
    /// The deadline elapsed; the process group was killed and the leader reaped.
    TimedOut,
    /// The command could not be started (or waiting for it failed).
    SpawnFailed(String),
}

impl CommandEnd {
    /// Exit status 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Exited(0))
    }

    /// The exit code, when the leader exited normally.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(*code),
            _ => None,
        }
    }
}

/// Keeps the last `limit` bytes written to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRing {
    bytes: VecDeque<u8>,
    limit: usize,
    dropped: u64,
}

impl OutputRing {
    /// An empty ring holding at most `limit` bytes (at least 1).
    #[must_use]
    pub fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            bytes: VecDeque::with_capacity(limit.min(64 * 1024)),
            limit,
            dropped: 0,
        }
    }

    /// Appends bytes, discarding the oldest ones beyond the limit.
    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.limit {
            self.dropped = self.dropped.saturating_add(
                u64::try_from(self.bytes.len() + data.len() - self.limit).unwrap_or(u64::MAX),
            );
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - self.limit..]);
            return;
        }
        let overflow = (self.bytes.len() + data.len()).saturating_sub(self.limit);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.dropped = self
                .dropped
                .saturating_add(u64::try_from(overflow).unwrap_or(u64::MAX));
        }
        self.bytes.extend(data);
    }

    /// Appends text.
    pub fn push_str(&mut self, text: &str) {
        self.push(text.as_bytes());
    }

    /// Bytes discarded so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Bytes currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Nothing held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The held bytes as text (invalid UTF-8 replaced), prefixed by a note when earlier
    /// output was discarded.
    #[must_use]
    pub fn text(&self) -> String {
        let (front, back) = self.bytes.as_slices();
        let mut raw = Vec::with_capacity(self.bytes.len());
        raw.extend_from_slice(front);
        raw.extend_from_slice(back);
        let body = String::from_utf8_lossy(&raw);
        if self.dropped == 0 {
            body.into_owned()
        } else {
            format!(
                "[yulius] {} earlier bytes of output were discarded\n{body}",
                self.dropped
            )
        }
    }
}

/// Kills the process group on drop while armed.
struct GroupGuard {
    pgid: Option<Pid>,
    armed: bool,
}

impl GroupGuard {
    fn kill(&self) {
        if self.armed
            && let Some(pgid) = self.pgid
        {
            // ESRCH (group already gone) is fine.
            let _ = killpg(pgid, Signal::SIGKILL);
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

enum Event {
    Stdout(Option<usize>),
    Stderr(Option<usize>),
    Exit(std::io::Result<ExitStatus>),
    Deadline,
    Drained,
    Tick,
}

async fn read_chunk<R: AsyncRead + Unpin>(pipe: &mut Option<R>, buf: &mut [u8]) -> Option<usize> {
    let reader = pipe.as_mut()?;
    match reader.read(buf).await {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn exit_end(status: ExitStatus) -> CommandEnd {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => CommandEnd::Exited(code),
        (None, Some(signal)) => CommandEnd::Signaled(signal),
        (None, None) => CommandEnd::Signaled(0),
    }
}

/// Runs `spec`, appending its output to `ring` and sending the ring's text to `snapshots`
/// about every [`SNAPSHOT_INTERVAL`] and once at the end.
pub async fn run_bounded(
    spec: &CommandSpec,
    ring: &mut OutputRing,
    snapshots: Option<&watch::Sender<String>>,
) -> CommandEnd {
    let Some((program, args)) = spec.argv.split_first() else {
        return CommandEnd::SpawnFailed("empty command".into());
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return CommandEnd::SpawnFailed(err.to_string()),
    };
    let mut group = GroupGuard {
        pgid: child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .map(Pid::from_raw),
        armed: true,
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out_buf = vec![0u8; CHUNK];
    let mut err_buf = vec![0u8; CHUNK];
    let deadline = Instant::now() + spec.timeout;
    let mut status: Option<ExitStatus> = None;
    let mut timed_out = false;
    let mut drain_until: Option<Instant> = None;
    let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if status.is_some() && stdout.is_none() && stderr.is_none() {
            break;
        }
        let event = tokio::select! {
            read = read_chunk(&mut stdout, &mut out_buf), if stdout.is_some() => Event::Stdout(read),
            read = read_chunk(&mut stderr, &mut err_buf), if stderr.is_some() => Event::Stderr(read),
            exit = child.wait(), if status.is_none() => Event::Exit(exit),
            () = tokio::time::sleep_until(deadline), if status.is_none() && !timed_out => Event::Deadline,
            () = sleep_until_opt(drain_until), if drain_until.is_some() => Event::Drained,
            _ = ticker.tick(), if snapshots.is_some() => Event::Tick,
        };
        match event {
            Event::Stdout(Some(n)) => ring.push(&out_buf[..n]),
            Event::Stdout(None) => stdout = None,
            Event::Stderr(Some(n)) => ring.push(&err_buf[..n]),
            Event::Stderr(None) => stderr = None,
            Event::Exit(Ok(exit)) => {
                // The leader is reaped: its id may be reused, so never signal the group again.
                group.armed = false;
                status = Some(exit);
                drain_until = Some(Instant::now() + DRAIN_GRACE);
            }
            Event::Exit(Err(err)) => {
                group.kill();
                group.armed = false;
                return CommandEnd::SpawnFailed(format!("waiting for the command failed: {err}"));
            }
            Event::Deadline => {
                timed_out = true;
                ring.push_str(&format!(
                    "\n[yulius] timed out after {} s; killing the process group\n",
                    spec.timeout.as_secs()
                ));
                group.kill();
            }
            Event::Drained => {
                ring.push_str(
                    "\n[yulius] the command exited but processes it started still hold its output; no longer reading it\n",
                );
                stdout = None;
                stderr = None;
            }
            Event::Tick => {
                if let Some(tx) = snapshots {
                    let _ = tx.send(ring.text());
                }
            }
        }
    }
    if let Some(tx) = snapshots {
        let _ = tx.send(ring.text());
    }
    match status {
        _ if timed_out => CommandEnd::TimedOut,
        Some(exit) => exit_end(exit),
        None => CommandEnd::SpawnFailed("the command ended without an exit status".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_tail_and_counts_discarded_bytes() {
        let mut ring = OutputRing::new(8);
        ring.push(b"abcd");
        ring.push(b"efgh");
        assert_eq!(ring.text(), "abcdefgh");
        ring.push(b"ij");
        assert_eq!(ring.dropped(), 2);
        assert!(ring.text().ends_with("cdefghij"));
        assert!(ring.text().starts_with("[yulius] 2 earlier bytes"));
        ring.push(b"0123456789");
        assert_eq!(ring.len(), 8);
        assert_eq!(ring.dropped(), 12);
        assert!(ring.text().ends_with("23456789"));
    }

    fn sh(script: &str, timeout: Duration) -> CommandSpec {
        CommandSpec {
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            timeout,
        }
    }

    #[tokio::test]
    async fn captures_output_and_exit_codes() {
        let mut ring = OutputRing::new(1024);
        let end = run_bounded(
            &sh("echo out; echo err >&2; exit 3", Duration::from_secs(20)),
            &mut ring,
            None,
        )
        .await;
        assert_eq!(end, CommandEnd::Exited(3));
        assert!(ring.text().contains("out") && ring.text().contains("err"));
    }

    #[tokio::test]
    async fn environment_is_cleared() {
        let mut ring = OutputRing::new(1024);
        let mut spec = sh("echo \"[$HOME][$PATH]\"", Duration::from_secs(20));
        spec.env.push(("HOME".into(), "/nowhere".into()));
        let end = run_bounded(&spec, &mut ring, None).await;
        assert!(end.succeeded());
        assert_eq!(ring.text().trim(), "[/nowhere][/usr/bin:/bin]");
    }

    #[tokio::test]
    async fn missing_program_is_a_spawn_failure() {
        let mut ring = OutputRing::new(16);
        let spec = CommandSpec {
            argv: vec!["/nonexistent/yulius-test-binary".into()],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        assert!(matches!(
            run_bounded(&spec, &mut ring, None).await,
            CommandEnd::SpawnFailed(_)
        ));
    }

    #[tokio::test]
    async fn timeout_kills_the_group() {
        let mut ring = OutputRing::new(1024);
        let end = run_bounded(
            &sh(
                "echo started; sleep 30; echo never",
                Duration::from_millis(300),
            ),
            &mut ring,
            None,
        )
        .await;
        assert_eq!(end, CommandEnd::TimedOut);
        let text = ring.text();
        assert!(text.contains("started") && !text.contains("never"));
        assert!(text.contains("timed out"));
    }
}
