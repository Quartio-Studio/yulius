//! Yulius's own outages (SPEC-358 MSB-006, MSB-007; ADR-0244). Yulius writes the time it was last
//! alive to `<data>/alive-at`. After a restart it waits for the reattached runs to replay their
//! transcripts, then types "Yulius is back; continue" once into each task that went idle while it
//! was down.
//!
//! "Went idle while it was down" means the transcript's record that closes the task's last turn
//! is later than `alive-at` and earlier than this start. That is the turn a dead `yulius wait`
//! woke, or a turn whose Stop hook failed open, and nothing settled or pushed it. This amends
//! SPEC-028 MSR-004 ("nothing is typed into an idle session") for this one case.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::context::App;

/// The file holding when Yulius was last alive (milliseconds since the epoch).
pub const ALIVE_FILE: &str = "alive-at";
/// How often `alive-at` is written.
pub const ALIVE_EVERY: Duration = Duration::from_secs(30);
/// How long after a start the tasks are woken: the reattached runs replay their transcripts
/// first.
pub const WAKE_GRACE: Duration = Duration::from_secs(90);
/// What a woken task is told.
pub const WAKE_TEXT: &str = "Yulius is back; continue";

/// What decides whether a reattached task is woken after an outage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeFacts {
    /// The task is Running with a live session.
    pub running: bool,
    /// A turn is open (the agent works).
    pub turn_open: bool,
    /// When the record closing its last turn was written (ms), 0 for none.
    pub turn_closed_ms: i64,
    /// Its goal is achieved: it is done.
    pub goal_achieved: bool,
    /// It waits for a person (`needs_you`).
    pub needs_you: bool,
    /// It is paused.
    pub paused: bool,
    /// A person typed into it in the last minute.
    pub person_typed: bool,
}

/// Whether the task is woken: idle, Running, with its last turn closed while Yulius was down
/// (after `down_since_ms`, before `started_ms`), and nobody else to wait for (MSB-007).
#[must_use]
pub fn wakes_after_outage(facts: &WakeFacts, down_since_ms: i64, started_ms: i64) -> bool {
    facts.running
        && !facts.turn_open
        && facts.turn_closed_ms > down_since_ms
        && facts.turn_closed_ms < started_ms
        && !facts.goal_achieved
        && !facts.needs_you
        && !facts.paused
        && !facts.person_typed
}

/// When Yulius was last alive, from `<data_dir>/alive-at`.
#[must_use]
pub fn read_alive_at(data_dir: &Path) -> Option<Timestamp> {
    let text = std::fs::read_to_string(data_dir.join(ALIVE_FILE)).ok()?;
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|ms| *ms > 0)
        .map(Timestamp)
}

/// Writes `at` to `<data_dir>/alive-at` (through a temporary file, so a crash leaves the old
/// time or the new one).
///
/// # Errors
/// File errors.
pub fn write_alive_at(data_dir: &Path, at: Timestamp) -> std::io::Result<()> {
    let tmp = data_dir.join(format!("{ALIVE_FILE}.tmp"));
    std::fs::write(&tmp, at.0.to_string())?;
    std::fs::rename(tmp, data_dir.join(ALIVE_FILE))
}

impl App {
    /// Reads when the previous server was last alive, keeps `alive-at` fresh from now on, and
    /// wakes the tasks that went idle during the outage [`WAKE_GRACE`] later. Returns when the
    /// previous server was last alive, if it was recorded.
    pub fn start_outage_watch(self: &Arc<Self>) -> Option<Timestamp> {
        let data_dir = self.config.data_dir.clone();
        let down_since = read_alive_at(&data_dir);
        let started = self.now();
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(ALIVE_EVERY);
            loop {
                tick.tick().await;
                if let Err(err) = write_alive_at(&data_dir, app.now()) {
                    tracing::warn!(error = %err, "could not record that Yulius is alive");
                }
            }
        });
        if let Some(down_since) = down_since {
            let app = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(WAKE_GRACE).await;
                app.wake_after_outage(down_since, started).await;
            });
        }
        down_since
    }

    /// Types [`WAKE_TEXT`] into each live task that went idle while Yulius was down (from
    /// `down_since` to `started`); returns the tasks it woke.
    pub async fn wake_after_outage(
        self: &Arc<Self>,
        down_since: Timestamp,
        started: Timestamp,
    ) -> Vec<TaskId> {
        let attempts = match self.deps.store.live_attempts().await {
            Ok(attempts) => attempts,
            Err(err) => {
                tracing::warn!(error = %err, "the tasks to wake after the outage could not be read");
                return Vec::new();
            }
        };
        let mut woken = Vec::new();
        for attempt in attempts {
            let Ok(Some(task)) = self.deps.store.task(&attempt.task_id).await else {
                continue;
            };
            let Some(runtime) = self.existing_runtime(&task.id) else {
                continue;
            };
            let facts = WakeFacts {
                running: task.status == TaskStatus::Running,
                turn_open: runtime.turn_in_progress(),
                turn_closed_ms: runtime.turn_record_at_ms.load(Ordering::SeqCst),
                goal_achieved: runtime.goal_achieved.load(Ordering::SeqCst),
                needs_you: !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None)),
                paused: !matches!(self.deps.store.open_pause(&task.id).await, Ok(None)),
                person_typed: self.typed_recently(&task.id).is_some(),
            };
            if !wakes_after_outage(&facts, down_since.0, started.0) {
                continue;
            }
            let paste = format!("\x1b[200~{WAKE_TEXT}\x1b[201~");
            match self.type_into_session(&task, paste.as_bytes()).await {
                Ok(()) => {
                    tracing::info!(task = %task.id, "woken after Yulius's outage");
                    woken.push(task.id.clone());
                }
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not wake the task after the outage");
                }
            }
        }
        woken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWN: i64 = 1_000;
    const STARTED: i64 = 5_000;

    fn idle() -> WakeFacts {
        WakeFacts {
            running: true,
            turn_closed_ms: 3_000,
            ..WakeFacts::default()
        }
    }

    #[test]
    fn a_task_idle_since_the_outage_is_woken() {
        assert!(wakes_after_outage(&idle(), DOWN, STARTED));
    }

    #[test]
    fn only_a_turn_closed_while_yulius_was_down_wakes() {
        for closed in [0, 500, DOWN, STARTED, 6_000] {
            let facts = WakeFacts {
                turn_closed_ms: closed,
                ..idle()
            };
            assert!(
                !wakes_after_outage(&facts, DOWN, STARTED),
                "closed at {closed}"
            );
        }
    }

    #[test]
    fn a_task_busy_done_waiting_for_a_person_or_paused_is_not_woken() {
        let cases = [
            WakeFacts {
                running: false,
                ..idle()
            },
            WakeFacts {
                turn_open: true,
                ..idle()
            },
            WakeFacts {
                goal_achieved: true,
                ..idle()
            },
            WakeFacts {
                needs_you: true,
                ..idle()
            },
            WakeFacts {
                paused: true,
                ..idle()
            },
            WakeFacts {
                person_typed: true,
                ..idle()
            },
        ];
        for facts in cases {
            assert!(!wakes_after_outage(&facts, DOWN, STARTED), "{facts:?}");
        }
    }

    #[test]
    fn alive_at_is_written_and_read_back() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        assert_eq!(read_alive_at(dir.path()), None, "no file: unknown");
        write_alive_at(dir.path(), Timestamp(1_761_900_000_000))?;
        assert_eq!(
            read_alive_at(dir.path()),
            Some(Timestamp(1_761_900_000_000))
        );
        std::fs::write(dir.path().join(ALIVE_FILE), "garbage")?;
        assert_eq!(read_alive_at(dir.path()), None, "unreadable: unknown");
        Ok(())
    }
}
