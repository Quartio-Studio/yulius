//! When a task is finished (SPEC-001 TSK-014, SPEC-002 §6.1 "Finish rule").
//!
//! A running task is `done` when its turn ended cleanly (the Stop hook did not block) and Claude
//! Code recorded the session's goal as achieved (`goal_status` with `met: true`) with no prompt
//! since. Claude Code writes that record after its Stop hooks, so the order is usually: Stop hook
//! → `waiting`, then the record → `done`; when the record comes first (Yulius' hook was slow or
//! blocked the turn), the next clean Stop makes the task `done`. Any prompt makes it `working`
//! again and the achieved goal no longer counts. A turn whose Stop Yulius never answered (the hook
//! client fails open) settles at the record that closes it (SPEC-290 GDW-001).
//!
//! Background shells the agent left running (Claude Code's "· 1 shell") are probed from the
//! process table while the task is idle — at the clean Stop, when the goal is achieved and every
//! [`SHELL_PROBE_MS`] — and reported with the task; the UI shows a done task with a shell still
//! running as Working ("1 background task still running"). A probe that cannot read the process
//! table, or takes longer than [`SHELL_PROBE_TIMEOUT`], reports nothing: the task is not held
//! back by it (Claude Code wakes the agent when a shell ends anyway). Background agents are no
//! process of that tree: Claude Code says how many it still waits for in the record that closes
//! the turn (`pendingBackgroundAgentCount`, SPEC-032), and the larger of the two counts is the
//! idle task's background work. Monitor streams, and agents launched in a turn that has not
//! closed yet, are read from the transcript's launches and the notifications that end them
//! (SPEC-079): a Monitor counts until it ends or expires, and an agent that stopped with
//! background work of its own still running, which Claude Code's records no longer count, until
//! its final notification (BGW-006). Shells that only run `yulius wait` are
//! no background work (SPEC-050 DSK-102); the probe counts them apart, and the task view says the
//! agent waits for Yulius, which wakes it (SPEC-085 STT-002).
//!
//! While Claude Code's goal is open, its evaluator (a Stop hook after Yulius's own) gives a
//! verdict after each clean Stop: the turn reads idle only once the verdict, the turn's record or
//! [`GOAL_VERDICT_WAIT`] settles it (SPEC-079 BGW-005).
//!
//! After a restart the transcript of a reattached run is read again from its start; it is reduced
//! to its final state ([`TranscriptFacts`]) and applied once, so a replayed prompt cannot put a
//! finished task back to Working, and a goal achieved while Yulius was down is not lost.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use yulius_domain::harness::is_compact_command;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, GoalOutcome, TaskStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;
use crate::ports::harness::Observation;
use crate::ports::session::StrayProcess;

/// How often the background shells of an idle task are probed (ms).
pub const SHELL_PROBE_MS: i64 = 5_000;

/// Longest a shell probe may take before its result counts as unknown.
pub const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an idle task's strays live once seen before they are killed (SPEC-353 LAP-004,
/// ADR-0240): a preview server a person may still look at survives a long break, a busy loop
/// burns hours, not days.
pub const STRAY_REAP_AFTER_MS: i64 = 2 * 3600 * 1000;

/// Claude Code's last goal notice after a turn an error ended (SPEC-333).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalRetryFact {
    /// It armed a retry that has not started a turn yet.
    Armed,
    /// It stopped retrying; its notice.
    Stopped(String),
}

/// The final state of the transcript lines read so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptFacts {
    /// `Some(true)` while a turn is open, `Some(false)` once the last turn ended, `None` before
    /// any turn was seen.
    pub turn_open: Option<bool>,
    /// Whether the session's goal stands achieved; `None` when nothing said either way.
    pub goal_achieved: Option<bool>,
    /// Background agents Claude Code still waited for when the last turn ended (0 after a new
    /// prompt or when it did not say).
    pub background_agents: u32,
    /// The last word on the session's context is that it is full (CTX-007): no reply or
    /// compaction came after the API rejected a prompt as too long.
    pub context_full: bool,
    /// The last turn ended at a usage limit: no prompt, agent work or turn's end came after the
    /// limit record (SPEC-072 LRS-001). A limit Claude Code continued past is history.
    pub limit_pending: bool,
    /// When the session's last prompt, interrupt, agent work or own turn end was recorded: a
    /// `limit` pause older than that was overtaken while Yulius was down (SPEC-072 LRS-001).
    pub last_turn_at: Option<yulius_domain::time::Timestamp>,
    /// A goal is open: set or "not met", not achieved, failed or cleared since (SPEC-079 BGW-005).
    pub goal_open: bool,
    /// Claude Code last recorded the goal as set, not met or failed: its work is unfinished
    /// (SPEC-429 ANS-002).
    pub goal_unfinished: bool,
    /// Claude Code recorded a goal in this session (any outcome): a launch `/goal` was taken
    /// (SPEC-146 GWR-003).
    pub goal_recorded: bool,
    /// The last word after a turn an error ended is Claude Code's goal notice (SPEC-333), until a
    /// prompt, agent work or an interrupt; an armed retry also ends with the goal, and spent
    /// retries with the session's own answer (SPEC-469).
    pub goal_retry: Option<GoalRetryFact>,
    /// Claude Code's notice of a goal pause it made at the pending usage limit or refused
    /// subscription (SPEC-464 UCL-007): the limit's, not a pause nobody handles, unless Yulius
    /// cannot take the task on from there. Until the next turn or limit.
    pub limit_goal_pause: Option<String>,
    /// When the session's agent first worked (a line of the model's own, or its answer) after
    /// Claude Code last stopped retrying the goal or a usage limit last cut its turn, or first
    /// worked at all when what was read holds neither: a request made because the session did
    /// not go on by itself is answered (SPEC-469 GPW-004, SPEC-464 UCL-010). A prompt alone, a
    /// sub-agent's answer and work without a time are no such work.
    pub worked_since_goal_pause: Option<yulius_domain::time::Timestamp>,
    /// When the last record that closes a session's turn (its answer or its `turn_duration`)
    /// was written: a turn closed while Yulius was down is woken after it (SPEC-358 MSB-007).
    pub turn_closed_at: Option<yulius_domain::time::Timestamp>,
    /// When the session last moved toward a Stop: its last prompt (typed, or Claude Code's own
    /// wake-up) or its last answer. A Stop Yulius answered after it was the last turn's, so that
    /// turn is not woken after an outage; one answered before it (a blocked Stop, after which the
    /// turn went on) was not (SPEC-358 MSB-007).
    pub turn_moved_at: Option<yulius_domain::time::Timestamp>,
    /// What waits in Claude Code's queue, as the lines read so far say (SPEC-471 CCQ-001).
    pub queue: yulius_domain::agent_cli::Queue,
    /// The dynamic workflows the session launched whose end was not read (SPEC-486 WFL-003).
    pub workflows: crate::dynamic_workflow::Unfinished,
}

impl TranscriptFacts {
    /// Folds one observation, in transcript order.
    pub fn fold(&mut self, observation: &Observation) {
        self.workflows.observe(observation);
        if let Observation::TurnComplete { at: Some(at) }
        | Observation::TurnEnded { at: Some(at), .. } = observation
        {
            self.turn_closed_at = Some(self.turn_closed_at.map_or(*at, |last| last.max(*at)));
        }
        if let Observation::UserTurn { at: Some(at), .. }
        | Observation::TurnComplete { at: Some(at) } = observation
        {
            self.turn_moved_at = Some(self.turn_moved_at.map_or(*at, |last| last.max(*at)));
        }
        if let Observation::UserTurn { at: Some(at), .. }
        | Observation::TurnInterrupted { at: Some(at) }
        | Observation::AgentWork { at: Some(at) }
        | Observation::TurnComplete { at: Some(at) } = observation
        {
            self.last_turn_at = Some(self.last_turn_at.map_or(*at, |last| last.max(*at)));
        }
        match observation {
            Observation::UserTurn { wake, .. } => {
                self.turn_open = Some(true);
                // The turn of what Claude Code handed on from its queue began (SPEC-471).
                self.queue.turn_seen();
                self.goal_retry = None;
                self.limit_goal_pause = None;
                // Claude Code's own wake-up keeps an achieved goal.
                if !wake {
                    self.goal_achieved = Some(false);
                }
                self.background_agents = 0;
                self.limit_pending = false;
            }
            Observation::TurnEnded {
                background_agents, ..
            } => {
                self.turn_open = Some(false);
                self.background_agents = background_agents.unwrap_or(0);
                // What still waits in the queue was left by this turn's end (SPEC-479).
                self.queue.turn_closed();
            }
            Observation::TurnComplete { at } => {
                self.turn_open = Some(false);
                // An answer written after a hand-over is of the turn it started (SPEC-471).
                self.queue.work_seen(*at);
                self.context_full = false;
                // A sub-agent's turn end (no time) says nothing about the session's limit.
                self.limit_pending &= at.is_none();
                if !self.limit_pending {
                    self.limit_goal_pause = None;
                }
                // The session's own answer: the model works, so a pause before it is over.
                if let Some(at) = at {
                    self.worked_since_goal_pause.get_or_insert(*at);
                    if matches!(self.goal_retry, Some(GoalRetryFact::Stopped(_))) {
                        self.goal_retry = None;
                    }
                }
            }
            Observation::TurnInterrupted { .. } => {
                self.turn_open = Some(false);
                self.goal_retry = None;
            }
            // A pause at a usage limit or a refused subscription is the limit's (SPEC-464
            // UCL-007): nobody is asked to send a message for it while Yulius takes the task on.
            Observation::GoalRetry {
                armed: false,
                notice,
            } if self.limit_pending => self.limit_goal_pause = Some(notice.clone()),
            Observation::GoalRetry { armed, notice } => {
                self.goal_retry = Some(if *armed {
                    GoalRetryFact::Armed
                } else {
                    GoalRetryFact::Stopped(notice.clone())
                });
                // Work before the pause does not answer it.
                if !armed {
                    self.worked_since_goal_pause = None;
                }
            }
            // The API rejected the prompt: the turn is over, without a Stop hook (#213).
            Observation::ContextFull => {
                self.turn_open = Some(false);
                self.context_full = true;
            }
            Observation::Compacted => self.context_full = false,
            Observation::Goal(outcome, _) => {
                // Claude Code drops an armed check-in once the goal ended.
                if !goal_stays_open(*outcome) && self.goal_retry == Some(GoalRetryFact::Armed) {
                    self.goal_retry = None;
                }
                self.goal_recorded = true;
                self.goal_open = goal_stays_open(*outcome);
                self.goal_unfinished = goal_unfinished(*outcome);
                self.fold_goal(*outcome);
            }
            // A refused subscription cuts the turn like a limit: the task goes on elsewhere. A
            // limit that refused a workflow's agents leaves the session at it too (SPEC-486).
            Observation::LimitReached { .. }
            | Observation::WorkflowLimitReached { .. }
            | Observation::AccessLost { .. } => {
                self.limit_pending = true;
                self.limit_goal_pause = None;
                // Work before the limit answers no request made at it: its goal pause's (the
                // notice follows this record, SPEC-464 UCL-007), or one for a limit nothing
                // says the end of (UCL-010). SPEC-469's rule for a pause.
                self.worked_since_goal_pause = None;
            }
            Observation::Queue(change, at) => self.queue.record(*change, *at),
            Observation::Session { .. }
            | Observation::CompactFailed
            | Observation::WorkflowLaunched { .. }
            | Observation::BackgroundStarted { .. }
            | Observation::BackgroundEnded { .. }
            | Observation::BackgroundResumable { .. } => {}
            // An assistant line that is not a turn's end after a recorded end: a Stop hook
            // continued the turn (STA-005).
            Observation::AgentWork { at } => {
                self.turn_open = Some(true);
                // Work written after a hand-over is of the turn it started (SPEC-471).
                self.queue.work_seen(*at);
                self.goal_retry = None;
                self.limit_goal_pause = None;
                self.context_full = false;
                self.limit_pending = false;
                if let Some(at) = at {
                    self.worked_since_goal_pause.get_or_insert(*at);
                }
            }
        }
    }

    fn fold_goal(&mut self, outcome: GoalOutcome) {
        match outcome {
            GoalOutcome::Met => {
                self.goal_achieved = Some(true);
                self.turn_open = Some(false);
            }
            GoalOutcome::Failed => {
                self.goal_achieved = Some(false);
                self.turn_open = Some(false);
            }
            GoalOutcome::NotMet => {
                self.goal_achieved = Some(false);
                self.turn_open = Some(true);
            }
            GoalOutcome::Set | GoalOutcome::Cleared => self.goal_achieved = Some(false),
        }
    }
}

/// Whether the goal's work is unfinished after Claude Code recorded `outcome`: set, not met or
/// failed (SPEC-429 ANS-002).
#[must_use]
pub(crate) fn goal_unfinished(outcome: GoalOutcome) -> bool {
    !matches!(outcome, GoalOutcome::Met | GoalOutcome::Cleared)
}

/// Whether the goal is open after Claude Code recorded `outcome`: set or not met yet, so a verdict
/// follows each clean Stop (SPEC-079 BGW-005).
#[must_use]
pub(crate) fn goal_stays_open(outcome: GoalOutcome) -> bool {
    matches!(outcome, GoalOutcome::Set | GoalOutcome::NotMet)
}

/// Background work the transcript says the session started (SPEC-079 BGW-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Launch {
    /// A background agent; otherwise a Monitor stream.
    pub agent: bool,
    /// When Yulius read it (ms): the same clock as the turn record's, and the same order as the
    /// transcript's lines.
    pub seen_ms: i64,
    /// When a Monitor expires (ms, from its record's time); `None` for an agent.
    pub expires_ms: Option<i64>,
    /// An agent that stopped with background work of its own still running (BGW-006): it resumes
    /// by itself and notifies again. `seen_ms` is then when that notification was read.
    pub resumable: bool,
}

/// Most launches remembered per session: the oldest go first.
const MAX_LAUNCHES: usize = 64;

/// How long an agent launch with no notification of its end still counts for a switch, and a
/// resumable agent for the task's status, from its launch or its last notification: a lost
/// notification holds them at most this long.
const AGENT_LAUNCH_TTL_MS: i64 = 60 * 60 * 1000;

/// The launches alive at `now_ms` (BGW-003, BGW-004, BGW-006).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LiveLaunches {
    /// Monitors not past their expiry.
    pub monitors: u32,
    /// Agents read after the last turn record (`record_ms`), whose count it did not say yet.
    pub agents_this_turn: u32,
    /// Agents with no notification of their end yet, younger than [`AGENT_LAUNCH_TTL_MS`]: a turn
    /// a wake-up or a prompt started in between does not end them. Resumable agents included.
    pub agents_pending: u32,
    /// Agents that stopped with background work of their own still running and did not notify
    /// their end yet, their last notification younger than [`AGENT_LAUNCH_TTL_MS`] (BGW-006).
    pub agents_resumable: u32,
}

impl LiveLaunches {
    /// The idle task's background agents and Monitors, with `turn_agents` from the last turn
    /// record (`pendingBackgroundAgentCount`). The agents of this turn and the resumable ones are
    /// another view of what that record counts, not added to it: the record after a resumable
    /// agent's notification does not count it (task #74), one after it resumed may.
    #[must_use]
    pub fn background(self, turn_agents: u32) -> u32 {
        turn_agents
            .max(self.agents_this_turn.saturating_add(self.agents_resumable))
            .saturating_add(self.monitors)
    }
}

/// Counts `launches` at `now_ms` with the last turn record seen at `record_ms`.
#[must_use]
pub(crate) fn live_launches(
    launches: &std::collections::BTreeMap<String, Launch>,
    now_ms: i64,
    record_ms: i64,
) -> LiveLaunches {
    let mut live = LiveLaunches::default();
    for launch in launches.values() {
        match launch.expires_ms {
            Some(expires) if expires > now_ms => live.monitors += 1,
            None if launch.agent => {
                let young = now_ms.saturating_sub(launch.seen_ms) < AGENT_LAUNCH_TTL_MS;
                if launch.resumable {
                    live.agents_resumable += u32::from(young);
                } else {
                    live.agents_this_turn += u32::from(launch.seen_ms > record_ms);
                }
                live.agents_pending += u32::from(young);
            }
            _ => {}
        }
    }
    live
}

/// The clean Stop (ms) whose turn waits for its goal verdict (0 = none). Whoever takes it settles
/// that turn: the verdict, agent work, a prompt, an interrupt, the turn record or the waiter's
/// timeout (SPEC-079 BGW-005).
#[derive(Debug, Default)]
pub(crate) struct VerdictSlot(AtomicI64);

impl VerdictSlot {
    /// The turn of the Stop at `stop_ms` waits for its verdict (a newer Stop replaces an older).
    pub fn open(&self, stop_ms: i64) {
        self.0.store(stop_ms, Ordering::SeqCst);
    }

    /// No turn waits (a prompt or an interrupt started or ended one; a new session).
    pub fn clear(&self) {
        self.0.store(0, Ordering::SeqCst);
    }

    /// The Stop whose turn waits (0 = none).
    pub fn pending(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Takes the slot if the turn of the Stop at `stop_ms` still waits.
    pub fn take(&self, stop_ms: i64) -> bool {
        stop_ms != 0
            && self
                .0
                .compare_exchange(stop_ms, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }

    /// Takes the slot whatever Stop waits.
    pub fn take_any(&self) -> bool {
        self.0.swap(0, Ordering::SeqCst) != 0
    }
}

/// Remembers `launch` as `id`, forgetting expired Monitors and, beyond [`MAX_LAUNCHES`], the
/// oldest launches.
pub(crate) fn remember_launch(
    launches: &mut std::collections::BTreeMap<String, Launch>,
    id: String,
    launch: Launch,
    now_ms: i64,
) {
    launches.retain(|_, l| l.expires_ms.is_none_or(|expires| expires > now_ms));
    launches.insert(id, launch);
    while launches.len() > MAX_LAUNCHES {
        let oldest = launches
            .iter()
            .min_by_key(|(_, l)| l.seen_ms)
            .map(|(id, _)| id.clone());
        match oldest {
            Some(id) => launches.remove(&id),
            None => break,
        };
    }
}

/// The agent `id` stopped with background work of its own still running (BGW-006): it stays
/// live, resumable, with its TTL counted from now. One whose launch Yulius did not see (read
/// before a restart, or forgotten) is remembered too: the notification says it notifies again.
pub(crate) fn remember_resumable(
    launches: &mut std::collections::BTreeMap<String, Launch>,
    id: String,
    now_ms: i64,
) {
    let launch = Launch {
        agent: true,
        seen_ms: now_ms,
        expires_ms: None,
        resumable: true,
    };
    remember_launch(launches, id, launch, now_ms);
}

/// How long a clean Stop of a session with an open goal waits for the verdict before the turn
/// reads idle anyway (SPEC-079 BGW-005).
pub const GOAL_VERDICT_WAIT: Duration = Duration::from_secs(20);

/// The idle task's background work: the shell probe's count (`-1` = not known) and the background
/// agents Claude Code counted. An unknown probe stays unknown without agents.
#[must_use]
pub(crate) fn combine_background(probe: i64, agents: u32) -> i64 {
    if agents > 0 {
        probe.max(i64::from(agents))
    } else {
        probe
    }
}

/// The first pid of each of `strays`, at most 20 (for the log).
pub(crate) fn first_pids(strays: &[StrayProcess]) -> Vec<u32> {
    strays
        .iter()
        .filter_map(|stray| stray.processes.first().map(|(pid, _)| *pid))
        .take(20)
        .collect()
}

/// Records the strays `found` now in `seen` (each stray's first process, `(pid, started)`, to
/// when it was first seen), forgetting those gone, and returns the strays seen for the first time
/// and those seen for `bound_ms` or longer (SPEC-353 LAP-004).
pub(crate) fn sort_strays(
    seen: &mut HashMap<(u32, u64), i64>,
    found: Vec<StrayProcess>,
    now: i64,
    bound_ms: i64,
) -> (Vec<StrayProcess>, Vec<StrayProcess>) {
    let keys: HashSet<(u32, u64)> = found
        .iter()
        .filter_map(|stray| stray.processes.first().copied())
        .collect();
    seen.retain(|key, _| keys.contains(key));
    let (mut new, mut due) = (Vec::new(), Vec::new());
    for stray in found {
        let Some(key) = stray.processes.first().copied() else {
            continue;
        };
        let first = *seen.entry(key).or_insert_with(|| {
            new.push(stray.clone());
            now
        });
        if now.saturating_sub(first) >= bound_ms {
            due.push(stray);
        }
    }
    (new, due)
}

/// Whether the record that closes the turn of a clean Stop (`stop_ms`, 0 = none) is still
/// expected: not seen since (`record_ms`) and the Stop is younger than [`TURN_RECORD_WAIT`].
#[must_use]
pub(crate) fn turn_record_pending(stop_ms: i64, record_ms: i64, now_ms: i64) -> bool {
    stop_ms > 0
        && record_ms < stop_ms
        && now_ms.saturating_sub(stop_ms) < TURN_RECORD_WAIT.as_millis() as i64
}

/// How long a decision at a clean Stop waits for the record that closes the turn, which says how
/// many background agents are pending (it is written after the Stop hooks, about a poll later).
const TURN_RECORD_WAIT: Duration = Duration::from_secs(3);

/// How long a Stop that decided a switch waits for the watcher to read the lines before it: two
/// rounds of [`crate::supervisor::WATCH_INTERVAL_MS`] and their work.
const TRANSCRIPT_READ_WAIT: Duration = Duration::from_secs(5);

/// What assistant work seen in the transcript (`at`) does to a running task's activity
/// (STA-003, STA-005): after the last clean stop (`last_stop_ms`) it shows the session goes on, so
/// an idle task is Working again (a Stop hook continued the turn, a wake-up started one); and
/// after the notice that flagged it (`notice_ms`, 0 = none) it shows the person's answer arrived.
/// Work without a time is not trusted: a line read late must not reopen a finished turn.
#[must_use]
pub fn activity_on_work(
    current: Option<Activity>,
    at: Option<yulius_domain::time::Timestamp>,
    last_stop_ms: i64,
    notice_ms: i64,
) -> Option<Activity> {
    let after = |ms: i64| at.is_some_and(|at| at.0 > ms);
    match current {
        Some(Activity::Waiting | Activity::Done) if after(last_stop_ms) => Some(Activity::Working),
        Some(Activity::NeedsAttention) if notice_ms > 0 && after(notice_ms) => {
            Some(Activity::Working)
        }
        _ => None,
    }
}

/// What a goal verdict of "not met" does (STA-004): the session carries on, so an idle task is
/// Working again. Claude Code's evaluator is a Stop hook that runs after Yulius's own, which has
/// already made the task idle.
#[must_use]
pub fn activity_on_goal_not_met(current: Option<Activity>) -> Option<Activity> {
    matches!(current, Some(Activity::Waiting | Activity::Done)).then_some(Activity::Working)
}

/// Whether a `UserPromptSubmit` prompt is one a person (or Yulius) typed, not a wake-up Claude
/// Code generated itself (task notifications and other tagged prompts start with `<`), nor the
/// compaction command, which asks for no new work (SPEC-085 STT-005).
#[must_use]
pub fn is_typed_prompt(prompt: Option<&str>) -> bool {
    prompt
        .map(str::trim_start)
        .is_some_and(|p| !p.is_empty() && !p.starts_with('<') && !is_compact_command(p))
}

/// Whether a `UserPromptSubmit` prompt came through the session's prompt box, typed by a person
/// or by Yulius (a command like `/compact` too), which a submit empties; Claude Code's own
/// wake-ups (they start with `<`) do not pass through it (SPEC-468 CCF-010).
#[must_use]
pub fn from_prompt_box(prompt: Option<&str>) -> bool {
    prompt
        .map(str::trim_start)
        .is_some_and(|p| !p.is_empty() && !p.starts_with('<'))
}

/// Whether a submitted prompt starts a turn of the agent: anything but the compaction command,
/// after which Claude Code compacts the conversation and runs no turn, so nothing would ever end
/// it (SPEC-085 STT-005).
#[must_use]
pub fn prompt_starts_work(prompt: Option<&str>) -> bool {
    !prompt.is_some_and(is_compact_command)
}

/// Whether the record that closes a turn, written at `record_at`, closes a turn whose Stop Yulius
/// never answered (SPEC-290 GDW-001): the run reports through hooks (`hooks`), a turn began
/// (`began_ms`) after the last answered Stop (`answered_ms`), no Stop of the run is being answered
/// now (`stops_in_flight`), and the record was written after that turn began. A record read late,
/// after a newer prompt, belongs to the turn before; one without a time says nothing.
#[must_use]
pub(crate) fn closes_unanswered_turn(
    hooks: bool,
    answered_ms: i64,
    began_ms: i64,
    stops_in_flight: u32,
    record_at: Option<yulius_domain::time::Timestamp>,
) -> bool {
    hooks
        && stops_in_flight == 0
        && began_ms > answered_ms
        && record_at.is_some_and(|at| at.0 > began_ms)
}

/// Whether a Stop that Yulius failed to answer settles its turn itself (SPEC-290 GDW-001): the
/// Stop is the live run's (`live`), and the record that closes its turn was written
/// (`record_at_ms`) after that turn began (`began_ms`) while the Stop was being answered.
#[must_use]
pub(crate) fn failed_stop_settles(live: bool, record_at_ms: i64, began_ms: i64) -> bool {
    live && began_ms > 0 && record_at_ms > began_ms
}

/// A Stop hook of the run being answered (SPEC-290 GDW-001): counted in
/// [`crate::supervisor::TaskRuntime::stops_in_flight`] until dropped, also when its handling
/// panics.
pub(crate) struct StopInFlight(Arc<crate::supervisor::TaskRuntime>);

impl StopInFlight {
    /// Counts a Stop of `runtime`'s run as being answered.
    pub(crate) fn enter(runtime: &Arc<crate::supervisor::TaskRuntime>) -> Self {
        runtime.stops_in_flight.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(runtime))
    }
}

impl Drop for StopInFlight {
    fn drop(&mut self) {
        self.0.stops_in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a turn that waited for its goal verdict settles as its Stop would have, the task
/// reading `activity` and the goal `achieved`: while it works or needs a person, and once the
/// goal is met while it reads Your turn (its turn's prompt hook came late or never, so it never
/// read Working; SPEC-346 LPH-003). Otherwise it reads settled already (a late replay).
#[must_use]
pub(crate) fn waited_turn_settles(activity: Option<Activity>, achieved: bool) -> bool {
    match activity {
        Some(Activity::Working | Activity::NeedsAttention) => true,
        Some(Activity::Waiting) => achieved,
        _ => false,
    }
}

/// Whether the flag raised at `noticed_ms` (a notice's, or a goal pause's) is the one the task
/// still reads: it reads Needs you, and has since no later than that time. A status change
/// replaces a flag without a word, and one raised afterwards is another matter's, which the
/// agent's work does not answer (SPEC-469).
#[must_use]
pub(crate) fn noticed_flag_stands(
    activity: Option<Activity>,
    state_at: yulius_domain::time::Timestamp,
    noticed_ms: i64,
) -> bool {
    activity == Some(Activity::NeedsAttention) && state_at.0 <= noticed_ms
}

/// Whether a turn closed with no Stop Yulius answered settles now: not while a full context is
/// being recovered (`context_clear` false), at a usage limit, or with an open pause, whose own
/// paths say how the task goes on (SPEC-290 GDW-001).
#[must_use]
pub(crate) fn unanswered_turn_settles(
    context_clear: bool,
    at_limit: bool,
    pause_open: bool,
) -> bool {
    context_clear && !at_limit && !pause_open
}

impl App {
    /// A clean Stop ended the turn (TSK-014): remembers when, and settles it now, or once the goal
    /// verdict came while a goal is open (SPEC-079 BGW-005; Claude Code's evaluator is a Stop
    /// hook that runs after this one, so the hook cannot wait for it).
    pub(crate) async fn clean_stop(self: &Arc<Self>, task: &TaskRecord) -> AppResult<()> {
        let runtime = self.runtime(&task.id);
        crate::race::reached(
            crate::race::Point::StopClean,
            std::path::Path::new(task.id.as_str()),
        );
        // After a prompt the transcript watcher is applying: it passed its last-Stop check and
        // starts its turn first (SPEC-453 TPO-001).
        let stop_ms = {
            let _order = runtime.turn_order.lock().await;
            let stop_ms = self.now().0;
            runtime.last_stop_ms.store(stop_ms, Ordering::SeqCst);
            stop_ms
        };
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        if runtime.goal_open.load(Ordering::SeqCst) && !runtime.goal_achieved.load(Ordering::SeqCst)
        {
            runtime.verdict_stop_ms.open(stop_ms);
            let (app, task_id) = (Arc::clone(self), task.id.clone());
            tokio::spawn(async move { app.await_goal_verdict(&task_id, stop_ms).await });
            return Ok(());
        }
        runtime.verdict_stop_ms.clear();
        // That prompt set Working by now: the Stop settles the task as it is, not as it was read
        // on arrival (the copy if the store fails; SPEC-453 TPO-002).
        let current = self.deps.store.task(&task.id).await.ok().flatten();
        self.settle_clean_turn(current.as_ref().unwrap_or(task))
            .await
    }

    /// A clean turn's end: Done when the goal is achieved, waiting otherwise (TSK-014). Its
    /// background work is probed, a finished task's pull request opens (#35), and once it is Done
    /// its chains start (TSK-012) and the queue may use its slot (SES-101).
    async fn settle_clean_turn(self: &Arc<Self>, task: &TaskRecord) -> AppResult<()> {
        let achieved = self.runtime(&task.id).goal_achieved.load(Ordering::SeqCst);
        let next = Activity::after_clean_turn(achieved);
        self.set_task_activity(task, next).await?;
        let (app, task_id) = (Arc::clone(self), task.id.clone());
        tokio::spawn(async move {
            // The background work first: chains and the queue read it.
            app.probe_idle_shells(&task_id).await;
            app.auto_open_pr(&task_id).await;
            app.advance_chains(&task_id).await;
            if next == Activity::Done {
                app.drain_queue().await;
            }
            // The turn is settled: an idle session on an older Claude Code restarts on the one
            // runs get now (SPEC-461 CCM-013).
            app.move_idle_to_newer_claude_code(&task_id).await;
        });
        Ok(())
    }

    /// Waits (bounded by [`GOAL_VERDICT_WAIT`]) for something to settle the turn of the clean
    /// Stop at `stop_ms`: the verdict, agent work, a prompt, an interrupt, the turn's record or a
    /// newer Stop takes the slot. Nothing did: the turn settles as it did before (BGW-005).
    async fn await_goal_verdict(self: &Arc<Self>, task_id: &TaskId, stop_ms: i64) {
        let runtime = self.runtime(task_id);
        let deadline = tokio::time::Instant::now() + GOAL_VERDICT_WAIT;
        while runtime.verdict_stop_ms.pending() == stop_ms
            && runtime.is_watching()
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if runtime.verdict_stop_ms.take(stop_ms) {
            tracing::info!(task = %task_id, "no goal verdict came: the turn settles");
            self.settle_waited_turn(task_id).await;
        }
    }

    /// Settles a turn that waited for its verdict as the Stop would have, unless
    /// [`waited_turn_settles`] says it reads settled already.
    async fn settle_waited_turn(self: &Arc<Self>, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let achieved = self.runtime(task_id).goal_achieved.load(Ordering::SeqCst);
        if !waited_turn_settles(task.activity, achieved) {
            return;
        }
        if let Err(err) = self.settle_clean_turn(&task).await {
            tracing::warn!(task = %task_id, error = %err, "could not settle the turn");
        }
    }

    /// The person stopped the agent's turn (Esc, or Ctrl+C while it works; F 2026-10-10: "when I
    /// stop Claude, the task status should change"). Claude Code runs no Stop hook for it, so the
    /// transcript's interrupt line is the turn's end: a working (or approval-waiting) task waits
    /// for its person. A turn held for capacity that the person interrupted holds nothing any
    /// more: the task runs again.
    pub(crate) async fn turn_interrupted(
        self: &Arc<Self>,
        task_id: &TaskId,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        self.hold_overtaken(task_id, at, Activity::Waiting).await;
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running
            || !matches!(
                task.activity,
                Some(Activity::Working | Activity::NeedsAttention) | None
            )
        {
            return;
        }
        let now = self.now();
        self.runtime(task_id).verdict_stop_ms.clear();
        // Its prompt, read late, does not reopen the stopped turn.
        self.runtime(task_id)
            .last_stop_ms
            .store(at.map_or(now.0, |at| at.0.max(0)), Ordering::SeqCst);
        tracing::info!(task = %task_id, "the person interrupted the agent's turn");
        let _ = self
            .deps
            .store
            .set_activity(task_id, Some(Activity::Waiting), now)
            .await;
        self.publish_task(task_id).await;
        self.probe_idle_shells(task_id).await;
    }

    /// A transcript line written after a turn or prompt of `task_id` was held for capacity, or
    /// after a usage limit cut its turn (at `at`), shows the agent is not held any more: the
    /// hold ends and the task runs again with `activity`.
    pub(crate) async fn hold_overtaken(
        &self,
        task_id: &TaskId,
        at: Option<yulius_domain::time::Timestamp>,
        activity: Activity,
    ) {
        let Some(at) = at else { return };
        let Ok(Some(pause)) = self.deps.store.open_pause(task_id).await else {
            return;
        };
        let held = matches!(
            pause.kind,
            crate::model::PauseKind::Turn | crate::model::PauseKind::Prompt
        ) && pause.account_id.is_some();
        if (held || pause.kind == crate::model::PauseKind::Limit) && at > pause.started_at {
            self.hold_ended(task_id, "overtaken", activity).await;
        }
    }

    /// Agent work written after a usage limit cut the turn of `task_id` (at `at`) shows Claude
    /// Code continued that turn by itself at the reset (SPEC-072 LRS-001, #276): the `limit`
    /// pause ends and the task runs, so the resume loop never ends the turn it continued.
    pub(crate) async fn cut_turn_continued(
        &self,
        task_id: &TaskId,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        let Some(at) = at else { return };
        if let Ok(Some(pause)) = self.deps.store.open_pause(task_id).await
            && pause.kind == crate::model::PauseKind::Limit
            && at > pause.started_at
        {
            self.hold_ended(task_id, "overtaken", Activity::Working)
                .await;
        }
    }

    /// A prompt was submitted: the achieved goal no longer counts, and the turn that waited for a
    /// verdict goes on.
    pub(crate) fn prompt_submitted(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        runtime.goal_achieved.store(false, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
    }

    /// The agent produced work (an assistant line that is not a turn's end, at `at`): when the
    /// task does not say so, it is working (STA-003, STA-005, [`activity_on_work`]). Cheap while a
    /// turn is known to be in progress and no notice flags the task.
    pub(crate) async fn agent_worked(
        self: &Arc<Self>,
        task_id: &TaskId,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        let runtime = self.runtime(task_id);
        // Work after a Stop whose verdict is awaited: the session went on (a Stop hook continued
        // the turn), and the task, still Working, never read idle (BGW-005, STA-005).
        let waiting = runtime.verdict_stop_ms.pending();
        if at.is_some_and(|at| at.0 > waiting) && runtime.verdict_stop_ms.take(waiting) {
            runtime.resumed_turn.store(true, Ordering::SeqCst);
        }
        let notice_ms = runtime.notice_attention_ms.load(Ordering::SeqCst);
        if notice_ms == 0 && runtime.turn_in_progress() {
            return;
        }
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
        }
        let notice_ms = self.standing_notice_ms(&task).await;
        let last_stop_ms = runtime.last_stop_ms.load(Ordering::SeqCst);
        match activity_on_work(task.activity, at, last_stop_ms, notice_ms) {
            Some(next) => self.resume_working(&task, next, at).await,
            // Working already: the next lines need no look at the store.
            None if task.activity == Some(Activity::Working) => runtime.start_turn(self.now()),
            None => {}
        }
    }

    /// When the flag that the agent's work answers was raised (a notice's, or a goal pause's;
    /// `notice_attention_ms`), or 0 when there is none or `task` no longer reads that flag
    /// ([`noticed_flag_stands`]). A time that went stale is dropped, so that later work answers no
    /// other flag; a state that cannot be read answers nothing this time and drops nothing.
    pub(crate) async fn standing_notice_ms(&self, task: &TaskRecord) -> i64 {
        let runtime = self.runtime(&task.id);
        let noticed_ms = runtime.notice_attention_ms.load(Ordering::SeqCst);
        if noticed_ms == 0 {
            return 0;
        }
        let Ok(Some(times)) = self.deps.store.task_times(&task.id).await else {
            return 0;
        };
        if noticed_flag_stands(task.activity, times.state_at, noticed_ms) {
            return noticed_ms;
        }
        let _ = runtime.notice_attention_ms.compare_exchange(
            noticed_ms,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        0
    }

    /// The session goes on although the task read idle or flagged: it is `next` (Working).
    async fn resume_working(
        self: &Arc<Self>,
        task: &crate::model::TaskRecord,
        next: Activity,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        tracing::info!(task = %task.id, was = ?task.activity, "the agent works on: the task is working");
        let runtime = self.runtime(&task.id);
        runtime.background_agents.store(0, Ordering::SeqCst);
        runtime.resumed_turn.store(true, Ordering::SeqCst);
        runtime.turn_began_ms.store(self.now().0, Ordering::SeqCst);
        runtime.start_turn(self.now());
        let stretch = self.continue_working_stretch(&task.id).await;
        if let Err(err) = self.set_task_activity(task, next).await {
            tracing::warn!(task = %task.id, error = %err, "could not update the task's activity");
        }
        // It went on by itself from idle, by a line written at `at`: when that is after the
        // pull request watch typed, its news is being answered (SPEC-487 CHN-002).
        self.pr_news_turn_began(&task.id, at.map(|at| at.0));
        self.keep_working_stretch(&task.id, stretch).await;
    }

    /// Claude Code recorded the session goal's state.
    pub(crate) async fn goal_observed(
        self: &Arc<Self>,
        task_id: &TaskId,
        outcome: GoalOutcome,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        if !goal_stays_open(outcome) {
            self.goal_ended_during_retry(task_id, outcome == GoalOutcome::Met)
                .await;
        }
        let runtime = self.runtime(task_id);
        runtime
            .goal_open
            .store(goal_stays_open(outcome), Ordering::SeqCst);
        // The UI reads it beside an idle task (Answered or Your turn): a record that came after the
        // turn settled is published (SPEC-429 ANS-008).
        let unfinished = goal_unfinished(outcome);
        if runtime.goal_unfinished.swap(unfinished, Ordering::SeqCst) != unfinished {
            self.publish_task(task_id).await;
        }
        let verdict = matches!(
            outcome,
            GoalOutcome::Met | GoalOutcome::NotMet | GoalOutcome::Failed
        );
        // A verdict ends the turn's evaluation: a prompt sent before Claude Code recorded it is
        // over. The record's own time, not when the watcher read it: the next turn's prompt is
        // sent after it (SPEC-346 LPH-002).
        if verdict {
            let now = self.now().0;
            let recorded = at.map_or(now, |at| at.0.min(now));
            let _order = runtime.turn_order.lock().await;
            runtime.turn_end_ms.fetch_max(recorded, Ordering::SeqCst);
        }
        // The verdict on a clean Stop whose turn waited for it (BGW-005).
        let waited = verdict && runtime.verdict_stop_ms.take_any();
        if waited && outcome == GoalOutcome::NotMet {
            // The agent goes on, and the task never read idle.
            runtime.resumed_turn.store(true, Ordering::SeqCst);
        } else if outcome == GoalOutcome::NotMet
            && let Ok(Some(task)) = self.deps.store.task(task_id).await
            && task.status == TaskStatus::Running
            && let Some(next) = activity_on_goal_not_met(task.activity)
        {
            // The evaluator sent the agent back to work after Yulius's own Stop hook ended the
            // turn (STA-004).
            self.resume_working(&task, next, at).await;
        }
        let achieved = outcome == GoalOutcome::Met;
        runtime.goal_submitted.store(true, Ordering::SeqCst);
        runtime.goal_achieved.store(achieved, Ordering::SeqCst);
        if waited && outcome != GoalOutcome::NotMet {
            if achieved {
                tracing::info!(task = %task_id, "the agent achieved the task's goal");
            }
            self.settle_waited_turn(task_id).await;
        } else if achieved {
            tracing::info!(task = %task_id, "the agent achieved the task's goal");
            self.settle_idle(task_id, None).await;
        }
    }

    /// Applies a replayed transcript's final state once it is caught up.
    pub(crate) async fn apply_transcript_facts(
        self: &Arc<Self>,
        task_id: &TaskId,
        facts: TranscriptFacts,
    ) {
        let runtime = self.runtime(task_id);
        // The workflows left unfinished are the replay's (SPEC-486 WFL-003).
        runtime.with_workflows(|workflows| workflows.clone_from(&facts.workflows));
        if let Some(at) = facts.turn_closed_at {
            runtime.turn_closed_ms.fetch_max(at.0, Ordering::SeqCst);
        }
        if let Some(at) = facts.turn_moved_at {
            runtime.turn_moved_ms.fetch_max(at.0, Ordering::SeqCst);
        }
        if let Some(achieved) = facts.goal_achieved {
            runtime.goal_achieved.store(achieved, Ordering::SeqCst);
        }
        runtime.goal_open.store(facts.goal_open, Ordering::SeqCst);
        runtime
            .goal_unfinished
            .store(facts.goal_unfinished, Ordering::SeqCst);
        // A goal recorded while Yulius was down was taken: a restarted launch-goal watch ends.
        if facts.goal_recorded {
            runtime.goal_submitted.store(true, Ordering::SeqCst);
        }
        runtime
            .background_agents
            .store(facts.background_agents, Ordering::SeqCst);
        // What waits in Claude Code's queue is what the run's lines say (SPEC-471 CCQ-001):
        // known from here on.
        runtime.with_queue(|queue| *queue = facts.queue);
        runtime.queue_unread.store(false, Ordering::SeqCst);
        // The agent worked after Claude Code's last goal pause while nobody watched: the request
        // made for that pause is answered (SPEC-469 GPW-004).
        if facts.worked_since_goal_pause.is_some() {
            self.goal_pause_overtaken(task_id).await;
        }
        let Ok(Some(mut task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
        }
        // So is the flag that pause raised, as it is while Yulius watches (STA-003): the task
        // follows its session, Working while the turn is open, settled below once it closed.
        let flagged_ms = self.standing_notice_ms(&task).await;
        if task.activity == Some(Activity::NeedsAttention)
            && activity_on_work(task.activity, facts.worked_since_goal_pause, 0, flagged_ms)
                == Some(Activity::Working)
        {
            match self.set_task_activity(&task, Activity::Working).await {
                Ok(()) => task.activity = Some(Activity::Working),
                Err(err) => {
                    tracing::warn!(task = %task_id, error = %err, "could not follow a session past its goal pause");
                }
            }
        }
        runtime.goal_retry_armed.store(
            facts.goal_retry == Some(GoalRetryFact::Armed),
            Ordering::SeqCst,
        );
        match &facts.goal_retry {
            // Claude Code retries the closed turn by itself: a Working task stays Working.
            Some(GoalRetryFact::Armed) if facts.turn_open == Some(false) => {
                if task.activity == Some(Activity::Waiting)
                    && let Err(err) = self.set_task_activity(&task, Activity::Working).await
                {
                    tracing::warn!(task = %task_id, error = %err, "could not mark a goal retry");
                }
                return;
            }
            // It stopped retrying while Yulius was down: the task needs a person (once).
            Some(GoalRetryFact::Stopped(notice))
                if task.activity != Some(Activity::NeedsAttention) =>
            {
                self.goal_retries_spent(&task, notice).await;
                return;
            }
            Some(GoalRetryFact::Stopped(_)) => return,
            _ => {}
        }
        match facts.turn_open {
            // The turn ended (perhaps while Yulius was down and its Stop hook unanswered).
            Some(false) => self.settle_idle(task_id, Some(facts)).await,
            // A turn is open: this attempt just started, or Claude Code works. A replay never
            // sets Working; the next prompt or Stop hook says what happens.
            Some(true) if task.activity == Some(Activity::Working) => {
                let now = self.now();
                runtime.turn_began_ms.store(
                    facts.last_turn_at.map_or(now.0, |at| at.0),
                    Ordering::SeqCst,
                );
                runtime.start_turn(now);
            }
            _ => {}
        }
    }

    /// Makes an idle task's activity follow its goal (`waiting` ⇄ `done`) and refreshes its
    /// background shells. With `replay`, a task still `working` whose replayed turn ended becomes
    /// idle too, and `done` falls back to `waiting` only when the replay says the goal is not
    /// achieved.
    pub(crate) async fn settle_idle(
        self: &Arc<Self>,
        task_id: &TaskId,
        replay: Option<TranscriptFacts>,
    ) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
        }
        let achieved = self.runtime(task_id).goal_achieved.load(Ordering::SeqCst);
        let next = match (task.activity, replay) {
            (Some(Activity::Working), Some(_))
            | (Some(Activity::Waiting), _)
            | (Some(Activity::Done), None) => Some(Activity::after_clean_turn(achieved)),
            (Some(Activity::Done), Some(facts)) if facts.goal_achieved == Some(false) => {
                Some(Activity::Waiting)
            }
            _ => None,
        };
        if let Some(next) = next
            && task.activity != Some(next)
        {
            if task.activity == Some(Activity::Working) {
                self.runtime(task_id).end_turn(self.now());
            }
            let _ = self
                .deps
                .store
                .set_activity(task_id, Some(next), self.now())
                .await;
            self.publish_task(task_id).await;
        }
        self.probe_idle_shells(task_id).await;
        if next == Some(Activity::Done) {
            // Tasks chained after this one start once it is Done (TSK-012), and a Done task no
            // longer counts against the session limit (SES-101): a queued task can start now.
            // Not from this loop, which must go on reading the record that says what is still
            // pending.
            let (app, task_id) = (Arc::clone(self), task_id.clone());
            tokio::spawn(async move {
                app.advance_chains(&task_id).await;
                app.drain_queue().await;
            });
        }
    }

    /// Waits (bounded) until the record that closes the turn of the last clean Stop was seen:
    /// before it the task's background agents are not known, and a task with one pending is not
    /// done. No wait without a recent Stop (a replay, a hookless run), for a run that is not
    /// watched, or once the record came.
    pub(crate) async fn await_turn_record(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        // No record can arrive for a run that ended or is not watched: nothing to wait for.
        if !runtime.is_watching() {
            return;
        }
        let pending = || {
            turn_record_pending(
                runtime.last_stop_ms.load(Ordering::SeqCst),
                runtime.turn_record_ms.load(Ordering::SeqCst),
                self.now().0,
            )
        };
        while pending() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The record that closes a turn was seen (its background agents are applied already). A
    /// turn the verdict or later work had resumed and that ends with no Stop hook (an error ended
    /// it) leaves the task idle again, not Working for good.
    pub(crate) async fn turn_record_seen(
        self: &Arc<Self>,
        task_id: &TaskId,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        let runtime = self.runtime(task_id);
        let record_ms = self.now().0;
        runtime.turn_record_ms.store(record_ms, Ordering::SeqCst);
        runtime
            .turn_record_at_ms
            .store(at.map_or(0, |at| at.0), Ordering::SeqCst);
        // Claude Code retries the turn an error ended by itself (SPEC-333 AER-001): the task stays
        // Working until the retry's prompt starts the next turn.
        if runtime.goal_retry_armed.load(Ordering::SeqCst) {
            runtime.verdict_stop_ms.clear();
            runtime.resumed_turn.store(false, Ordering::SeqCst);
            runtime.end_turn(self.now());
            return;
        }
        // The turn closed with no verdict (the evaluator deferred or failed): it settles now.
        if runtime.verdict_stop_ms.take_any() {
            self.settle_waited_turn(task_id).await;
            return;
        }
        // The turn closed with no Stop Yulius answered (its hook failed open on an error answer
        // or an unreachable server, or an error ended the turn with no Stop hook): it settles as
        // that clean Stop would have (SPEC-290 GDW-001).
        let unanswered = closes_unanswered_turn(
            runtime.hooks_seen.load(Ordering::SeqCst),
            runtime.stop_answered_ms.load(Ordering::SeqCst),
            runtime.turn_began_ms.load(Ordering::SeqCst),
            runtime.stops_in_flight.load(Ordering::SeqCst),
            at,
        );
        if unanswered && self.settle_unanswered_turn(task_id, record_ms).await {
            runtime.resumed_turn.store(false, Ordering::SeqCst);
            return;
        }
        if !runtime.resumed_turn.swap(false, Ordering::SeqCst) {
            return;
        }
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let working =
            task.status == TaskStatus::Running && task.activity == Some(Activity::Working);
        if working {
            runtime.end_turn(self.now());
            let next = Activity::after_clean_turn(runtime.goal_achieved.load(Ordering::SeqCst));
            if let Err(err) = self.set_task_activity(&task, next).await {
                tracing::warn!(task = %task_id, error = %err, "could not settle the task");
            }
            self.probe_idle_shells(task_id).await;
        }
    }

    /// Settles the Working turn of `task_id` that closed with no Stop Yulius answered, as that
    /// clean Stop at `stop_ms` would have, unless [`unanswered_turn_settles`] says its own path
    /// decides. Whether it settled (SPEC-290 GDW-001).
    pub(crate) async fn settle_unanswered_turn(
        self: &Arc<Self>,
        task_id: &TaskId,
        stop_ms: i64,
    ) -> bool {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return false;
        };
        if task.status != TaskStatus::Running || task.activity != Some(Activity::Working) {
            return false;
        }
        let runtime = self.runtime(task_id);
        let context_clear = runtime
            .context
            .lock()
            .is_ok_and(|episode| matches!(*episode, crate::context_full::ContextEpisode::Clear));
        let pause_open = !matches!(self.deps.store.open_pause(task_id).await, Ok(None));
        if !unanswered_turn_settles(
            context_clear,
            runtime.at_limit.load(Ordering::SeqCst),
            pause_open,
        ) {
            return false;
        }
        tracing::warn!(task = %task_id, "the turn closed with no Stop Yulius answered: it settles");
        // The Stop it stands for came with its record: chains do not wait for another one, and a
        // prompt of the turn read late does not reopen it.
        runtime.last_stop_ms.store(stop_ms, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
        runtime.end_turn(self.now());
        if let Err(err) = self.settle_clean_turn(&task).await {
            tracing::warn!(task = %task_id, error = %err, "could not settle the task");
        }
        true
    }

    /// Probes the background shells of an idle task (bounded) and publishes the task when their
    /// number changed. Busy or ended tasks are not probed; their count is forgotten.
    pub(crate) async fn probe_idle_shells(self: &Arc<Self>, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        let idle = matches!(
            self.deps.store.task(task_id).await,
            Ok(Some(task)) if task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle)
        );
        let child = if idle {
            self.deps
                .store
                .live_attempt(task_id)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.child_pid)
        } else {
            None
        };
        let (count, waits, strays) = match child {
            Some(pid) => {
                let host = Arc::clone(&self.deps.session_host);
                let probe = tokio::task::spawn_blocking(move || host.background_probe(pid));
                match tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await {
                    Ok(Ok(Some(found))) => (
                        i64::from(found.shells),
                        i64::from(found.waits),
                        Some(found.strays),
                    ),
                    _ => {
                        tracing::debug!(task = %task_id, "the background-shell probe gave no answer");
                        (-1, -1, None)
                    }
                }
            }
            None => (-1, -1, None),
        };
        if let Some(strays) = strays {
            self.tend_strays(task_id, &runtime, strays).await;
        }
        // An agent waiting in `yulius wait` goes on by itself (SPEC-085 STT-002).
        let waits_before = runtime.background_waits.swap(waits, Ordering::SeqCst);
        // Background agents are no process of the session's shell tree: Claude Code counts them
        // in its turn record (STA-001), the transcript the ones it does not count (this turn's,
        // resumable ones, BGW-006). The count is the larger of the two views of the same
        // background work (a background Bash command is in both).
        let count = if idle {
            let agents = runtime.background_agents.load(Ordering::SeqCst);
            combine_background(count, self.live_launches_of(task_id).background(agents))
        } else {
            count
        };
        let previous = runtime.background_shells.swap(count, Ordering::SeqCst);
        // How long the idle agent's shells have been running, which the UI shows (SHL-002).
        let now = self.now().0;
        if count > 0 && previous <= 0 {
            runtime.shells_since_ms.store(now, Ordering::SeqCst);
        } else if count <= 0 {
            runtime.shells_since_ms.store(0, Ordering::SeqCst);
        }
        if previous != count || (waits_before > 0) != (waits > 0) {
            if count > 0 && previous != count {
                tracing::info!(task = %task_id, shells = count, "background shells still running");
            }
            self.publish_task(task_id).await;
            if previous > 0 && count == 0 {
                // The last shell ended: a done task is Done now, its chain may start and its slot
                // is free (SES-101, #305). Not from the watcher's loop: the chain waits for the
                // turn record.
                let (app, task_id) = (Arc::clone(self), task_id.clone());
                tokio::spawn(async move {
                    app.advance_chains(&task_id).await;
                    app.drain_queue().await;
                });
            }
        }
    }

    /// Logs an idle run's strays when first seen and reaps those seen for
    /// [`crate::context::AppConfig::stray_reap_after_ms`] (SPEC-353 LAP-004). Only an idle task's
    /// run is probed, so a working turn's processes are never reaped.
    async fn tend_strays(
        &self,
        task_id: &TaskId,
        runtime: &crate::supervisor::TaskRuntime,
        strays: Vec<StrayProcess>,
    ) {
        let (new, due) = {
            let mut seen = runtime
                .strays_seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sort_strays(
                &mut seen,
                strays,
                self.now().0,
                self.config.stray_reap_after_ms,
            )
        };
        if !new.is_empty() {
            tracing::warn!(
                task = %task_id,
                strays = new.len(),
                processes = new.iter().map(|s| s.processes.len()).sum::<usize>(),
                pids = ?first_pids(&new),
                "leftover processes in the run: no shell of the agent owns them; they are ended \
                 once the task has been idle with them for a while (docs/operations.md)"
            );
        }
        if due.is_empty() {
            return;
        }
        // Still idle now, after the probe: a turn that started meanwhile keeps its processes.
        let idle = matches!(
            self.deps.store.task(task_id).await,
            Ok(Some(task)) if task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle)
        );
        if !idle {
            return;
        }
        let strays = due.len();
        let pids = first_pids(&due);
        match self.reap_due_strays(due).await {
            Some(0) => {}
            Some(processes) => {
                tracing::warn!(task = %task_id, strays, processes, pids = ?pids, "reaped leftover processes");
            }
            None => {
                tracing::warn!(task = %task_id, strays, "the leftover processes were not reaped in time")
            }
        }
    }

    /// Kills `due` strays through the session host within [`SHELL_PROBE_TIMEOUT`]: how many
    /// processes were signalled, `None` when it did not answer in time.
    pub(crate) async fn reap_due_strays(&self, due: Vec<StrayProcess>) -> Option<usize> {
        let host = Arc::clone(&self.deps.session_host);
        let reap = tokio::task::spawn_blocking(move || host.reap_strays(&due));
        match tokio::time::timeout(SHELL_PROBE_TIMEOUT, reap).await {
            Ok(Ok(processes)) => Some(processes),
            _ => None,
        }
    }

    /// Claude Code ended a turn and said how many background agents it still waits for: a change
    /// settles the idle task's background work at once (STA-001).
    pub async fn background_agents_observed(self: &Arc<Self>, task_id: &TaskId, agents: u32) {
        let previous = self
            .runtime(task_id)
            .background_agents
            .swap(agents, Ordering::SeqCst);
        if previous != agents {
            self.probe_idle_shells(task_id).await;
        }
    }

    /// The transcript saw background work start (SPEC-079 BGW-002): a Monitor stream (until it
    /// ends or expires) or a background agent.
    pub(crate) async fn background_started(
        self: &Arc<Self>,
        task_id: &TaskId,
        id: String,
        agent: bool,
        expires_in_ms: Option<u64>,
        at: Option<yulius_domain::time::Timestamp>,
    ) {
        let now = self.now().0;
        let at_ms = at.map_or(now, |at| at.0);
        let launch = Launch {
            agent,
            seen_ms: now,
            expires_ms: expires_in_ms
                .map(|ms| at_ms.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX))),
            resumable: false,
        };
        if let Ok(mut launches) = self.runtime(task_id).launches.lock() {
            remember_launch(&mut launches, id, launch, now);
        }
        // Read after the turn settled (the same poll as its record): an idle task counts it now.
        self.probe_idle_shells(task_id).await;
    }

    /// A task notification said the background work `id` ended: an idle task's count follows.
    pub(crate) async fn background_ended(self: &Arc<Self>, task_id: &TaskId, id: &str) {
        let removed = self
            .runtime(task_id)
            .launches
            .lock()
            .is_ok_and(|mut launches| launches.remove(id).is_some());
        if removed {
            self.probe_idle_shells(task_id).await;
        }
    }

    /// A task notification said the background agent `id` stopped with background work of its
    /// own still running (BGW-006): it has not ended. It counts as the idle task's background work
    /// until the same id's final notification, or [`AGENT_LAUNCH_TTL_MS`] from now.
    pub(crate) async fn background_resumable(self: &Arc<Self>, task_id: &TaskId, id: String) {
        let now = self.now().0;
        if let Ok(mut launches) = self.runtime(task_id).launches.lock() {
            remember_resumable(&mut launches, id, now);
        }
        self.probe_idle_shells(task_id).await;
    }

    /// The task's live launches ([`live_launches`]).
    fn live_launches_of(&self, task_id: &TaskId) -> LiveLaunches {
        let Some(runtime) = self.existing_runtime(task_id) else {
            return LiveLaunches::default();
        };
        let record_ms = runtime.turn_record_ms.load(Ordering::SeqCst);
        runtime.launches.lock().map_or_else(
            |_| LiveLaunches::default(),
            |launches| live_launches(&launches, self.now().0, record_ms),
        )
    }

    /// Waits (bounded) until the transcript watcher read the lines written before now: two of its
    /// rounds begin, so one began and ended after this call. Nothing to wait for when no watcher
    /// runs (BGW-004).
    pub(crate) async fn await_transcript_read(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        let start = runtime.watch_polls.load(Ordering::SeqCst);
        let deadline = tokio::time::Instant::now() + TRANSCRIPT_READ_WAIT;
        while runtime.is_watching()
            && runtime.watch_polls.load(Ordering::SeqCst) < start.saturating_add(2)
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Whether the task's agent has background work running right now: a fresh probe of its
    /// shells, or background agents its last turn end said it waits for.
    pub(crate) async fn shells_running_now(&self, task_id: &TaskId) -> bool {
        self.background_work_now(task_id).await == Some(true)
    }

    /// Whether the task's last turn end waits for background agents, or its transcript names
    /// live Monitors or agents launched or resumable and not ended (BGW-004, BGW-006).
    fn background_agents_now(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id)
            .is_some_and(|runtime| runtime.background_agents.load(Ordering::SeqCst) > 0)
            || {
                let live = self.live_launches_of(task_id);
                live.monitors > 0 || live.agents_pending > 0
            }
    }

    /// Whether anything of the task's agent runs in the background right now: background agents
    /// its last turn end said it waits for, live Monitors, agents launched or resumable and not
    /// ended (BGW-004, BGW-006), or shells a fresh probe finds (`yulius wait` alone does not
    /// count, DSK-102). `None` when the probe gives no answer; a task without a live agent
    /// process has none.
    pub(crate) async fn background_work_now(&self, task_id: &TaskId) -> Option<bool> {
        if self.background_agents_now(task_id) {
            return Some(true);
        }
        let attempt = self.deps.store.live_attempt(task_id).await.ok()?;
        let Some(pid) = attempt.and_then(|attempt| attempt.child_pid) else {
            return Some(false);
        };
        let host = Arc::clone(&self.deps.session_host);
        let probe = tokio::task::spawn_blocking(move || host.background_shells(pid));
        match tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await {
            Ok(Ok(Some(n))) => Some(n > 0),
            _ => None,
        }
    }

    /// [`Self::background_work_now`], counting also a shell that only runs `yulius wait`: the
    /// agent waits there for Yulius's news (its checks, a deploy, an armed merge) and would never
    /// hear it if its session were restarted (SPEC-461 CCM-013). One look at the processes;
    /// `None` when it gives no answer.
    pub(crate) async fn background_or_wait_now(&self, task_id: &TaskId) -> Option<bool> {
        if self.background_agents_now(task_id) {
            return Some(true);
        }
        let attempt = self.deps.store.live_attempt(task_id).await.ok()?;
        let Some(pid) = attempt.and_then(|attempt| attempt.child_pid) else {
            return Some(false);
        };
        let host = Arc::clone(&self.deps.session_host);
        let probe = tokio::task::spawn_blocking(move || host.background_probe(pid));
        match tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await {
            Ok(Ok(Some(found))) => Some(found.shells > 0 || found.waits > 0),
            _ => None,
        }
    }

    /// Since when the idle task's background shells have been running, when they are.
    pub(crate) fn background_shells_since(&self, task_id: &TaskId) -> Option<i64> {
        let runtime = self.existing_runtime(task_id)?;
        let since = runtime.shells_since_ms.load(Ordering::SeqCst);
        (since > 0 && runtime.background_shells.load(Ordering::SeqCst) > 0).then_some(since)
    }

    /// Background shells reported with an idle task: `None` when not known.
    pub(crate) fn background_shells_of(&self, task_id: &TaskId) -> Option<u32> {
        let runtime = self.existing_runtime(task_id)?;
        u32::try_from(runtime.background_shells.load(Ordering::SeqCst)).ok()
    }

    /// The run ids of the dynamic workflows the task's session launched whose end was not read
    /// (SPEC-486 WFL-003). Public for tests, which wait until a launch was read.
    #[must_use]
    pub fn dynamic_workflows(&self, task_id: &TaskId) -> Vec<String> {
        self.existing_runtime(task_id)
            .map(|runtime| runtime.with_workflows(|workflows| workflows.runs()))
            .unwrap_or_default()
    }

    /// Whether the idle task's agent waits in `yulius wait` (its checks, its delivery or an armed
    /// merge), as last probed: it goes on by itself when Yulius has news (SPEC-085 STT-002).
    pub(crate) fn agent_waits_on_yulius(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id)
            .is_some_and(|runtime| runtime.background_waits.load(Ordering::SeqCst) > 0)
    }

    /// Whether Claude Code last recorded the task's goal as set or not met yet, as its transcript
    /// was read so far: the goal is open, and its session works on after a clean Stop. Public
    /// for tests, which hold a goal turn until the goal was read.
    #[must_use]
    pub fn goal_is_open(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id)
            .is_some_and(|runtime| runtime.goal_open.load(Ordering::SeqCst))
    }

    /// Whether Claude Code last recorded the task's goal as set, not met or failed (SPEC-429
    /// ANS-002).
    pub(crate) fn goal_unfinished(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id)
            .is_some_and(|runtime| runtime.goal_unfinished.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::time::Timestamp;
    use yulius_domain::usage::LimitScope;

    fn reduce(observations: &[Observation]) -> TranscriptFacts {
        let mut facts = TranscriptFacts::default();
        for o in observations {
            facts.fold(o);
        }
        facts
    }

    const PROMPT: Observation = Observation::UserTurn {
        at: Some(Timestamp(1)),
        wake: false,
    };

    const ENDED: Observation = Observation::TurnEnded {
        background_agents: None,
        at: None,
    };

    /// SPEC-471 CCQ-001, CCQ-003: the lines of a run say what waits in Claude Code's queue,
    /// in the order Claude Code 2.1.322 writes them: the prompt's `enqueue` while a turn runs;
    /// after the Stop that ends the turn the `dequeue`, the ended turn's `turn_duration`, and
    /// only then the prompt record of the turn that took the prompt.
    #[test]
    fn the_queue_is_folded_from_its_records() {
        use yulius_domain::agent_cli::QueueChange::{Added, HandedOn, Taken};
        let queued = |facts: &TranscriptFacts| (facts.queue.waiting(), facts.queue.holds());
        let answer = Observation::TurnComplete {
            at: Some(Timestamp(5)),
        };
        let work = Observation::AgentWork {
            at: Some(Timestamp(6)),
        };
        assert_eq!(queued(&reduce(&[PROMPT])), (0, false));
        let waiting = [PROMPT, Observation::Queue(Added, None), answer.clone()];
        assert_eq!(queued(&reduce(&waiting)), (1, true));
        // Handed on: the turn that ended closes, and nothing of the new one shows yet.
        let mut lines = waiting.to_vec();
        lines.extend([Observation::Queue(HandedOn, None), ENDED]);
        assert_eq!(
            queued(&reduce(&lines)),
            (0, true),
            "between the two records"
        );
        // The ended turn's own late lines are not the new turn.
        lines.extend([work, Observation::TurnInterrupted { at: None }]);
        assert_eq!(queued(&reduce(&lines)), (0, true));
        lines.push(PROMPT);
        assert_eq!(queued(&reduce(&lines)), (0, false), "its turn began");
        // Taken into the running turn at a tool result: no turn of its own follows.
        let taken = [
            PROMPT,
            Observation::Queue(Added, None),
            Observation::Queue(Taken, None),
        ];
        assert_eq!(queued(&reduce(&taken)), (0, false));
        // A prompt Claude Code scheduled for itself is handed on with a record that is read as
        // no prompt: the model's first line written after the hand-over shows its turn. The
        // ended turn's lines were written before it.
        let handed_on = Observation::Queue(HandedOn, Some(Timestamp(100)));
        let scheduled = |then: Observation| {
            reduce(&[
                Observation::Queue(Added, None),
                handed_on.clone(),
                ENDED,
                then,
            ])
        };
        let work_at = |ms| Observation::AgentWork {
            at: Some(Timestamp(ms)),
        };
        let answer_at = |ms| Observation::TurnComplete {
            at: Some(Timestamp(ms)),
        };
        assert_eq!(queued(&scheduled(work_at(99))), (0, true), "the old turn");
        assert_eq!(queued(&scheduled(work_at(101))), (0, false));
        assert_eq!(queued(&scheduled(answer_at(99))), (0, true), "the old turn");
        assert_eq!(queued(&scheduled(answer_at(101))), (0, false));
        let sub_agent = Observation::TurnComplete { at: None };
        assert_eq!(queued(&scheduled(sub_agent)), (0, true), "no time, no word");
        // Claude Code's own wake-up is a turn that took what was handed on.
        let woken = [
            Observation::Queue(Added, None),
            Observation::Queue(HandedOn, None),
            Observation::UserTurn {
                at: Some(Timestamp(9)),
                wake: true,
            },
        ];
        assert_eq!(queued(&reduce(&woken)), (0, false));
    }

    /// SPEC-479 SWQ-002: a reattached run's lines say what a turn's end left in Claude Code's
    /// queue. The closing record of a turn marks what still waits; a Stop a hook blocked wrote
    /// none, so a prompt that waits through it is not marked.
    #[test]
    fn what_a_turns_end_left_in_the_queue_is_folded() {
        use yulius_domain::agent_cli::QueueChange::{Added, HandedOn};
        let queued = |facts: &TranscriptFacts| {
            let queue = facts.queue;
            (queue.waiting(), queue.left_over(), queue.hands_on())
        };
        let answer = Observation::TurnComplete {
            at: Some(Timestamp(5)),
        };
        // A prompt waits while the turn's answer is written (and its Stop is blocked).
        let mut lines = vec![PROMPT, Observation::Queue(Added, None), answer.clone()];
        assert_eq!(queued(&reduce(&lines)), (1, 0, true));
        // The turn ends with the entry still there: it was left.
        lines.push(ENDED);
        assert_eq!(queued(&reduce(&lines)), (1, 1, false));
        // A prompt entered in the next turn is handed on at its end, before the closing record.
        lines.extend([PROMPT, Observation::Queue(Added, None), answer]);
        assert_eq!(queued(&reduce(&lines)), (2, 1, true));
        lines.extend([Observation::Queue(HandedOn, None), ENDED, PROMPT]);
        assert_eq!(queued(&reduce(&lines)), (1, 1, false));
        // An entry Claude Code hands on alone, and a prompt behind it: the closing record of
        // the end that handed the entry on finds the prompt waiting and marks nothing.
        let behind = [
            PROMPT,
            Observation::Queue(Added, None),
            Observation::Queue(Added, None),
            Observation::Queue(HandedOn, None),
            ENDED,
            PROMPT,
        ];
        assert_eq!(queued(&reduce(&behind)), (1, 0, true));
    }

    /// SPEC-358 MSB-007: the time of the last record closing a session's turn, its answer or its
    /// `turn_duration`; a prompt, an interrupt or a sub-agent's answer is not one.
    #[test]
    fn the_last_turn_close_is_folded() {
        let at = |ms| Some(Timestamp(ms));
        let facts = reduce(&[
            Observation::TurnComplete { at: at(10) },
            Observation::TurnEnded {
                background_agents: None,
                at: at(12),
            },
            Observation::TurnComplete { at: None },
            Observation::TurnInterrupted { at: at(30) },
            Observation::UserTurn {
                at: at(40),
                wake: false,
            },
        ]);
        assert_eq!(facts.turn_closed_at, Some(Timestamp(12)));
        assert_eq!(facts.turn_moved_at, Some(Timestamp(40)));
        // A Stop answered with a block, then the turn went on to a new answer: it moved since.
        let blocked = reduce(&[
            Observation::UserTurn {
                at: at(10),
                wake: false,
            },
            Observation::TurnComplete { at: at(20) },
            Observation::AgentWork { at: at(25) },
            Observation::TurnComplete { at: at(30) },
        ]);
        assert_eq!(blocked.turn_moved_at, Some(Timestamp(30)));
        assert_eq!(reduce(&[PROMPT]).turn_closed_at, None);
    }

    fn retry(armed: bool) -> Observation {
        Observation::GoalRetry {
            armed,
            notice: "Goal paused after 3 automatic retries · the API was unavailable".into(),
        }
    }

    /// SPEC-464 UCL-007: Claude Code answers the refusal of a model for its usage credits with
    /// "Goal paused · the request was rate limited". After a usage limit, or a refused
    /// subscription, of the same turn that pause is the limit's: the replay keeps the limit and
    /// asks nobody. With no limit before it, it is a pause nobody handles, as before; and a
    /// limit a later turn overtook does not hide that turn's pause.
    #[test]
    fn a_goal_pause_at_a_usage_limit_is_the_limits() {
        let paused = Observation::GoalRetry {
            armed: false,
            notice: "Goal paused · the request was rate limited".into(),
        };
        let limit = Observation::LimitReached {
            resets_at: None,
            message: "You're out of usage credits. /model to switch models.".into(),
            window_minutes: None,
            scope: LimitScope::Model(None),
            at: Some(Timestamp(2)),
        };
        let refused = Observation::AccessLost {
            message: "Your organization has disabled Claude subscription access".into(),
            at: Some(Timestamp(2)),
        };
        let notice = Some("Goal paused · the request was rate limited".to_owned());
        for cut in [limit.clone(), refused] {
            let facts = reduce(&[PROMPT, cut.clone(), paused.clone(), ENDED]);
            assert_eq!(facts.goal_retry, None, "{cut:?}");
            assert!(facts.limit_pending, "{cut:?}");
            // Kept beside the limit: it needs a person if Yulius cannot take the task on.
            assert_eq!(facts.limit_goal_pause, notice, "{cut:?}");
        }
        let alone = reduce(&[PROMPT, paused.clone(), ENDED]);
        assert_eq!(
            alone.goal_retry,
            Some(GoalRetryFact::Stopped(
                "Goal paused · the request was rate limited".into()
            ))
        );
        assert_eq!(alone.limit_goal_pause, None);
        let later = reduce(&[PROMPT, limit.clone(), ENDED, PROMPT, paused.clone(), ENDED]);
        assert!(later.goal_retry.is_some() && !later.limit_pending);
        assert_eq!(later.limit_goal_pause, None);
        // The next turn, or a new limit, ends a pause held at the limit before it.
        let work = Observation::AgentWork {
            at: Some(Timestamp(3)),
        };
        for next in [PROMPT, work.clone(), limit.clone()] {
            let facts = reduce(&[PROMPT, limit.clone(), paused.clone(), ENDED, next.clone()]);
            assert_eq!(facts.limit_goal_pause, None, "{next:?}");
        }
        // SPEC-469 for a request made at a limit (its goal pause's, or one for a limit nothing
        // says the end of, with or without a notice): work before the limit answers nothing,
        // work after it does.
        let earlier = Observation::AgentWork {
            at: Some(Timestamp(1)),
        };
        for cut in [vec![limit.clone(), paused.clone()], vec![limit.clone()]] {
            let mut read = vec![PROMPT, earlier.clone(), PROMPT];
            read.extend(cut);
            read.push(ENDED);
            assert_eq!(reduce(&read).worked_since_goal_pause, None, "{read:?}");
            read.push(work.clone());
            assert_eq!(
                reduce(&read).worked_since_goal_pause,
                Some(Timestamp(3)),
                "{read:?}"
            );
        }
    }

    /// SPEC-333: a replay keeps the retry Claude Code armed over the record that closes the error
    /// turn, and its spent retries with Claude Code's words, until a prompt, agent work or an
    /// interrupt comes after them; an armed retry also ends with the goal.
    #[test]
    fn a_replay_keeps_the_last_goal_retry_notice_until_the_next_turn() {
        let armed = reduce(&[PROMPT, retry(true), ENDED]);
        assert_eq!(
            (armed.turn_open, armed.goal_retry),
            (Some(false), Some(GoalRetryFact::Armed))
        );
        assert_eq!(
            reduce(&[PROMPT, retry(false), ENDED]).goal_retry,
            Some(GoalRetryFact::Stopped(
                "Goal paused after 3 automatic retries · the API was unavailable".into()
            ))
        );
        for ended in [GoalOutcome::Cleared, GoalOutcome::Met, GoalOutcome::Failed] {
            let facts = reduce(&[PROMPT, retry(true), ENDED, Observation::Goal(ended, None)]);
            assert_eq!(facts.goal_retry, None, "{ended:?}");
            // Spent retries stay: the request is the person's.
            let facts = reduce(&[PROMPT, retry(false), ENDED, Observation::Goal(ended, None)]);
            assert!(facts.goal_retry.is_some(), "{ended:?}");
        }
        let still = reduce(&[
            PROMPT,
            retry(true),
            ENDED,
            Observation::Goal(GoalOutcome::NotMet, None),
        ]);
        assert_eq!(still.goal_retry, Some(GoalRetryFact::Armed));
        let wake = Observation::UserTurn {
            at: Some(Timestamp(2)),
            wake: true,
        };
        let work = Observation::AgentWork {
            at: Some(Timestamp(2)),
        };
        let interrupt = Observation::TurnInterrupted {
            at: Some(Timestamp(2)),
        };
        for next in [wake, PROMPT, work, interrupt] {
            for armed in [true, false] {
                let facts = reduce(&[PROMPT, retry(armed), ENDED, next.clone()]);
                assert_eq!(facts.goal_retry, None, "{next:?} after {armed}");
            }
        }
        assert_eq!(reduce(&[PROMPT, ENDED]).goal_retry, None);
    }
    const WORK: Observation = Observation::AgentWork {
        at: Some(Timestamp(2)),
    };

    /// SPEC-469 GPW-004: a replay says when the session's agent first worked after Claude Code's
    /// last goal pause (or first worked at all, in a run with no pause: the pause was an earlier
    /// run's). Its own line or its answer is such work, and ends the pause's last word. A prompt
    /// alone, a sub-agent's answer (no time), work without a time and work before the pause are
    /// not; an armed retry is no pause.
    #[test]
    fn a_replay_says_when_the_agent_first_worked_after_the_last_goal_pause() {
        let wake = Observation::UserTurn {
            at: Some(Timestamp(2)),
            wake: true,
        };
        let work_at = |ms| Observation::AgentWork {
            at: Some(Timestamp(ms)),
        };
        let answer_at = |ms| Observation::TurnComplete {
            at: Some(Timestamp(ms)),
        };
        let sub_agent = Observation::TurnComplete { at: None };
        let worked = |observations: &[Observation]| reduce(observations).worked_since_goal_pause;
        for work in [work_at(3), answer_at(3)] {
            let woken = reduce(&[PROMPT, retry(false), ENDED, wake.clone(), work.clone()]);
            assert_eq!(
                woken.worked_since_goal_pause,
                Some(Timestamp(3)),
                "{work:?}"
            );
            // With no prompt seen either (a hidden continuation): the pause is not the last word.
            let hidden = reduce(&[PROMPT, retry(false), ENDED, work.clone()]);
            assert_eq!(
                (hidden.worked_since_goal_pause, hidden.goal_retry),
                (Some(Timestamp(3)), None),
                "{work:?}"
            );
            assert_eq!(
                worked(&[PROMPT, work.clone()]),
                Some(Timestamp(3)),
                "a run after the pause"
            );
            assert_eq!(
                worked(&[PROMPT, work.clone(), retry(false), ENDED]),
                None,
                "work before the pause"
            );
            assert_eq!(
                worked(&[PROMPT, work.clone(), retry(true), ENDED]),
                Some(Timestamp(3)),
                "an armed retry is no pause"
            );
        }
        // The first work after the pause, not the last: a flag raised later is not its own.
        assert_eq!(
            worked(&[
                PROMPT,
                work_at(2),
                retry(false),
                ENDED,
                work_at(5),
                answer_at(9)
            ]),
            Some(Timestamp(5))
        );
        assert_eq!(worked(&[PROMPT, retry(false), ENDED]), None);
        assert_eq!(
            worked(&[PROMPT, retry(false), ENDED, wake.clone(), ENDED]),
            None,
            "a woken turn the API refused again"
        );
        assert_eq!(worked(&[PROMPT, retry(false), ENDED, PROMPT]), None);
        assert_eq!(
            worked(&[PROMPT, retry(false), ENDED, wake.clone(), sub_agent]),
            None
        );
        let untimed = Observation::AgentWork { at: None };
        assert_eq!(worked(&[PROMPT, retry(false), ENDED, wake, untimed]), None);
        assert_eq!(worked(&[]), None);
        // An answer before the pause leaves the pause the last word (a failed goal check).
        let checked = reduce(&[PROMPT, answer_at(3), retry(false), ENDED]);
        assert!(matches!(
            checked.goal_retry,
            Some(GoalRetryFact::Stopped(_))
        ));
        // An armed retry is Claude Code's to end: the answer of another turn does not.
        let armed = reduce(&[PROMPT, retry(true), ENDED, answer_at(3)]);
        assert_eq!(armed.goal_retry, Some(GoalRetryFact::Armed));
    }

    /// SPEC-469 (review round 1): the time of a flag that work answers holds only while the task
    /// reads that flag: Needs you, since no later than the time remembered. A flag replaced by a
    /// status change, or one raised afterwards, is not answered by it.
    #[test]
    fn a_remembered_flag_stands_only_while_the_task_reads_it() {
        use Activity as A;
        assert!(noticed_flag_stands(
            Some(A::NeedsAttention),
            Timestamp(100),
            100
        ));
        assert!(noticed_flag_stands(
            Some(A::NeedsAttention),
            Timestamp(100),
            107
        ));
        assert!(
            !noticed_flag_stands(Some(A::NeedsAttention), Timestamp(200), 107),
            "flagged again later"
        );
        for replaced in [Some(A::Working), Some(A::Waiting), Some(A::Done), None] {
            assert!(
                !noticed_flag_stands(replaced, Timestamp(100), 107),
                "{replaced:?}"
            );
        }
    }

    /// STA-001: a replayed turn end that says background agents are pending keeps them, until the
    /// next prompt.
    #[test]
    fn a_replay_keeps_the_background_agents_of_the_last_turn_end() {
        let pending = Observation::TurnEnded {
            background_agents: Some(2),
            at: None,
        };
        let waiting = reduce(&[
            PROMPT,
            Observation::TurnComplete { at: None },
            pending.clone(),
        ]);
        assert_eq!(waiting.turn_open, Some(false));
        assert_eq!(waiting.background_agents, 2);
        // The wake-up is a user line: a new turn, nothing pending until it ends.
        let woken = reduce(&[PROMPT, pending.clone(), PROMPT]);
        assert_eq!((woken.turn_open, woken.background_agents), (Some(true), 0));
        let done = reduce(&[PROMPT, pending, PROMPT, ENDED]);
        assert_eq!((done.turn_open, done.background_agents), (Some(false), 0));
    }

    /// STA-005: a stop the hook continued (a goal not met, a notice) is no end of the turn:
    /// assistant work after the turn's last end line reopens it, however the replay ends.
    #[test]
    fn work_after_a_stop_the_hook_continued_keeps_the_turn_open() {
        let blocked = reduce(&[PROMPT, Observation::TurnComplete { at: None }, WORK]);
        assert_eq!(blocked.turn_open, Some(true));
        // The agent finishes the continued turn: closed again by its own end.
        let closed = reduce(&[
            PROMPT,
            Observation::TurnComplete { at: None },
            WORK,
            Observation::TurnComplete { at: None },
            ENDED,
        ]);
        assert_eq!(closed.turn_open, Some(false));
        // The person interrupted afterwards.
        let stopped = reduce(&[
            PROMPT,
            WORK,
            Observation::TurnInterrupted {
                at: Some(Timestamp(3)),
            },
        ]);
        assert_eq!(stopped.turn_open, Some(false));
    }

    /// SPEC-429 ANS-002: the goal's work is unfinished while Claude Code last recorded it set, not
    /// met or failed; a met or cleared goal is not, and a prompt typed after a met goal (a
    /// follow-up question) does not reopen it.
    #[test]
    fn the_last_goal_outcome_says_whether_its_work_is_unfinished() {
        for (outcome, unfinished) in [
            (GoalOutcome::Set, true),
            (GoalOutcome::NotMet, true),
            (GoalOutcome::Failed, true),
            (GoalOutcome::Met, false),
            (GoalOutcome::Cleared, false),
        ] {
            let facts = reduce(&[PROMPT, Observation::Goal(outcome, None), ENDED]);
            assert_eq!(facts.goal_unfinished, unfinished, "{outcome:?}");
        }
        let follow_up = reduce(&[
            Observation::Goal(GoalOutcome::Met, None),
            ENDED,
            PROMPT,
            ENDED,
        ]);
        assert!(!follow_up.goal_unfinished, "answered after a met goal");
        assert!(!reduce(&[PROMPT, ENDED]).goal_unfinished, "no goal");
    }

    /// TSK-014: a replay is reduced to its final state, in transcript order.
    #[test]
    fn a_replayed_transcript_reduces_to_its_final_state() {
        assert_eq!(reduce(&[]), TranscriptFacts::default());
        // The production case: goal set, prompt, end of turn, goal achieved, turn closed.
        let finished = reduce(&[
            Observation::Goal(GoalOutcome::Set, None),
            PROMPT,
            Observation::TurnComplete { at: None },
            Observation::Goal(GoalOutcome::Met, None),
            ENDED,
        ]);
        assert_eq!(
            finished,
            TranscriptFacts {
                turn_open: Some(false),
                goal_achieved: Some(true),
                background_agents: 0,
                context_full: false,
                limit_pending: false,
                last_turn_at: Some(Timestamp(1)),
                goal_open: false,
                goal_unfinished: false,
                goal_recorded: true,
                goal_retry: None,
                limit_goal_pause: None,
                worked_since_goal_pause: None,
                turn_closed_at: None,
                turn_moved_at: Some(Timestamp(1)),
                queue: yulius_domain::agent_cli::Queue::default(),
                workflows: crate::dynamic_workflow::Unfinished::default(),
            }
        );
        // Typed again after the goal: a new open turn, the goal no longer counts.
        let again = reduce(&[Observation::Goal(GoalOutcome::Met, None), ENDED, PROMPT]);
        assert_eq!(again.turn_open, Some(true));
        assert_eq!(again.goal_achieved, Some(false));
        // Not met yet: the agent continues.
        let continuing = reduce(&[
            PROMPT,
            Observation::TurnComplete { at: None },
            Observation::Goal(GoalOutcome::NotMet, None),
        ]);
        assert_eq!(continuing.turn_open, Some(true));
        // Impossible: the turn ends without an achieved goal.
        let failed = reduce(&[PROMPT, Observation::Goal(GoalOutcome::Failed, None)]);
        assert_eq!(failed.turn_open, Some(false));
        assert_eq!(failed.goal_achieved, Some(false));
        // A prompt-mode session never says anything about a goal.
        let plain = reduce(&[PROMPT, Observation::TurnComplete { at: None }]);
        assert_eq!(plain.goal_achieved, Some(false));
        assert!(!plain.goal_recorded);
        // SPEC-146 GWR-003: any record of a goal says it was taken.
        assert!(reduce(&[Observation::Goal(GoalOutcome::Cleared, None)]).goal_recorded);
        let limit = reduce(&[Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: LimitScope::Account,
            at: None,
        }]);
        // A limit changes no turn fact; it is only pending (SPEC-072 LRS-001).
        assert_eq!(
            limit,
            TranscriptFacts {
                limit_pending: true,
                ..TranscriptFacts::default()
            }
        );
    }

    /// STA-003, STA-005: assistant work reopens an idle task only when it is newer than the last
    /// stop, and takes back a notice's flag only when it is newer than the notice.
    #[test]
    fn work_after_the_stop_or_the_notice_makes_the_task_working() {
        use Activity as A;
        let at = |ms| Some(Timestamp(ms));
        // Idle tasks: a stop at 100 ms.
        for idle in [A::Waiting, A::Done] {
            assert_eq!(
                activity_on_work(Some(idle), at(101), 100, 0),
                Some(A::Working)
            );
            assert_eq!(
                activity_on_work(Some(idle), at(100), 100, 0),
                None,
                "same instant: old turn"
            );
            assert_eq!(
                activity_on_work(Some(idle), at(40), 100, 0),
                None,
                "read late"
            );
            assert_eq!(
                activity_on_work(Some(idle), None, 100, 0),
                None,
                "no time, no trust"
            );
        }
        // Flagged by a notice at 50 ms: the answer arrived.
        assert_eq!(
            activity_on_work(Some(A::NeedsAttention), at(60), 0, 50),
            Some(A::Working)
        );
        assert_eq!(
            activity_on_work(Some(A::NeedsAttention), at(50), 0, 50),
            None
        );
        // Flagged by anything else (no notice recorded): not the work's to take back.
        assert_eq!(
            activity_on_work(Some(A::NeedsAttention), at(60), 0, 0),
            None
        );
        // Already working, or not known: nothing to change.
        assert_eq!(activity_on_work(Some(A::Working), at(60), 0, 0), None);
        assert_eq!(activity_on_work(None, at(60), 0, 0), None);
    }

    /// STA-004: a goal "not met" makes an idle task Working; it leaves the others alone.
    #[test]
    fn a_goal_not_met_makes_an_idle_task_working() {
        use Activity as A;
        assert_eq!(activity_on_goal_not_met(Some(A::Waiting)), Some(A::Working));
        assert_eq!(activity_on_goal_not_met(Some(A::Done)), Some(A::Working));
        assert_eq!(activity_on_goal_not_met(Some(A::Working)), None);
        assert_eq!(activity_on_goal_not_met(Some(A::NeedsAttention)), None);
        assert_eq!(activity_on_goal_not_met(None), None);
    }

    /// STA-001 (review of SPEC-032): the idle count of background work keeps an unknown probe
    /// unknown unless agents are pending, and the chain waits only for a record still expected.
    #[test]
    fn background_work_and_the_turn_record_wait() {
        assert_eq!(combine_background(-1, 0), -1, "unknown stays unknown");
        assert_eq!(combine_background(0, 0), 0);
        assert_eq!(combine_background(-1, 2), 2);
        assert_eq!(combine_background(1, 2), 2);
        assert_eq!(combine_background(3, 2), 3);
        // A Stop at 1000 ms: the record is expected until it came or 3 s passed.
        assert!(turn_record_pending(1_000, 0, 1_200));
        assert!(!turn_record_pending(1_000, 1_100, 1_200), "the record came");
        assert!(!turn_record_pending(1_000, 0, 4_001), "gave up waiting");
        assert!(
            !turn_record_pending(0, 0, 10),
            "no Stop of this run (replay, hookless)"
        );
    }

    /// SPEC-079 BGW-003, BGW-004: Monitors count until they expire; agents launched after the last
    /// turn record count until that record (its count takes over); old launches go first.
    #[test]
    fn live_launches_follow_expiry_and_the_turn_record() {
        let mut launches = std::collections::BTreeMap::new();
        let monitor = |seen_ms, expires_ms| Launch {
            agent: false,
            seen_ms,
            expires_ms: Some(expires_ms),
            resumable: false,
        };
        let agent = |seen_ms| Launch {
            agent: true,
            seen_ms,
            expires_ms: None,
            resumable: false,
        };
        let live = |monitors, agents_this_turn, agents_pending| LiveLaunches {
            monitors,
            agents_this_turn,
            agents_pending,
            agents_resumable: 0,
        };
        remember_launch(&mut launches, "m1".into(), monitor(100, 1_000), 100);
        remember_launch(&mut launches, "a1".into(), agent(200), 200);
        assert_eq!(live_launches(&launches, 300, 150), live(1, 1, 1));
        // The turn record counts it now; a later turn (a wake-up) still waits for it to end.
        assert_eq!(live_launches(&launches, 300, 250), live(1, 0, 1));
        assert_eq!(
            live_launches(&launches, 1_000, 150),
            live(0, 1, 1),
            "expired"
        );
        assert_eq!(
            live_launches(&launches, 200 + AGENT_LAUNCH_TTL_MS, 250),
            live(0, 0, 0),
            "a lost notification holds nothing for good"
        );
        // An expired Monitor is forgotten when another launch is remembered.
        remember_launch(&mut launches, "m2".into(), monitor(1_100, 5_000), 1_100);
        assert!(!launches.contains_key("m1"));
        for n in 0..(MAX_LAUNCHES as i64 + 5) {
            remember_launch(&mut launches, format!("x{n}"), agent(2_000 + n), 2_000);
        }
        assert_eq!(launches.len(), MAX_LAUNCHES);
        assert!(!launches.contains_key("a1"), "the oldest went first");
        assert!(launches.contains_key(&format!("x{}", MAX_LAUNCHES as i64 + 4)));
    }

    /// SPEC-079 BGW-006, task #74: an agent that stopped with background work of its own still
    /// running counts as the idle task's background work, though the turn record after its
    /// notification counts no agent, until its final notification or the TTL from its last
    /// notification. It is not counted twice: not with this turn's agents, not on top of a turn
    /// record that counts it.
    #[test]
    fn a_resumable_agent_counts_until_its_final_notification() {
        let mut launches = std::collections::BTreeMap::new();
        let launch = Launch {
            agent: true,
            seen_ms: 200,
            expires_ms: None,
            resumable: false,
        };
        // 00:32:18 the review agent is launched; 00:32:20 the turn record counts it.
        remember_launch(&mut launches, "a1".into(), launch, 200);
        let at_launch = live_launches(&launches, 300, 250);
        assert_eq!(at_launch.background(1), 1);
        // 00:34:53 it stops with its own test shells running; 00:34:55 the turn record after
        // its notification counts no agent. The task's background work is still 1, not 0.
        remember_resumable(&mut launches, "a1".into(), 10_000);
        let resumable = live_launches(&launches, 10_200, 10_100);
        assert_eq!(
            resumable,
            LiveLaunches {
                monitors: 0,
                agents_this_turn: 0,
                agents_pending: 1,
                agents_resumable: 1,
            }
        );
        assert_eq!(resumable.background(0), 1);
        // A record that counts it once it resumed, or before its notification, is the same agent.
        assert_eq!(resumable.background(1), 1);
        // Read before the record: still one agent, not one of this turn and one resumable.
        assert_eq!(live_launches(&launches, 10_050, 5_000).background(0), 1);
        // With an agent launched after the record and a live Monitor.
        remember_launch(
            &mut launches,
            "m1".into(),
            Launch {
                agent: false,
                seen_ms: 10_300,
                expires_ms: Some(20_000),
                resumable: false,
            },
            10_300,
        );
        remember_launch(
            &mut launches,
            "a2".into(),
            Launch {
                seen_ms: 10_400,
                ..launch
            },
            10_400,
        );
        assert_eq!(live_launches(&launches, 10_500, 10_100).background(0), 3);
        launches.retain(|id, _| id == "a1");
        // The TTL counts from its notification, not from its launch.
        let ttl = |now_ms| live_launches(&launches, now_ms, 10_100).agents_resumable;
        assert_eq!(ttl(200 + AGENT_LAUNCH_TTL_MS), 1);
        assert_eq!(ttl(10_000 + AGENT_LAUNCH_TTL_MS), 0, "a lost notification");
        // An agent whose launch Yulius did not see (a restart) is remembered as resumable.
        remember_resumable(&mut launches, "a9".into(), 10_600);
        assert_eq!(live_launches(&launches, 10_700, 10_100).agents_resumable, 2);
        // 00:39:49 its final notification ends it (`background_ended` removes it).
        launches.remove("a1");
        launches.remove("a9");
        assert_eq!(
            live_launches(&launches, 10_800, 10_100),
            LiveLaunches::default()
        );
    }

    /// SPEC-079 BGW-005: one slot holds the Stop whose turn waits for its verdict; the first to
    /// take it settles the turn, a newer Stop replaces it and a prompt clears it.
    #[test]
    fn the_first_to_take_the_verdict_slot_settles_the_turn() {
        let slot = VerdictSlot::default();
        assert!(!slot.take_any(), "nothing waits");
        assert!(!slot.take(0));
        slot.open(100);
        // The verdict (or the turn record) takes it; the waiter's timeout then settles nothing.
        assert!(slot.take_any());
        assert!(!slot.take(100));
        // The waiter of an older Stop does not take a newer Stop's turn.
        slot.open(100);
        slot.open(200);
        assert!(!slot.take(100));
        assert_eq!(slot.pending(), 200);
        assert!(slot.take(200), "the newer Stop's waiter settles it");
        // A prompt clears it: nothing settles the turn that went on.
        slot.open(300);
        slot.clear();
        assert!(!slot.take(300) && !slot.take_any());
    }

    /// SPEC-079 BGW-001 (review): Claude Code's own wake-up (background work ended) keeps an
    /// achieved goal, also in a replay; a typed prompt does not.
    #[test]
    fn a_wake_up_keeps_an_achieved_goal() {
        let wake = Observation::UserTurn {
            at: Some(Timestamp(5)),
            wake: true,
        };
        let woken = reduce(&[Observation::Goal(GoalOutcome::Met, None), ENDED, wake]);
        assert_eq!(
            (woken.goal_achieved, woken.turn_open),
            (Some(true), Some(true))
        );
        let typed = reduce(&[Observation::Goal(GoalOutcome::Met, None), ENDED, PROMPT]);
        assert_eq!(typed.goal_achieved, Some(false));
    }

    /// SPEC-079 BGW-005: a goal is open from its set or "not met" record until it is achieved,
    /// failed or cleared, also in a replay.
    #[test]
    fn a_goal_is_open_until_it_is_decided() {
        use GoalOutcome as G;
        let open = |outcomes: &[G]| {
            reduce(
                &outcomes
                    .iter()
                    .map(|o| Observation::Goal(*o, None))
                    .collect::<Vec<_>>(),
            )
            .goal_open
        };
        assert!(open(&[G::Set]));
        assert!(open(&[G::Set, G::NotMet]));
        assert!(!open(&[G::Set, G::Met]));
        assert!(!open(&[G::Set, G::Failed]));
        assert!(!open(&[G::Set, G::Cleared]));
        assert!(open(&[G::Met, G::Set]), "a new goal");
        assert!(!open(&[]));
    }

    #[test]
    fn typed_prompts_are_told_from_wakeups() {
        assert!(is_typed_prompt(Some("more tests please")));
        assert!(is_typed_prompt(Some("/goal Ship it")));
        assert!(!is_typed_prompt(Some(
            "<task-notification>\n<task-id>b1</task-id>"
        )));
        assert!(!is_typed_prompt(Some("   ")));
        assert!(!is_typed_prompt(None));
    }

    /// SPEC-468 CCF-010: a prompt a person or Yulius submitted came through the prompt box and
    /// emptied it, a command that starts no work too; Claude Code's own wake-up did not.
    #[test]
    fn prompts_from_the_prompt_box_are_told_from_wakeups() {
        for typed in [
            "more tests please",
            "/goal Ship it",
            "/compact keep the plan",
        ] {
            assert!(from_prompt_box(Some(typed)), "{typed}");
        }
        assert!(!from_prompt_box(Some(
            "<task-notification>\n<task-id>b1</task-id>"
        )));
        assert!(!from_prompt_box(Some("   ")));
        assert!(!from_prompt_box(None), "a hook without its prompt");
    }

    /// SPEC-085 STT-005: `/compact` typed into the session (its `UserPromptSubmit`, should Claude
    /// Code run it) neither starts work nor takes back an achieved goal; a prompt typed after it
    /// does.
    #[test]
    fn the_compaction_command_starts_no_work() {
        for compact in ["/compact", "/compact keep the plan"] {
            assert!(!prompt_starts_work(Some(compact)), "{compact}");
            assert!(!is_typed_prompt(Some(compact)), "{compact}");
        }
        for prompt in [
            "close the issues once it merges",
            "/goal Ship it",
            "/compactor",
        ] {
            assert!(prompt_starts_work(Some(prompt)), "{prompt}");
            assert!(is_typed_prompt(Some(prompt)), "{prompt}");
        }
        assert!(prompt_starts_work(None), "a hook without its prompt");
    }

    /// SPEC-072 LRS-001: a replay whose last word is a usage limit leaves it pending; a prompt,
    /// agent work or a turn's end after it (Claude Code continued by itself) makes it history.
    #[test]
    fn a_replayed_limit_is_pending_until_a_turn_follows_it() {
        let limit = Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: LimitScope::Account,
            at: None,
        };
        assert!(reduce(&[PROMPT, limit.clone()]).limit_pending);
        assert!(reduce(&[PROMPT, limit.clone(), Observation::Compacted]).limit_pending);
        let ended = Observation::TurnComplete {
            at: Some(Timestamp(3)),
        };
        for after in [PROMPT, WORK, ended.clone()] {
            assert!(
                !reduce(&[PROMPT, limit.clone(), after.clone()]).limit_pending,
                "{after:?}"
            );
        }
        assert!(!reduce(&[PROMPT, WORK]).limit_pending);
        // A sub-agent's turn end (no time) is not the session's.
        let sidechain = Observation::TurnComplete { at: None };
        assert!(reduce(&[PROMPT, limit.clone(), sidechain]).limit_pending);
        // The last turn's time, for a pause overtaken while Yulius was down.
        let continued = reduce(&[PROMPT, limit, WORK, ended]);
        assert_eq!(continued.last_turn_at, Some(Timestamp(3)));
        assert_eq!(reduce(&[WORK, PROMPT]).last_turn_at, Some(Timestamp(2)));
    }

    /// CTX-007: a replay that ends in a full context (no reply or compaction after it) leaves the
    /// recovery pending and the turn closed; a later reply or compaction settles it.
    #[test]
    fn a_replay_ending_in_a_full_context_leaves_its_recovery_pending() {
        let full = reduce(&[
            PROMPT,
            Observation::ContextFull,
            PROMPT,
            Observation::ContextFull,
        ]);
        assert!(full.context_full);
        assert_eq!(full.turn_open, Some(false));
        let failed = reduce(&[PROMPT, Observation::ContextFull, Observation::CompactFailed]);
        assert!(failed.context_full);
        for after in [
            Observation::Compacted,
            Observation::TurnComplete { at: None },
            WORK,
        ] {
            let settled = reduce(&[PROMPT, Observation::ContextFull, after.clone()]);
            assert!(!settled.context_full, "{after:?}");
        }
        assert!(!reduce(&[PROMPT, Observation::TurnComplete { at: None }]).context_full);
    }

    /// SPEC-290 GDW-001: the record that closes a turn begun after the last answered Stop
    /// settles it; GDW-002: not when a Stop of the turn was answered (a block, a hold, a switch)
    /// or is being answered, not for a record read late after a newer prompt, not without hooks
    /// or the record's time.
    #[test]
    fn only_a_record_of_a_turn_with_no_answered_stop_settles_it() {
        let at = |ms| Some(Timestamp(ms));
        // Stop answered at 100, the turn began at 200, its record written at 300.
        assert!(closes_unanswered_turn(true, 100, 200, 0, at(300)));
        // Its Stop was answered (at 250), whatever Yulius decided.
        assert!(!closes_unanswered_turn(true, 250, 200, 0, at(300)));
        // A Stop of the run is being answered: the hook decides.
        assert!(!closes_unanswered_turn(true, 100, 200, 1, at(300)));
        // The record of the turn before, read after a newer prompt (began at 200).
        assert!(!closes_unanswered_turn(true, 100, 200, 0, at(150)));
        // No time on the record, no turn begun, or a run without hooks.
        assert!(!closes_unanswered_turn(true, 100, 200, 0, None));
        assert!(!closes_unanswered_turn(true, 0, 0, 0, at(300)));
        assert!(!closes_unanswered_turn(false, 100, 200, 0, at(300)));
    }

    /// SPEC-290 GDW-001: a full context being recovered, a usage limit or an open pause decide
    /// how the task goes on; the unanswered turn's record does not settle it then.
    #[test]
    fn recovery_a_limit_or_a_pause_keep_an_unanswered_turn_as_it_is() {
        assert!(unanswered_turn_settles(true, false, false));
        assert!(!unanswered_turn_settles(false, false, false));
        assert!(!unanswered_turn_settles(true, true, false));
        assert!(!unanswered_turn_settles(true, false, true));
    }

    /// SPEC-346 LPH-003 (#921): a turn that waited for its verdict settles while the task works or
    /// needs a person, and once the goal is met while it reads Your turn (its prompt hook came
    /// late, so it never read Working); a task Done, or Your turn with the goal not met, stays.
    #[test]
    fn a_waited_turn_settles_a_met_goal_also_from_your_turn() {
        for achieved in [false, true] {
            assert!(waited_turn_settles(Some(Activity::Working), achieved));
            assert!(waited_turn_settles(
                Some(Activity::NeedsAttention),
                achieved
            ));
            assert!(!waited_turn_settles(Some(Activity::Done), achieved));
            assert!(!waited_turn_settles(None, achieved));
        }
        assert!(waited_turn_settles(Some(Activity::Waiting), true));
        assert!(!waited_turn_settles(Some(Activity::Waiting), false));
    }

    /// SPEC-290 GDW-001: a Stop of the live run that failed while its turn's record came settles
    /// the turn; a record written before that turn began (the turn before's, read late), a turn
    /// not known to have begun, or another run's Stop do not.
    #[test]
    fn a_failed_stop_settles_only_after_its_own_turns_record() {
        assert!(failed_stop_settles(true, 300, 200));
        assert!(!failed_stop_settles(true, 150, 200));
        assert!(!failed_stop_settles(true, 0, 200));
        assert!(!failed_stop_settles(true, 300, 0));
        assert!(!failed_stop_settles(false, 300, 200));
    }

    /// SPEC-290 GDW-001: a Stop counts as in flight until its guard is dropped, also when its
    /// handling panics.
    #[test]
    fn a_stop_in_flight_is_counted_until_its_guard_drops() {
        let runtime = Arc::new(crate::supervisor::TaskRuntime::new());
        let guard = StopInFlight::enter(&runtime);
        assert_eq!(runtime.stops_in_flight.load(Ordering::SeqCst), 1);
        drop(guard);
        assert_eq!(runtime.stops_in_flight.load(Ordering::SeqCst), 0);
        let held = Arc::clone(&runtime);
        let panicked = std::thread::spawn(move || {
            let _guard = StopInFlight::enter(&held);
            panic!("the Stop's handling failed");
        })
        .join();
        assert!(panicked.is_err());
        assert_eq!(runtime.stops_in_flight.load(Ordering::SeqCst), 0);
    }
}
