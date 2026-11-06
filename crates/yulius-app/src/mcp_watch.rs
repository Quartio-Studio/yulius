//! A session whose Yulius MCP went silent is reconnected (SPEC-432 MCR-002..MCR-004, ADR-0276).
//!
//! Claude Code can drop a session's `yulius` MCP server for good: a stdio server that does not
//! connect within its bound is marked failed and its bridge closed, and nothing a hook returns
//! brings it back. The bridge (`yulius mcp`) pings Yulius every minute, and every request of the
//! session's MCP client to `POST /mcp` records the contact (`yulius tool` calls do not: they come
//! from the shell). A live, idle task silent for [`SILENT_AFTER_MS`] gets `/mcp reconnect yulius`
//! typed into its session; once its MCP is heard again, a task whose turn closed while it was
//! surely cut off is told to continue. This amends SPEC-028 MSR-004 ("nothing is typed into an
//! idle session") for this one case.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::AppError;

/// A task's MCP silent this long (ms) was dropped by Claude Code: its bridge pings every minute.
pub const SILENT_AFTER_MS: i64 = 3 * 60 * 1000;
/// Reconnects of one task are at least this far apart (ms).
pub const RECONNECT_GAP_MS: i64 = 10 * 60 * 1000;
/// Reconnects that went unanswered in a row before Yulius tries only every [`BACKOFF_GAP_MS`].
pub const MAX_UNANSWERED: u32 = 3;
/// The gap (ms) between reconnects past [`MAX_UNANSWERED`]: a sandbox that could not reach Yulius
/// for a while gets its MCP back once it can, without a reconnect every 10 minutes meanwhile.
pub const BACKOFF_GAP_MS: i64 = 60 * 60 * 1000;
/// A bridge that last pinged this long ago (ms) is surely gone: it pings every 60 s.
pub const PING_GRACE_MS: i64 = 90 * 1000;
/// How often the watch runs.
pub const WATCH_EVERY: Duration = Duration::from_secs(30);
/// What reconnects the session's Yulius MCP (Claude Code's inline `/mcp` action).
pub const RECONNECT_TEXT: &str = "/mcp reconnect yulius";
/// What a task whose turn closed while it was cut off is told once its tools are back.
pub const CONTINUE_TEXT: &str = "Yulius's tools are reconnected; continue";
/// The JSON-RPC id of the bridge's own pings (`yulius mcp`).
pub const MCP_PING_ID: &str = "yulius-bridge-alive";
/// The JSON-RPC id of `yulius tool`'s calls: they say nothing about the session's MCP client.
pub const SHELL_CALL_ID: &str = "yulius-tool";

/// The watch's bounds: the constants above in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchBounds {
    /// When this server started: no task is silent before it has been up this long, and a run
    /// spawned before it was reattached.
    pub started: Timestamp,
    /// Silence (ms) that counts as a dropped server.
    pub silent_after_ms: i64,
    /// Least time (ms) between two reconnects of one task.
    pub reconnect_gap_ms: i64,
    /// The gap (ms) once [`MAX_UNANSWERED`] went unanswered.
    pub backoff_gap_ms: i64,
    /// How long (ms) after its last ping a bridge is surely gone.
    pub ping_grace_ms: i64,
}

impl WatchBounds {
    /// The production bounds for a server started at `started`.
    #[must_use]
    pub fn new(started: Timestamp) -> Self {
        Self {
            started,
            silent_after_ms: SILENT_AFTER_MS,
            reconnect_gap_ms: RECONNECT_GAP_MS,
            backoff_gap_ms: BACKOFF_GAP_MS,
            ping_grace_ms: PING_GRACE_MS,
        }
    }
}

/// What the watch knows of one task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatchFacts {
    /// Now (ms).
    pub now_ms: i64,
    /// The task is Running with a live session.
    pub running: bool,
    /// When the current run was spawned (ms; a reattached run's attempt start).
    pub spawned_ms: i64,
    /// When the session's MCP client last reached `POST /mcp` (ms, 0 = never on this server).
    pub seen_ms: i64,
    /// When its bridge last pinged (ms, 0 = never on this server; an older bridge never pings).
    pub pinged_ms: i64,
    /// A turn is open.
    pub turn_open: bool,
    /// An update holds the tasks, or a person paused this one (its processes may be frozen).
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

/// The watch's state of one run: the reconnects typed since its MCP was last heard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconnects {
    /// When the last one was typed (ms, 0 = none).
    pub last_typed_ms: i64,
    /// How many went unanswered in a row.
    pub unanswered: u32,
    /// From when (ms) the session was surely cut off; `None` when that is not known (a run
    /// reattached with a bridge that never pinged here), so it is never told to continue.
    pub cut_off_from_ms: Option<i64>,
    /// When the watch last found the task held (ms, 0 = never): a frozen bridge pings nothing,
    /// so its silence counts from the thaw.
    pub held_ms: i64,
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

/// Decides what to do to a task, and its state afterwards.
#[must_use]
pub fn decide(
    facts: &WatchFacts,
    state: Reconnects,
    bounds: &WatchBounds,
) -> (WatchAction, Reconnects) {
    if facts.held {
        return (
            WatchAction::None,
            Reconnects {
                held_ms: facts.now_ms,
                ..state
            },
        );
    }
    let idle = facts.running && !facts.turn_open && !facts.person_typed;
    // Heard after a reconnect: the reconnect worked. The agent goes on if its turn closed while
    // it was surely cut off and nobody else is to answer.
    if state.last_typed_ms > 0 && facts.seen_ms > state.last_typed_ms {
        let goes_on = idle
            && state
                .cut_off_from_ms
                .is_some_and(|from| facts.turn_closed_ms > from)
            && !facts.goal_achieved
            && !facts.needs_you;
        let action = if goes_on {
            WatchAction::Continue
        } else {
            WatchAction::None
        };
        return (
            action,
            Reconnects {
                held_ms: state.held_ms,
                ..Reconnects::default()
            },
        );
    }
    let last_contact = facts.seen_ms.max(facts.spawned_ms).max(state.held_ms);
    let silent = facts.now_ms.saturating_sub(last_contact) >= bounds.silent_after_ms
        && facts.now_ms.saturating_sub(bounds.started.0) >= bounds.silent_after_ms;
    let gap = if state.unanswered >= MAX_UNANSWERED {
        bounds.backoff_gap_ms
    } else {
        bounds.reconnect_gap_ms
    };
    let spaced =
        state.last_typed_ms == 0 || facts.now_ms.saturating_sub(state.last_typed_ms) >= gap;
    if !idle || !silent || !spaced {
        return (WatchAction::None, state);
    }
    let cut_off_from_ms = if state.unanswered > 0 {
        state.cut_off_from_ms
    } else if facts.pinged_ms > 0 {
        // A bridge that pings every minute and stopped.
        Some(facts.pinged_ms + bounds.ping_grace_ms)
    } else if facts.spawned_ms >= bounds.started.0 {
        // Spawned by this server, with a bridge that pings, and never pinged: cut off since its
        // last contact, or its spawn if it never connected.
        Some(facts.spawned_ms.max(facts.seen_ms))
    } else {
        None
    };
    (
        WatchAction::Reconnect,
        Reconnects {
            last_typed_ms: facts.now_ms,
            unanswered: state.unanswered.saturating_add(1),
            cut_off_from_ms,
            held_ms: state.held_ms,
        },
    )
}

impl App {
    /// Records that the session of `task` reached `POST /mcp` now with JSON-RPC `id` (MCR-002):
    /// its bridge's ping, or any call of its MCP client; not a `yulius tool` call.
    pub fn note_mcp_contact(&self, task: &TaskId, id: &serde_json::Value) {
        if id.as_str() == Some(SHELL_CALL_ID) {
            return;
        }
        let Some(runtime) = self.existing_runtime(task) else {
            return;
        };
        let now = self.now().0;
        runtime.mcp_seen_ms.store(now, Ordering::SeqCst);
        if id.as_str() == Some(MCP_PING_ID) {
            runtime.mcp_pinged_ms.store(now, Ordering::SeqCst);
        }
    }

    /// When the session of `task` last reached `POST /mcp` on this server (ms, 0 = never).
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
                // A session not ready for input yet is refused by `type_into_session`.
                running: task.status == TaskStatus::Running,
                spawned_ms: runtime.spawned_ms.load(Ordering::SeqCst),
                seen_ms: runtime.mcp_seen_ms.load(Ordering::SeqCst),
                pinged_ms: runtime.mcp_pinged_ms.load(Ordering::SeqCst),
                turn_open: runtime.turn_in_progress(),
                held: update_holds
                    || !matches!(self.deps.store.open_pause(&task.id).await, Ok(None)),
                person_typed: self.typed_recently(&task.id).is_some(),
                turn_closed_ms: runtime.turn_closed_ms.load(Ordering::SeqCst),
                goal_achieved: runtime.goal_achieved.load(Ordering::SeqCst),
                needs_you: !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None)),
            };
            let Ok(current) = runtime.mcp_reconnects.lock().map(|state| *state) else {
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
                match self.type_into_session(&task, paste.as_bytes()).await {
                    Ok(()) => {
                        tracing::info!(task = %task.id, "typed {text:?}: its Yulius MCP was silent");
                        typed.push((task.id.clone(), action));
                    }
                    // Not ready for input, or a person is typing: tried again at the next pass.
                    Err(AppError::Conflict(why)) => {
                        tracing::debug!(task = %task.id, why, "{text:?} waits");
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(task = %task.id, error = %err, "could not type {text:?}");
                        continue;
                    }
                }
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

    /// A server started at 0.
    fn bounds() -> WatchBounds {
        WatchBounds::new(Timestamp(0))
    }

    /// An idle task spawned by this server at 10 minutes, whose bridge last pinged at 12.
    fn silent() -> WatchFacts {
        WatchFacts {
            now_ms: 20 * MIN,
            running: true,
            spawned_ms: 10 * MIN,
            seen_ms: 12 * MIN,
            pinged_ms: 12 * MIN,
            turn_closed_ms: 15 * MIN,
            ..WatchFacts::default()
        }
    }

    /// The facts once the reconnected session is heard, a minute after the reconnect.
    fn heard(facts: WatchFacts) -> WatchFacts {
        WatchFacts {
            now_ms: facts.now_ms + MIN,
            seen_ms: facts.now_ms + 5_000,
            ..facts
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
                cut_off_from_ms: Some(12 * MIN + PING_GRACE_MS),
                held_ms: 0,
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
                pinged_ms: 0,
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
    fn a_busy_typed_into_or_stopped_task_is_not_reconnected() {
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
                person_typed: true,
                ..silent()
            },
        ];
        for facts in cases {
            let (action, _) = decide(&facts, Reconnects::default(), &bounds());
            assert_eq!(action, WatchAction::None, "{facts:?}");
        }
    }

    /// Review round 1 (minor 4): a held task's frozen bridge pings nothing; its silence counts
    /// from the thaw.
    #[test]
    fn a_held_task_is_left_alone_and_its_silence_counts_from_the_thaw() {
        let held = WatchFacts {
            held: true,
            now_ms: 19 * MIN,
            ..silent()
        };
        let (action, state) = decide(&held, Reconnects::default(), &bounds());
        assert_eq!(action, WatchAction::None);
        assert_eq!(state.held_ms, 19 * MIN);
        // Thawed a minute later: not silent for 3 minutes yet.
        assert_eq!(decide(&silent(), state, &bounds()).0, WatchAction::None);
        let later = WatchFacts {
            now_ms: 22 * MIN,
            ..silent()
        };
        assert_eq!(decide(&later, state, &bounds()).0, WatchAction::Reconnect);
    }

    /// Review round 1 (minor 1): past 3 unanswered reconnects, they go on hourly.
    #[test]
    fn reconnects_are_spaced_then_back_off() {
        let (_, first) = decide(&silent(), Reconnects::default(), &bounds());
        let at = |minutes: i64| WatchFacts {
            now_ms: minutes * MIN,
            ..silent()
        };
        assert_eq!(decide(&at(25), first, &bounds()).0, WatchAction::None);
        let (action, second) = decide(&at(30), first, &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(second.unanswered, 2);
        assert_eq!(
            second.cut_off_from_ms, first.cut_off_from_ms,
            "the first cut is kept"
        );
        let (action, third) = decide(&at(40), second, &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(decide(&at(90), third, &bounds()).0, WatchAction::None);
        assert_eq!(decide(&at(100), third, &bounds()).0, WatchAction::Reconnect);
    }

    #[test]
    fn heard_again_a_task_cut_off_mid_work_continues_once() {
        let (_, typed) = decide(&silent(), Reconnects::default(), &bounds());
        let (action, next) = decide(&heard(silent()), typed, &bounds());
        assert_eq!(action, WatchAction::Continue);
        assert_eq!(next, Reconnects::default());
        assert_eq!(
            decide(&heard(silent()), next, &bounds()).0,
            WatchAction::None,
            "once"
        );
    }

    #[test]
    fn heard_again_a_task_idle_before_busy_or_waiting_does_not_continue() {
        let (_, typed) = decide(&silent(), Reconnects::default(), &bounds());
        let cases = [
            // Its last turn closed within a ping of the last one: maybe before the cut.
            WatchFacts {
                turn_closed_ms: 12 * MIN + 30_000,
                ..heard(silent())
            },
            WatchFacts {
                turn_open: true,
                ..heard(silent())
            },
            WatchFacts {
                goal_achieved: true,
                ..heard(silent())
            },
            WatchFacts {
                needs_you: true,
                ..heard(silent())
            },
            WatchFacts {
                person_typed: true,
                ..heard(silent())
            },
        ];
        for facts in cases {
            let (action, next) = decide(&facts, typed, &bounds());
            assert_eq!(action, WatchAction::None, "{facts:?}");
            assert_eq!(next, Reconnects::default(), "{facts:?}");
        }
    }

    /// Review round 1 (M1): after the release, a reattached session's older bridge never pings.
    /// It is reconnected, but never told to continue: when it was cut off is not known, whether
    /// it finished hours ago or used Yulius just before its turn closed.
    #[test]
    fn a_reattached_session_that_never_pinged_is_reconnected_but_not_woken() {
        let restarted = WatchBounds::new(Timestamp(60 * MIN));
        let reattached = WatchFacts {
            now_ms: 64 * MIN,
            spawned_ms: 5 * MIN,
            seen_ms: 0,
            pinged_ms: 0,
            turn_closed_ms: 30 * MIN,
            ..silent()
        };
        // An older bridge whose last call came just before its turn closed after the start.
        let busy = WatchFacts {
            now_ms: 65 * MIN,
            seen_ms: 61 * MIN,
            turn_closed_ms: 61 * MIN + 5_000,
            ..reattached
        };
        for facts in [reattached, busy] {
            let (action, typed) = decide(&facts, Reconnects::default(), &restarted);
            assert_eq!(action, WatchAction::Reconnect, "{facts:?}");
            assert_eq!(typed.cut_off_from_ms, None, "{facts:?}");
            assert_eq!(
                decide(&heard(facts), typed, &restarted).0,
                WatchAction::None,
                "{facts:?}"
            );
        }
    }

    /// A session this server spawned whose bridge never pinged never had its tools: a turn
    /// closed after its spawn continues.
    #[test]
    fn a_session_spawned_here_that_never_connected_continues() {
        let never = WatchFacts {
            seen_ms: 0,
            pinged_ms: 0,
            ..silent()
        };
        let (action, typed) = decide(&never, Reconnects::default(), &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(typed.cut_off_from_ms, Some(10 * MIN));
        assert_eq!(
            decide(&heard(never), typed, &bounds()).0,
            WatchAction::Continue
        );
    }

    /// Review round 2: a run spawned here whose MCP client connected but whose bridge died
    /// before its first ping was cut off after that contact, not from its spawn.
    #[test]
    fn a_session_heard_before_its_first_ping_is_cut_off_from_that_contact() {
        let connected = WatchFacts {
            seen_ms: 11 * MIN,
            pinged_ms: 0,
            turn_closed_ms: 11 * MIN - 5_000,
            ..silent()
        };
        let (action, typed) = decide(&connected, Reconnects::default(), &bounds());
        assert_eq!(action, WatchAction::Reconnect);
        assert_eq!(typed.cut_off_from_ms, Some(11 * MIN));
        assert_eq!(
            decide(&heard(connected), typed, &bounds()).0,
            WatchAction::None,
            "its turn closed before the contact"
        );
    }
}
