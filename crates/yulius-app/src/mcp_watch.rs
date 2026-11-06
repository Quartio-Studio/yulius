//! A session whose Yulius MCP went silent is reconnected (SPEC-432 MCR-002..MCR-004, ADR-0276).
//!
//! Claude Code can drop a session's `yulius` MCP server for good: a stdio server that does not
//! connect within its bound is marked failed and its bridge closed, and nothing a hook returns
//! brings it back. The bridge (`yulius mcp`) pings Yulius every minute, and every request of a
//! task's agent to `POST /mcp` records the contact. A live, idle task silent for
//! [`SILENT_AFTER_MS`] gets `/mcp reconnect yulius` typed into its session; once its bridge is
//! heard again, a task whose turn closed while it was silent is told to continue. This amends
//! SPEC-028 MSR-004 ("nothing is typed into an idle session") for this one case.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::context::App;

/// A task's bridge silent this long (ms) has been dropped by Claude Code: it pings every minute.
pub const SILENT_AFTER_MS: i64 = 3 * 60 * 1000;
/// Reconnects of one task are at least this far apart (ms).
pub const RECONNECT_GAP_MS: i64 = 10 * 60 * 1000;
/// Reconnects that went unanswered in a row before Yulius stops typing them.
pub const MAX_UNANSWERED: u32 = 3;
/// How often the watch runs.
pub const WATCH_EVERY: Duration = Duration::from_secs(30);
/// What reconnects the session's Yulius MCP (Claude Code's inline `/mcp` action).
pub const RECONNECT_TEXT: &str = "/mcp reconnect yulius";
/// What a task whose turn closed while it was cut off is told once its tools are back.
pub const CONTINUE_TEXT: &str = "Yulius's tools are reconnected; continue";

/// The watch's bounds: [`SILENT_AFTER_MS`] and [`RECONNECT_GAP_MS`] in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchBounds {
    /// When this server started: no task is silent before it has been up this long.
    pub started: Timestamp,
    /// Silence (ms) that counts as a dropped server.
    pub silent_after_ms: i64,
    /// Least time (ms) between two reconnects of one task.
    pub reconnect_gap_ms: i64,
}

impl WatchBounds {
    /// The production bounds for a server started at `started`.
    #[must_use]
    pub fn new(started: Timestamp) -> Self {
        Self {
            started,
            silent_after_ms: SILENT_AFTER_MS,
            reconnect_gap_ms: RECONNECT_GAP_MS,
        }
    }
}

/// What the watch knows of one task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchFacts {
    /// Now (ms).
    pub now_ms: i64,
    /// The task is Running with a live Claude Code session whose SessionStart arrived.
    pub running: bool,
    /// When the current run was spawned (ms).
    pub spawned_ms: i64,
    /// When its agent last reached `POST /mcp` (ms, 0 = never on this server).
    pub seen_ms: i64,
    /// A turn is open.
    pub turn_open: bool,
    /// An update holds the tasks, or a person paused this one.
    pub held: bool,
    /// A person typed into it in the last minute.
    pub person_typed: bool,
    /// When the record closing its last turn was written (ms, 0 = none).
    pub turn_closed_ms: i64,
    /// Its goal is achieved.
    pub goal_achieved: bool,
    /// It waits for a person (`needs_you`).
    pub needs_you: bool,
}

/// The reconnects typed into one task since its bridge was last heard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconnects {
    /// When the last one was typed (ms, 0 = none).
    pub last_typed_ms: i64,
    /// How many went unanswered in a row.
    pub unanswered: u32,
    /// The last contact before the first of them (ms).
    pub silent_since_ms: i64,
}

/// What the watch does to one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAction {
    /// Nothing.
    None,
    /// Type [`RECONNECT_TEXT`].
    Reconnect,
    /// Type [`CONTINUE_TEXT`].
    Continue,
}

/// Decides what to do to a task, and its reconnects afterwards.
#[must_use]
pub fn decide(
    facts: &WatchFacts,
    reconnects: Reconnects,
    bounds: &WatchBounds,
) -> (WatchAction, Reconnects) {
    let idle = facts.running && !facts.turn_open && !facts.held && !facts.person_typed;
    // Heard after a reconnect: the reconnect worked. The agent goes on if its turn closed while
    // it was cut off and nobody else is to answer.
    if reconnects.last_typed_ms > 0 && facts.seen_ms > reconnects.last_typed_ms {
        let goes_on = idle
            && facts.turn_closed_ms > reconnects.silent_since_ms
            && !facts.goal_achieved
            && !facts.needs_you;
        let action = if goes_on {
            WatchAction::Continue
        } else {
            WatchAction::None
        };
        return (action, Reconnects::default());
    }
    let last_contact = facts.seen_ms.max(facts.spawned_ms);
    let silent = facts.now_ms.saturating_sub(last_contact) >= bounds.silent_after_ms
        && facts.now_ms.saturating_sub(bounds.started.0) >= bounds.silent_after_ms;
    let spaced = reconnects.last_typed_ms == 0
        || facts.now_ms.saturating_sub(reconnects.last_typed_ms) >= bounds.reconnect_gap_ms;
    if !idle || !silent || !spaced || reconnects.unanswered >= MAX_UNANSWERED {
        return (WatchAction::None, reconnects);
    }
    let silent_since_ms = if reconnects.unanswered == 0 {
        last_contact
    } else {
        reconnects.silent_since_ms
    };
    (
        WatchAction::Reconnect,
        Reconnects {
            last_typed_ms: facts.now_ms,
            unanswered: reconnects.unanswered + 1,
            silent_since_ms,
        },
    )
}

impl App {
    /// Records that `task`'s agent reached `POST /mcp` now (MCR-002).
    pub fn note_mcp_contact(&self, task: &TaskId) {
        if let Some(runtime) = self.existing_runtime(task) {
            runtime.mcp_seen_ms.store(self.now().0, Ordering::SeqCst);
        }
    }

    /// When `task`'s agent last reached `POST /mcp` on this server (ms, 0 = never).
    #[must_use]
    pub fn mcp_contact_ms(&self, task: &TaskId) -> i64 {
        self.existing_runtime(task)
            .map_or(0, |runtime| runtime.mcp_seen_ms.load(Ordering::SeqCst))
    }

    /// Runs [`Self::watch_mcp`] every [`WATCH_EVERY`] for a server started at `started`.
    pub fn start_mcp_watch(self: &Arc<Self>, started: Timestamp) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let bounds = WatchBounds::new(started);
            let mut tick = tokio::time::interval(WATCH_EVERY);
            loop {
                tick.tick().await;
                app.watch_mcp(&bounds).await;
            }
        });
    }

    /// One pass over the live tasks: types a reconnect into each idle one whose Yulius MCP went
    /// silent, and the continue text into each one heard again; returns what it typed.
    pub async fn watch_mcp(self: &Arc<Self>, bounds: &WatchBounds) -> Vec<(TaskId, WatchAction)> {
        let attempts = match self.deps.store.live_attempts().await {
            Ok(attempts) => attempts,
            Err(err) => {
                tracing::warn!(error = %err, "the tasks whose Yulius MCP to watch could not be read");
                return Vec::new();
            }
        };
        let update_holds = self.update_holds_starts();
        let mut typed = Vec::new();
        for attempt in attempts {
            let Ok(Some(task)) = self.deps.store.task(&attempt.task_id).await else {
                continue;
            };
            let Some(runtime) = self.existing_runtime(&task.id) else {
                continue;
            };
            let facts = WatchFacts {
                now_ms: self.now().0,
                running: task.status == TaskStatus::Running
                    && runtime.session_started.load(Ordering::SeqCst),
                spawned_ms: runtime.spawned_ms.load(Ordering::SeqCst),
                seen_ms: runtime.mcp_seen_ms.load(Ordering::SeqCst),
                turn_open: runtime.turn_in_progress(),
                held: update_holds
                    || !matches!(self.deps.store.open_pause(&task.id).await, Ok(None)),
                person_typed: self.typed_recently(&task.id).is_some(),
                turn_closed_ms: runtime.turn_closed_ms.load(Ordering::SeqCst),
                goal_achieved: runtime.goal_achieved.load(Ordering::SeqCst),
                needs_you: !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None)),
            };
            let Ok(current) = runtime.mcp_reconnects.lock().map(|slot| *slot) else {
                continue;
            };
            let (action, next) = decide(&facts, current, bounds);
            let text = match action {
                WatchAction::None => None,
                WatchAction::Reconnect => Some(RECONNECT_TEXT),
                WatchAction::Continue => Some(CONTINUE_TEXT),
            };
            if let Some(text) = text {
                let paste = format!("\x1b[200~{text}\x1b[201~");
                if let Err(err) = self.type_into_session(&task, paste.as_bytes()).await {
                    tracing::warn!(task = %task.id, error = %err, "could not type {text:?}");
                    continue;
                }
                tracing::info!(task = %task.id, "typed {text:?}: its Yulius MCP was silent");
                typed.push((task.id.clone(), action));
            }
            if let Ok(mut stored) = runtime.mcp_reconnects.lock() {
                *stored = next;
            }
        }
        typed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60 * 1000;

    fn bounds() -> WatchBounds {
        WatchBounds::new(Timestamp(0))
    }

    /// An idle task spawned 10 minutes into the server's life, last heard at 12 minutes.
    fn silent() -> WatchFacts {
        WatchFacts {
            now_ms: 20 * MIN,
            running: true,
            spawned_ms: 10 * MIN,
            seen_ms: 12 * MIN,
            turn_closed_ms: 15 * MIN,
            ..WatchFacts::default()
        }
    }

    #[test]
    fn a_silent_idle_task_is_reconnected() {
        let (action, next) = decide(&silent(), Reconnects::default(), &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(
            next,
            Reconnects {
                last_typed_ms: 20 * MIN,
                unanswered: 1,
                silent_since_ms: 12 * MIN,
            }
        );
    }

    #[test]
    fn a_task_heard_lately_or_just_started_is_left_alone() {
        let cases = [
            WatchFacts {
                seen_ms: 18 * MIN,
                ..silent()
            },
            // Spawned 2 minutes ago, never heard: its bridge may still be connecting.
            WatchFacts {
                spawned_ms: 18 * MIN,
                seen_ms: 0,
                ..silent()
            },
        ];
        for facts in cases {
            let (action, _) = decide(&facts, Reconnects::default(), &bounds());
            assert_eq!(action, WatchAction::None, "{facts:?}");
        }
        // A server up for less than the silence: the bridges reconnect after its start.
        let young = WatchBounds::new(Timestamp(18 * MIN));
        let (action, _) = decide(&silent(), Reconnects::default(), &young);
        assert_eq!(action, WatchAction::None);
    }

    #[test]
    fn a_busy_held_typed_into_or_stopped_task_is_not_reconnected() {
        let cases = [
            WatchFacts {
                running: false,
                ..silent()
            },
            WatchFacts {
                turn_open: true,
                ..silent()
            },
            WatchFacts {
                held: true,
                ..silent()
            },
            WatchFacts {
                person_typed: true,
                ..silent()
            },
        ];
        for facts in cases {
            let (action, _) = decide(&facts, Reconnects::default(), &bounds());
            assert_eq!(action, WatchAction::None, "{facts:?}");
        }
    }

    #[test]
    fn reconnects_are_spaced_and_capped() {
        let (_, first) = decide(&silent(), Reconnects::default(), &bounds());
        let soon = WatchFacts {
            now_ms: 25 * MIN,
            ..silent()
        };
        assert_eq!(decide(&soon, first, &bounds()).0, WatchAction::None);
        let later = WatchFacts {
            now_ms: 30 * MIN,
            ..silent()
        };
        let (action, second) = decide(&later, first, &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(second.unanswered, 2);
        assert_eq!(second.silent_since_ms, 12 * MIN, "the first silence is kept");
        let (action, third) = decide(
            &WatchFacts {
                now_ms: 40 * MIN,
                ..silent()
            },
            second,
            &bounds(),
        );
        assert_eq!(action, WatchAction::Reconnect);
        let (action, _) = decide(
            &WatchFacts {
                now_ms: 90 * MIN,
                ..silent()
            },
            third,
            &bounds(),
        );
        assert_eq!(action, WatchAction::None, "a fourth unanswered is never typed");
    }

    #[test]
    fn heard_again_a_task_cut_off_mid_work_continues_once() {
        let (_, typed) = decide(&silent(), Reconnects::default(), &bounds());
        let heard = WatchFacts {
            now_ms: 21 * MIN,
            seen_ms: 20 * MIN + 5_000,
            ..silent()
        };
        let (action, next) = decide(&heard, typed, &bounds());
        assert_eq!(action, WatchAction::Continue);
        assert_eq!(next, Reconnects::default());
        assert_eq!(decide(&heard, next, &bounds()).0, WatchAction::None, "once");
    }

    #[test]
    fn heard_again_a_task_idle_before_busy_or_waiting_does_not_continue() {
        let (_, typed) = decide(&silent(), Reconnects::default(), &bounds());
        let heard = WatchFacts {
            now_ms: 21 * MIN,
            seen_ms: 20 * MIN + 5_000,
            ..silent()
        };
        let cases = [
            // Its last turn closed before it went silent.
            WatchFacts {
                turn_closed_ms: 11 * MIN,
                ..heard
            },
            WatchFacts {
                turn_open: true,
                ..heard
            },
            WatchFacts {
                goal_achieved: true,
                ..heard
            },
            WatchFacts {
                needs_you: true,
                ..heard
            },
            WatchFacts {
                person_typed: true,
                ..heard
            },
        ];
        for facts in cases {
            let (action, next) = decide(&facts, typed, &bounds());
            assert_eq!(action, WatchAction::None, "{facts:?}");
            assert_eq!(next, Reconnects::default(), "{facts:?}");
        }
    }
}
