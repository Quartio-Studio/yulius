//! When Yulius restarts a sandbox's Docker front and engine by itself (SPEC-494 DFJ-008,
//! ADR-0344), as was done by hand on 2026-11-15: the front stops (its engine with it; running
//! containers stop, volumes stay in the engine's storage) and the upkeep starts a new one.
//!
//! A repair is due when the front has stayed full or refusing for [`REPAIR_AFTER`], or when its
//! engine runs and [`PINGS_FAILED`] pings in a row through the front got no answer. It runs at
//! the moment that loses the least: when no CI job of the sandbox runs (a job's containers would
//! stop under it), or once it has been due for [`REPAIR_WAIT`], since a jammed front fails the
//! jobs anyway.

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Time a front stays full or refusing before a repair is due.
pub const REPAIR_AFTER: Duration = Duration::from_secs(120);
/// Longest wait for no CI job of the sandbox once a repair is due.
pub const REPAIR_WAIT: Duration = Duration::from_secs(600);
/// Failed pings in a row (one per upkeep tick) that make a repair of a running engine due.
pub const PINGS_FAILED: u32 = 3;
/// Bound on one ping through the front.
pub const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a front is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// It stayed full or refusing.
    Full,
    /// Its engine runs and does not answer through it.
    Unanswered,
    /// A person asked (the Superagent's `docker_restart`).
    Asked,
}

impl Why {
    /// As the journal and the tool say it.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Self::Full => "it stayed full",
            Self::Unanswered => "its engine runs but Docker calls through it got no answer",
            Self::Asked => "a person asked",
        }
    }
}

/// One tick's reading of a front's state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    /// Full, or refused since the last tick.
    pub jammed: bool,
    /// Its engine runs.
    pub engine_running: bool,
    /// Its engine is starting or stopping (it answers nothing then).
    pub engine_busy: bool,
    /// Connections it holds.
    pub held: usize,
}

/// Whether the upkeep pings the engine through the front this tick: only while a client holds
/// a connection, so a ping never keeps an engine from idling out nor starts a stopped one
/// (ADR-0151's idle stop; review round 1 of slice 3), while the engine runs and is not starting
/// or stopping, and while the front is not jammed (a jam is repaired on its own).
#[must_use]
pub fn should_ping(reading: &Reading) -> bool {
    reading.engine_running && !reading.engine_busy && reading.held > 0 && !reading.jammed
}

/// What the upkeep knows of one front toward a repair.
#[derive(Debug, Default)]
pub struct RepairWatch {
    jammed_since: Option<Instant>,
    failed_pings: u32,
    due_since: Option<Instant>,
    why: Option<Why>,
}

impl RepairWatch {
    /// Takes one tick's reading at `now`: whether the front is full or refused since the last
    /// tick, and the ping's answer (`None`: not asked, which starts the count of unanswered
    /// pings again: they count only in a row).
    pub fn observe(&mut self, jammed: bool, ping: Option<bool>, now: Instant) {
        self.jammed_since = if jammed {
            Some(self.jammed_since.unwrap_or(now))
        } else {
            None
        };
        match ping {
            Some(false) => self.failed_pings = self.failed_pings.saturating_add(1),
            Some(true) | None => self.failed_pings = 0,
        }
        let why = if self
            .jammed_since
            .is_some_and(|since| now.duration_since(since) >= REPAIR_AFTER)
        {
            Some(Why::Full)
        } else if self.failed_pings >= PINGS_FAILED {
            Some(Why::Unanswered)
        } else {
            None
        };
        match why {
            Some(why) => {
                self.due_since.get_or_insert(now);
                self.why = Some(why);
            }
            None => {
                self.due_since = None;
                self.why = None;
            }
        }
    }

    /// Why to restart the front now: a repair is due, and no CI job of the sandbox runs, or it
    /// has been due for [`REPAIR_WAIT`].
    #[must_use]
    pub fn decide(&self, ci_jobs: u32, now: Instant) -> Option<Why> {
        let why = self.why?;
        let waited = self
            .due_since
            .is_some_and(|since| now.duration_since(since) >= REPAIR_WAIT);
        (ci_jobs == 0 || waited).then_some(why)
    }
}

/// Whether the engine answers `GET /_ping` through the front's `socket` within
/// [`PING_TIMEOUT`]; `None` when the server may not connect to the socket at all (the answer
/// would say nothing of the engine, and must never make a repair due).
pub async fn ping(socket: &Path) -> Option<bool> {
    ping_within(socket, PING_TIMEOUT).await
}

/// [`ping`] bounded by `limit` (tests use a short one).
async fn ping_within(socket: &Path, limit: Duration) -> Option<bool> {
    let stream = match tokio::time::timeout(limit, tokio::net::UnixStream::connect(socket)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Ok(Err(_)) | Err(_) => return Some(false),
    };
    let asked = async {
        let mut stream = stream;
        stream
            .write_all(b"GET /_ping HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n")
            .await?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await?;
        std::io::Result::Ok(head.starts_with(b"HTTP/1.") && head.ends_with(b" 200"))
    };
    Some(matches!(
        tokio::time::timeout(limit, asked).await,
        Ok(Ok(true))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    /// SPEC-494 AC-6: a front full for two minutes is restarted once no CI job runs; with a job
    /// running it waits, at most ten minutes after it was due.
    #[test]
    fn a_full_front_is_restarted_when_no_ci_job_runs_or_after_the_wait() {
        let start = Instant::now();
        let mut watch = RepairWatch::default();
        watch.observe(true, None, start);
        watch.observe(true, None, start + MIN);
        assert_eq!(
            watch.decide(0, start + MIN),
            None,
            "not full for long enough"
        );
        watch.observe(true, None, start + 2 * MIN);
        assert_eq!(watch.decide(1, start + 2 * MIN), None, "a CI job runs");
        assert_eq!(watch.decide(0, start + 2 * MIN), Some(Why::Full));
        watch.observe(true, None, start + 11 * MIN);
        assert_eq!(watch.decide(1, start + 11 * MIN), None, "due 9 minutes");
        watch.observe(true, None, start + 12 * MIN);
        assert_eq!(
            watch.decide(1, start + 12 * MIN),
            Some(Why::Full),
            "waited long enough"
        );
    }

    /// A front that has room again is due nothing, and a later jam counts from its start.
    #[test]
    fn room_again_cancels_a_repair() {
        let start = Instant::now();
        let mut watch = RepairWatch::default();
        watch.observe(true, None, start);
        watch.observe(true, None, start + 2 * MIN);
        watch.observe(false, None, start + 3 * MIN);
        assert_eq!(watch.decide(0, start + 3 * MIN), None);
        watch.observe(true, None, start + 4 * MIN);
        assert_eq!(watch.decide(0, start + 4 * MIN), None);
    }

    /// SPEC-494 DFJ-008: an engine that runs but does not answer through the front for three
    /// ticks in a row is due a restart; an answer in between starts the count again.
    #[test]
    fn three_unanswered_pings_in_a_row_make_a_repair_due() {
        let start = Instant::now();
        let mut watch = RepairWatch::default();
        let tick = Duration::from_secs(30);
        for (i, answer) in [false, false, true, false, false].into_iter().enumerate() {
            watch.observe(
                false,
                Some(answer),
                start + tick * u32::try_from(i).unwrap_or(0),
            );
        }
        assert_eq!(watch.decide(0, start + tick * 5), None);
        watch.observe(false, Some(false), start + tick * 5);
        assert_eq!(watch.decide(0, start + tick * 5), Some(Why::Unanswered));
        // A tick that did not ask (no client, the engine starting or stopped) starts it again.
        watch.observe(false, None, start + tick * 6);
        assert_eq!(watch.decide(0, start + tick * 6), None);
        for i in 7..9 {
            watch.observe(false, Some(false), start + tick * i);
        }
        assert_eq!(watch.decide(0, start + tick * 8), None, "two in a row only");
    }

    /// The ping reads an engine's answer through a socket, and a socket that does not answer
    /// fails within its bound.
    #[tokio::test]
    async fn the_ping_reads_the_engines_answer() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let ok = dir.path().join("ok.sock");
        let listener = tokio::net::UnixListener::bind(&ok)?;
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n\r\nOK").await;
            }
        });
        assert_eq!(ping(&ok).await, Some(true));
        assert_eq!(ping(&dir.path().join("none.sock")).await, Some(false));
        server.abort();
        Ok(())
    }

    /// Slice 3 review round 1: a front that answers 503 (its own refusal) or nothing within the
    /// bound is no answer of the engine.
    #[tokio::test]
    async fn a_refusal_or_silence_is_no_answer() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let refusing = dir.path().join("refusing.sock");
        let silent = dir.path().join("silent.sock");
        let (a, b) = (
            tokio::net::UnixListener::bind(&refusing)?,
            tokio::net::UnixListener::bind(&silent)?,
        );
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                tokio::select! {
                    Ok((mut stream, _)) = a.accept() => {
                        let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
                    }
                    Ok((stream, _)) = b.accept() => held.push(stream),
                }
            }
        });
        let limit = Duration::from_millis(300);
        assert_eq!(ping_within(&refusing, limit).await, Some(false));
        let started = Instant::now();
        assert_eq!(ping_within(&silent, limit).await, Some(false));
        assert!(started.elapsed() < Duration::from_secs(5), "bounded");
        server.abort();
        Ok(())
    }

    /// Slice 3 review round 1: the upkeep pings only while a client holds a connection, the
    /// engine runs and is not starting or stopping, and the front is not jammed.
    #[test]
    fn a_ping_is_asked_only_beside_a_client_of_a_settled_engine() {
        let busy = Reading {
            jammed: false,
            engine_running: true,
            engine_busy: false,
            held: 1,
        };
        assert!(should_ping(&busy));
        assert!(
            !should_ping(&Reading { held: 0, ..busy }),
            "an idle engine idles out"
        );
        assert!(
            !should_ping(&Reading {
                engine_running: false,
                ..busy
            }),
            "never starts one"
        );
        assert!(
            !should_ping(&Reading {
                engine_busy: true,
                ..busy
            }),
            "starting or stopping"
        );
        assert!(!should_ping(&Reading {
            jammed: true,
            ..busy
        }));
    }
}
