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

/// How often the background shells of an idle task are probed (ms).
pub const SHELL_PROBE_MS: i64 = 5_000;

/// Longest a shell probe may take before its result counts as unknown.
pub const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The final state of the transcript lines read so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    /// Claude Code recorded a goal in this session (any outcome): a launch `/goal` was taken
    /// (SPEC-146 GWR-003).
    pub goal_recorded: bool,
}

impl TranscriptFacts {
    /// Folds one observation, in transcript order.
    pub fn fold(&mut self, observation: &Observation) {
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
            }
            Observation::TurnComplete { at } => {
                self.turn_open = Some(false);
                self.context_full = false;
                // A sub-agent's turn end (no time) says nothing about the session's limit.
                self.limit_pending &= at.is_none();
            }
            Observation::TurnInterrupted { .. } => self.turn_open = Some(false),
            // The API rejected the prompt: the turn is over, without a Stop hook (#213).
            Observation::ContextFull => {
                self.turn_open = Some(false);
                self.context_full = true;
            }
            Observation::Compacted => self.context_full = false,
            Observation::Goal(outcome) => {
                self.goal_recorded = true;
                self.goal_open = goal_stays_open(*outcome);
                self.fold_goal(*outcome);
            }
            Observation::LimitReached { .. } => self.limit_pending = true,
            Observation::Session { .. }
            | Observation::CompactFailed
            | Observation::BackgroundStarted { .. }
            | Observation::BackgroundEnded { .. }
            | Observation::BackgroundResumable { .. } => {}
            // An assistant line that is not a turn's end after a recorded end: a Stop hook
            // continued the turn (STA-005).
            Observation::AgentWork { .. } => {
                self.turn_open = Some(true);
                self.context_full = false;
                self.limit_pending = false;
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
        let stop_ms = self.now().0;
        runtime.last_stop_ms.store(stop_ms, Ordering::SeqCst);
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        if runtime.goal_open.load(Ordering::SeqCst) && !runtime.goal_achieved.load(Ordering::SeqCst)
        {
            runtime.verdict_stop_ms.open(stop_ms);
            let (app, task_id) = (Arc::clone(self), task.id.clone());
            tokio::spawn(async move { app.await_goal_verdict(&task_id, stop_ms).await });
            return Ok(());
        }
        runtime.verdict_stop_ms.clear();
        self.settle_clean_turn(task).await
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

    /// Settles a turn that waited for its verdict as the Stop would have, unless it reads idle
    /// already (a late replay or a person settled it).
    async fn settle_waited_turn(self: &Arc<Self>, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if !matches!(
            task.activity,
            Some(Activity::Working | Activity::NeedsAttention)
        ) {
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
        let last_stop_ms = runtime.last_stop_ms.load(Ordering::SeqCst);
        match activity_on_work(task.activity, at, last_stop_ms, notice_ms) {
            Some(next) => self.resume_working(&task, next).await,
            // Working already: the next lines need no look at the store.
            None if task.activity == Some(Activity::Working) => runtime.start_turn(self.now()),
            None => {}
        }
    }

    /// The session goes on although the task read idle or flagged: it is `next` (Working).
    async fn resume_working(self: &Arc<Self>, task: &crate::model::TaskRecord, next: Activity) {
        tracing::info!(task = %task.id, was = ?task.activity, "the agent works on: the task is working");
        let runtime = self.runtime(&task.id);
        runtime.background_agents.store(0, Ordering::SeqCst);
        runtime.resumed_turn.store(true, Ordering::SeqCst);
        runtime.turn_began_ms.store(self.now().0, Ordering::SeqCst);
        runtime.start_turn(self.now());
        self.continue_working_stretch(&task.id).await;
        if let Err(err) = self.set_task_activity(task, next).await {
            tracing::warn!(task = %task.id, error = %err, "could not update the task's activity");
        }
    }

    /// Claude Code recorded the session goal's state.
    pub(crate) async fn goal_observed(self: &Arc<Self>, task_id: &TaskId, outcome: GoalOutcome) {
        let runtime = self.runtime(task_id);
        runtime
            .goal_open
            .store(goal_stays_open(outcome), Ordering::SeqCst);
        // The verdict on a clean Stop whose turn waited for it (BGW-005).
        let waited = matches!(
            outcome,
            GoalOutcome::Met | GoalOutcome::NotMet | GoalOutcome::Failed
        ) && runtime.verdict_stop_ms.take_any();
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
            self.resume_working(&task, next).await;
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
        if let Some(achieved) = facts.goal_achieved {
            runtime.goal_achieved.store(achieved, Ordering::SeqCst);
        }
        runtime.goal_open.store(facts.goal_open, Ordering::SeqCst);
        // A goal recorded while Yulius was down was taken: a restarted launch-goal watch ends.
        if facts.goal_recorded {
            runtime.goal_submitted.store(true, Ordering::SeqCst);
        }
        runtime
            .background_agents
            .store(facts.background_agents, Ordering::SeqCst);
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
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
        let (count, waits) = match child {
            Some(pid) => {
                let host = Arc::clone(&self.deps.session_host);
                let probe = tokio::task::spawn_blocking(move || host.background_probe(pid));
                match tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await {
                    Ok(Ok(Some(found))) => (i64::from(found.shells), i64::from(found.waits)),
                    _ => {
                        tracing::debug!(task = %task_id, "the background-shell probe gave no answer");
                        (-1, -1)
                    }
                }
            }
            None => (-1, -1),
        };
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

    /// Whether anything of the task's agent runs in the background right now: background agents
    /// its last turn end said it waits for, live Monitors, agents launched or resumable and not
    /// ended (BGW-004, BGW-006), or shells a fresh probe finds (`yulius wait` alone does not
    /// count, DSK-102). `None` when the probe gives no answer; a task without a live agent
    /// process has none.
    pub(crate) async fn background_work_now(&self, task_id: &TaskId) -> Option<bool> {
        if self
            .existing_runtime(task_id)
            .is_some_and(|runtime| runtime.background_agents.load(Ordering::SeqCst) > 0)
            || {
                let live = self.live_launches_of(task_id);
                live.monitors > 0 || live.agents_pending > 0
            }
        {
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

    /// Whether the idle task's agent waits in `yulius wait` (its checks, its delivery or an armed
    /// merge), as last probed: it goes on by itself when Yulius has news (SPEC-085 STT-002).
    pub(crate) fn agent_waits_on_yulius(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id)
            .is_some_and(|runtime| runtime.background_waits.load(Ordering::SeqCst) > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::time::Timestamp;

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
    const WORK: Observation = Observation::AgentWork {
        at: Some(Timestamp(2)),
    };

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

    /// TSK-014: a replay is reduced to its final state, in transcript order.
    #[test]
    fn a_replayed_transcript_reduces_to_its_final_state() {
        assert_eq!(reduce(&[]), TranscriptFacts::default());
        // The production case: goal set, prompt, end of turn, goal achieved, turn closed.
        let finished = reduce(&[
            Observation::Goal(GoalOutcome::Set),
            PROMPT,
            Observation::TurnComplete { at: None },
            Observation::Goal(GoalOutcome::Met),
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
                goal_recorded: true,
            }
        );
        // Typed again after the goal: a new open turn, the goal no longer counts.
        let again = reduce(&[Observation::Goal(GoalOutcome::Met), ENDED, PROMPT]);
        assert_eq!(again.turn_open, Some(true));
        assert_eq!(again.goal_achieved, Some(false));
        // Not met yet: the agent continues.
        let continuing = reduce(&[
            PROMPT,
            Observation::TurnComplete { at: None },
            Observation::Goal(GoalOutcome::NotMet),
        ]);
        assert_eq!(continuing.turn_open, Some(true));
        // Impossible: the turn ends without an achieved goal.
        let failed = reduce(&[PROMPT, Observation::Goal(GoalOutcome::Failed)]);
        assert_eq!(failed.turn_open, Some(false));
        assert_eq!(failed.goal_achieved, Some(false));
        // A prompt-mode session never says anything about a goal.
        let plain = reduce(&[PROMPT, Observation::TurnComplete { at: None }]);
        assert_eq!(plain.goal_achieved, Some(false));
        assert!(!plain.goal_recorded);
        // SPEC-146 GWR-003: any record of a goal says it was taken.
        assert!(reduce(&[Observation::Goal(GoalOutcome::Cleared)]).goal_recorded);
        let limit = reduce(&[Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
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
        let woken = reduce(&[Observation::Goal(GoalOutcome::Met), ENDED, wake]);
        assert_eq!(
            (woken.goal_achieved, woken.turn_open),
            (Some(true), Some(true))
        );
        let typed = reduce(&[Observation::Goal(GoalOutcome::Met), ENDED, PROMPT]);
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
                    .map(|o| Observation::Goal(*o))
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
