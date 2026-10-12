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
//! running as Working ("1 background shell still running"). A probe that cannot read the process
//! table, or takes longer than [`SHELL_PROBE_TIMEOUT`], reports nothing: the task is not held
//! back by it (Claude Code wakes the agent when a shell ends anyway).
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
/// How long an idle agent's background shells run before Yulius tells it (once per episode).
const SHELLS_LONG_MS: i64 = 30 * 60 * 1000;

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
}

impl TranscriptFacts {
    /// Folds one observation, in transcript order.
    pub fn fold(&mut self, observation: &Observation) {
        match observation {
            Observation::UserTurn { .. } => {
                self.turn_open = Some(true);
                self.goal_achieved = Some(false);
            }
            Observation::TurnComplete
            | Observation::TurnEnded
            | Observation::TurnInterrupted { .. } => self.turn_open = Some(false),
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
        }
    }
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

    /// Claude Code recorded the session goal's state.
    pub(crate) async fn goal_observed(self: &Arc<Self>, task_id: &TaskId, outcome: GoalOutcome) {
        let achieved = outcome == GoalOutcome::Met;
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
            // Tasks chained after this one start once it is Done (TSK-012).
            self.advance_chains(task_id).await;
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
        let previous = runtime.background_shells.swap(count, Ordering::SeqCst);
        // How long the idle agent's shells have been running: told once after a while, since a
        // wait that can never end keeps the task "working" for hours (complaint 1).
        let now = self.now().0;
        if count > 0 && previous <= 0 {
            runtime.shells_since_ms.store(now, Ordering::SeqCst);
            runtime.shells_told.store(false, Ordering::SeqCst);
        } else if count <= 0 {
            runtime.shells_since_ms.store(0, Ordering::SeqCst);
        }
        let since = runtime.shells_since_ms.load(Ordering::SeqCst);
        if count > 0
            && since > 0
            && now - since >= SHELLS_LONG_MS
            && !runtime.shells_told.swap(true, Ordering::SeqCst)
            && let Ok(Some(task)) = self.deps.store.task(task_id).await
        {
            self.tell_task(
                &task,
                "Your background shells keep running while you are idle",
                format!(
                    "{count} background shell{} you started {} min ago still run while your turn \
                     has ended. If one waits for something that cannot happen (a `pgrep -f` loop, \
                     `tail -f`, a long `sleep`), stop it (KillShell) so the task does not look \
                     busy. To wait for checks, a merge or a notice, start `yulius wait` with \
                     run_in_background instead. A server you keep on purpose can stay.",
                    if count == 1 { "" } else { "s" },
                    (now - since) / 60_000
                ),
            )
            .await;
        }
        if previous != count {
            if count > 0 {
                tracing::info!(task = %task_id, shells = count, "background shells still running");
            }
            self.publish_task(task_id).await;
            if previous > 0 && count == 0 {
                // The last shell ended: a done task is Done now, and its chain may start.
                self.advance_chains(task_id).await;
            }
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
            Observation::TurnEnded,
        ]);
        assert_eq!(
            finished,
            TranscriptFacts {
                turn_open: Some(false),
                goal_achieved: Some(true)
            }
        );
        // Typed again after the goal: a new open turn, the goal no longer counts.
        let again = reduce(&[
            Observation::Goal(GoalOutcome::Met),
            Observation::TurnEnded,
            PROMPT,
        ]);
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
