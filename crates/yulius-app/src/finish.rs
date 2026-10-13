//! When a task is finished (SPEC-001 TSK-014, SPEC-002 §6.1 "Finish rule").
//!
//! A running task is `done` when its turn ended cleanly (the Stop hook did not block) and Claude
//! Code recorded the session's goal as achieved (`goal_status` with `met: true`) with no prompt
//! since. Claude Code writes that record after its Stop hooks, so the order is usually: Stop hook
//! → `waiting`, then the record → `done`; when the record comes first (Yulius' hook was slow or
//! blocked the turn), the next clean Stop makes the task `done`. Any prompt makes it `working`
//! again and the achieved goal no longer counts.
//!
//! Background shells the agent left running (Claude Code's "· 1 shell") are probed from the
//! process table while the task is idle — at the clean Stop, when the goal is achieved and every
//! [`SHELL_PROBE_MS`] — and reported with the task; the UI shows a done task with a shell still
//! running as Working ("1 background task still running"). A probe that cannot read the process
//! table, or takes longer than [`SHELL_PROBE_TIMEOUT`], reports nothing: the task is not held
//! back by it (Claude Code wakes the agent when a shell ends anyway). Background agents are no
//! process of that tree: Claude Code says how many it still waits for in the record that closes
//! the turn (`pendingBackgroundAgentCount`, SPEC-032), and the larger of the two counts is the
//! idle task's background work.
//!
//! After a restart the transcript of a reattached run is read again from its start; it is reduced
//! to its final state ([`TranscriptFacts`]) and applied once, so a replayed prompt cannot put a
//! finished task back to Working, and a goal achieved while Yulius was down is not lost.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, GoalOutcome, TaskStatus};

use crate::context::App;
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
}

impl TranscriptFacts {
    /// Folds one observation, in transcript order.
    pub fn fold(&mut self, observation: &Observation) {
        match observation {
            Observation::UserTurn { .. } => {
                self.turn_open = Some(true);
                self.goal_achieved = Some(false);
                self.background_agents = 0;
            }
            Observation::TurnEnded { background_agents } => {
                self.turn_open = Some(false);
                self.background_agents = background_agents.unwrap_or(0);
            }
            Observation::TurnComplete | Observation::TurnInterrupted { .. } => {
                self.turn_open = Some(false);
            }
            Observation::Goal(GoalOutcome::Met) => {
                self.goal_achieved = Some(true);
                self.turn_open = Some(false);
            }
            Observation::Goal(GoalOutcome::Failed) => {
                self.goal_achieved = Some(false);
                self.turn_open = Some(false);
            }
            Observation::Goal(GoalOutcome::NotMet) => {
                self.goal_achieved = Some(false);
                self.turn_open = Some(true);
            }
            Observation::Goal(GoalOutcome::Set | GoalOutcome::Cleared) => {
                self.goal_achieved = Some(false);
            }
            Observation::Session { .. } | Observation::LimitReached { .. } => {}
            // An assistant line that is not a turn's end after a recorded end: a Stop hook
            // continued the turn (STA-005).
            Observation::AgentWork { .. } => self.turn_open = Some(true),
        }
    }
}

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
/// Code generated itself (task notifications and other tagged prompts start with `<`).
#[must_use]
pub fn is_typed_prompt(prompt: Option<&str>) -> bool {
    prompt
        .map(str::trim_start)
        .is_some_and(|p| !p.is_empty() && !p.starts_with('<'))
}

impl App {
    /// The activity a clean Stop gives the task (TSK-014), remembering when the turn ended.
    pub(crate) fn activity_after_clean_turn(&self, task_id: &TaskId) -> Activity {
        let runtime = self.runtime(task_id);
        runtime.last_stop_ms.store(self.now().0, Ordering::SeqCst);
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        Activity::after_clean_turn(runtime.goal_achieved.load(Ordering::SeqCst))
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

    /// A transcript line written after a turn or prompt of `task_id` was held for capacity (at
    /// `at`) shows the agent is not held any more: the hold ends and the task runs again with
    /// `activity`.
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
        if matches!(
            pause.kind,
            crate::model::PauseKind::Turn | crate::model::PauseKind::Prompt
        ) && pause.account_id.is_some()
            && at > pause.started_at
        {
            self.hold_ended(task_id, "overtaken", activity).await;
        }
    }

    /// A prompt was submitted: the achieved goal no longer counts.
    pub(crate) fn prompt_submitted(&self, task_id: &TaskId) {
        self.runtime(task_id)
            .goal_achieved
            .store(false, Ordering::SeqCst);
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
        runtime.start_turn(self.now());
        if let Err(err) = self.set_task_activity(task, next).await {
            tracing::warn!(task = %task.id, error = %err, "could not update the task's activity");
        }
    }

    /// Claude Code recorded the session goal's state.
    pub(crate) async fn goal_observed(self: &Arc<Self>, task_id: &TaskId, outcome: GoalOutcome) {
        if outcome == GoalOutcome::NotMet
            && let Ok(Some(task)) = self.deps.store.task(task_id).await
            && task.status == TaskStatus::Running
            && let Some(next) = activity_on_goal_not_met(task.activity)
        {
            // The evaluator sent the agent back to work after Yulius's own Stop hook ended the
            // turn (STA-004).
            self.resume_working(&task, next).await;
        }
        let achieved = outcome == GoalOutcome::Met;
        self.runtime(task_id)
            .goal_submitted
            .store(true, Ordering::SeqCst);
        self.runtime(task_id)
            .goal_achieved
            .store(achieved, Ordering::SeqCst);
        if achieved {
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
                runtime.start_turn(self.now());
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
    pub(crate) async fn turn_record_seen(self: &Arc<Self>, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        runtime.turn_record_ms.store(self.now().0, Ordering::SeqCst);
        if !runtime.resumed_turn.swap(false, Ordering::SeqCst) {
            return;
        }
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status == TaskStatus::Running && task.activity == Some(Activity::Working) {
            runtime.end_turn(self.now());
            let next = Activity::after_clean_turn(runtime.goal_achieved.load(Ordering::SeqCst));
            if let Err(err) = self.set_task_activity(&task, next).await {
                tracing::warn!(task = %task_id, error = %err, "could not settle the task");
            }
            self.probe_idle_shells(task_id).await;
        }
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
        let count = match child {
            Some(pid) => {
                let host = Arc::clone(&self.deps.session_host);
                let probe = tokio::task::spawn_blocking(move || host.background_shells(pid));
                match tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await {
                    Ok(Ok(Some(n))) => i64::from(n),
                    _ => {
                        tracing::debug!(task = %task_id, "the background-shell probe gave no answer");
                        -1
                    }
                }
            }
            None => -1,
        };
        // Background agents are no process of the session's shell tree: Claude Code counts them
        // in its turn record (STA-001). The count is the larger of the two views of the same
        // background work (a background Bash command is in both).
        let count = if idle {
            combine_background(count, runtime.background_agents.load(Ordering::SeqCst))
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
        if previous != count {
            if count > 0 {
                tracing::info!(task = %task_id, shells = count, "background shells still running");
            }
            self.publish_task(task_id).await;
            if previous > 0 && count == 0 {
                // The last shell ended: a done task is Done now, and its chain may start (not
                // from the watcher's loop: the chain waits for the turn record).
                let (app, task_id) = (Arc::clone(self), task_id.clone());
                tokio::spawn(async move { app.advance_chains(&task_id).await });
            }
        }
    }

    /// Claude Code ended a turn and said how many background agents it still waits for: a change
    /// settles the idle task's background work at once (STA-001).
    pub(crate) async fn background_agents_observed(
        self: &Arc<Self>,
        task_id: &TaskId,
        agents: u32,
    ) {
        let previous = self
            .runtime(task_id)
            .background_agents
            .swap(agents, Ordering::SeqCst);
        if previous != agents {
            self.probe_idle_shells(task_id).await;
        }
    }

    /// Whether the task's agent has background work running right now: a fresh probe of its
    /// shells, or background agents its last turn end said it waits for.
    pub(crate) async fn shells_running_now(&self, task_id: &TaskId) -> bool {
        if self
            .runtime(task_id)
            .background_agents
            .load(Ordering::SeqCst)
            > 0
        {
            return true;
        }
        let Ok(Some(attempt)) = self.deps.store.live_attempt(task_id).await else {
            return false;
        };
        let Some(pid) = attempt.child_pid else {
            return false;
        };
        let host = Arc::clone(&self.deps.session_host);
        let probe = tokio::task::spawn_blocking(move || host.background_shells(pid));
        matches!(
            tokio::time::timeout(SHELL_PROBE_TIMEOUT, probe).await,
            Ok(Ok(Some(n))) if n > 0
        )
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
    };

    const ENDED: Observation = Observation::TurnEnded {
        background_agents: None,
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
        };
        let waiting = reduce(&[PROMPT, Observation::TurnComplete, pending.clone()]);
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
        let blocked = reduce(&[PROMPT, Observation::TurnComplete, WORK]);
        assert_eq!(blocked.turn_open, Some(true));
        // The agent finishes the continued turn: closed again by its own end.
        let closed = reduce(&[
            PROMPT,
            Observation::TurnComplete,
            WORK,
            Observation::TurnComplete,
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
            Observation::TurnComplete,
            Observation::Goal(GoalOutcome::Met),
            ENDED,
        ]);
        assert_eq!(
            finished,
            TranscriptFacts {
                turn_open: Some(false),
                goal_achieved: Some(true),
                background_agents: 0,
            }
        );
        // Typed again after the goal: a new open turn, the goal no longer counts.
        let again = reduce(&[Observation::Goal(GoalOutcome::Met), ENDED, PROMPT]);
        assert_eq!(again.turn_open, Some(true));
        assert_eq!(again.goal_achieved, Some(false));
        // Not met yet: the agent continues.
        let continuing = reduce(&[
            PROMPT,
            Observation::TurnComplete,
            Observation::Goal(GoalOutcome::NotMet),
        ]);
        assert_eq!(continuing.turn_open, Some(true));
        // Impossible: the turn ends without an achieved goal.
        let failed = reduce(&[PROMPT, Observation::Goal(GoalOutcome::Failed)]);
        assert_eq!(failed.turn_open, Some(false));
        assert_eq!(failed.goal_achieved, Some(false));
        // A prompt-mode session never says anything about a goal.
        let plain = reduce(&[PROMPT, Observation::TurnComplete]);
        assert_eq!(plain.goal_achieved, Some(false));
        let limit = reduce(&[Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
        }]);
        assert_eq!(limit, TranscriptFacts::default());
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
}
