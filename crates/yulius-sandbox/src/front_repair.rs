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
    /// tick, and the ping's answer (`None`: not asked, as with no engine running).
    pub fn observe(&mut self, jammed: bool, ping: Option<bool>, now: Instant) {
        self.jammed_since = if jammed {
            Some(self.jammed_since.unwrap_or(now))
        } else {
            None
        };
        match ping {
            Some(false) => self.failed_pings = self.failed_pings.saturating_add(1),
            Some(true) => self.failed_pings = 0,
            None => {}
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
/// [`PING_TIMEOUT`].
pub async fn ping(socket: &Path) -> bool {
    let asked = async {
        let mut stream = tokio::net::UnixStream::connect(socket).await?;
        stream
            .write_all(b"GET /_ping HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n")
            .await?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await?;
        std::io::Result::Ok(head.starts_with(b"HTTP/1.") && head.ends_with(b" 200"))
    };
    matches!(
        tokio::time::timeout(PING_TIMEOUT, asked).await,
        Ok(Ok(true))
    )
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
        watch.observe(false, None, start + tick * 6);
        assert_eq!(
            watch.decide(0, start + tick * 6),
            Some(Why::Unanswered),
            "no engine: kept"
        );
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
        assert!(ping(&ok).await);
        assert!(!ping(&dir.path().join("none.sock")).await);
        server.abort();
        Ok(())
    }
}
