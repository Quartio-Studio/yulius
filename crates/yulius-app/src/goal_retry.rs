//! A goal turn an error ended, as Claude Code's goal mode answers it (SPEC-333, ADR-0225).
//!
//! An API error ("Connection lost mid-response") ends the turn with no Stop hook. With a goal
//! active, Claude Code 2.1.292 retries the turn itself: it says so in a notice ("Goal still active
//! · … · retrying in 1 min (1/3)"), closes the turn and delivers a goal check-in prompt 1, 5 and 15
//! minutes later. Until that prompt (or a person's) starts the next turn the task reads Working:
//! nobody has to act. Once Claude Code gives the retries up ("Goal paused after 3 automatic
//! retries"), or pauses the goal on an API error it does not retry, nothing goes on by itself:
//! the task is in Needs you. Yulius types nothing itself, so no prompt of its own races Claude
//! Code's check-in.
//!
//! A pause that follows a usage limit in the same turn is not such a pause (SPEC-464): Claude
//! Code answers the refusal of a model for its usage credits with "Goal paused · the request was
//! rate limited", and the limit path already moves the task or resumes it at the reset. Its
//! notice is held (`LimitGoalPause`): when the move is refused before it ends the session, the
//! session stays at the refusal, nothing goes on by itself, and the pause needs a person after
//! all.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::model::TaskRecord;
use crate::needs_you::NeedsYouRecord;
use crate::supervisor::TaskRuntime;

/// What the person does when Claude Code stopped retrying.
const RETRY_ACTION: &str =
    "Send the task a message (for example \"continue\") to retry once the API works again";

/// Why it needs a person.
const STOPPED: &str = "The agent stopped and Claude Code does not retry any more: the task does not \
                       go on by itself.";

/// The audit action of a goal Claude Code stopped retrying.
const AUDIT_ACTION: &str = "task.goal_retries_spent";

/// What became of a goal pause Claude Code wrote at the run's usage limit (SPEC-464 UCL-007).
/// The notice and the limit path's refusal come in either order (the move runs beside the
/// transcript's reading), so each side leaves its word here for the other.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum LimitGoalPause {
    /// No pause was written at the run's limit.
    #[default]
    None,
    /// Its notice, held while Yulius moves the task past the limit or pauses it.
    Held(String),
    /// Yulius could not take the task on: its session stays at the limit, so a pause written
    /// there needs a person.
    Stays,
    /// Yulius asked a person about this limit itself (a limit nothing says the end of, UCL-010):
    /// a pause written there adds nothing, before or after.
    Asked,
}

impl LimitGoalPause {
    /// Claude Code's pause `notice` was read at the limit. `true` when it is held for the limit
    /// path; `false` when that path already said the session stays, so it needs a person now.
    pub(crate) fn hold(&mut self, notice: &str) -> bool {
        match self {
            Self::Stays => {
                *self = Self::None;
                false
            }
            Self::Asked => true,
            Self::None | Self::Held(_) => {
                *self = Self::Held(notice.to_owned());
                true
            }
        }
    }

    /// The move past the limit was refused: the notice held meanwhile, which needs a person
    /// now. Without one, a notice read later does.
    pub(crate) fn refused(&mut self) -> Option<String> {
        match std::mem::replace(self, Self::Stays) {
            Self::Held(notice) => {
                *self = Self::None;
                Some(notice)
            }
            Self::Asked => {
                *self = Self::Asked;
                None
            }
            Self::None | Self::Stays => None,
        }
    }

    /// Yulius asked a person about the limit itself: a pause held is dropped, and one read
    /// later is not asked about either.
    pub(crate) fn asked(&mut self) {
        *self = Self::Asked;
    }
}

impl App {
    /// Claude Code's goal mode answered a turn an error ended: a retry it `armed`, or its
    /// `notice` that it stopped (AER-001, AER-002).
    pub(crate) async fn goal_retry_observed(
        self: &Arc<Self>,
        task_id: &TaskId,
        armed: bool,
        notice: &str,
    ) {
        let runtime = self.runtime(task_id);
        runtime.goal_retry_armed.store(armed, Ordering::SeqCst);
        // Claude Code decided what follows the turn: no goal verdict comes for it (a failed goal
        // check is retried too), so nothing settles it as a verdict's wait would.
        runtime.verdict_stop_ms.clear();
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running {
            return;
        }
        if !armed {
            // A pause at a usage limit, or at a refused subscription, is the limit's: Yulius
            // moves the task past it or resumes it at the reset, and nobody has to send a
            // message (SPEC-464 UCL-007). Unless the session stays at its limit.
            if runtime.at_limit.load(Ordering::SeqCst) {
                if self.hold_limit_goal_pause(&runtime, notice) {
                    tracing::info!(task = %task_id, notice, "Claude Code paused the goal at a usage limit: the limit path goes on");
                } else {
                    self.limit_goal_pause_needs_a_person(&task, notice).await;
                }
                return;
            }
            self.goal_retries_spent(&task, notice).await;
            return;
        }
        tracing::info!(task = %task_id, notice, "Claude Code retries the goal turn an error ended");
        // A turn something settled already goes on: Claude Code continues it by itself.
        if task.activity == Some(Activity::Waiting)
            && let Err(err) = self.set_task_activity(&task, Activity::Working).await
        {
            tracing::warn!(task = %task_id, error = %err, "could not mark a goal retry");
        }
    }

    /// Holds the `notice` of a goal pause written at the run's limit for the limit path; `false`
    /// when that path already said the session stays there (the pause then needs a person).
    fn hold_limit_goal_pause(&self, runtime: &TaskRuntime, notice: &str) -> bool {
        // Never a lost request: a poisoned lock asks the person.
        runtime
            .limit_goal_pause
            .lock()
            .is_ok_and(|mut pause| pause.hold(notice))
    }

    /// A switch of `task` was refused before it ended the session. At a usage limit that leaves
    /// the session at Claude Code's refusal, where nothing goes on by itself: a goal pause
    /// written there needs a person after all, whichever of the two came first (SPEC-464
    /// UCL-007). A task that waits for capacity is tried again by the pool and asks nobody.
    pub(crate) async fn limit_move_refused(&self, task: &TaskRecord) {
        let runtime = self.runtime(&task.id);
        if task.status != TaskStatus::Running || !runtime.at_limit.load(Ordering::SeqCst) {
            return;
        }
        let held = match runtime.limit_goal_pause.lock() {
            Ok(mut pause) => pause.refused(),
            Err(_) => return,
        };
        if let Some(notice) = held {
            self.limit_goal_pause_needs_a_person(task, &notice).await;
        }
    }

    /// The goal pause written at the limit where `task`'s session stays needs a person: asked
    /// and audited once per limit. The audit is the record of that: an entry written at or after
    /// Claude Code recorded the limit says this limit was asked about (the request itself may
    /// be an earlier limit's, kept, or the agent's own). Reading the same pause again says
    /// nothing new then: a restart replays it, and the task may read otherwise meanwhile (the
    /// replay settles the closed turn as Your turn, which can land after the request), so it is
    /// only put back under Needs you. A refusal after a later prompt (Yulius's own, which ends
    /// no request) is a new limit.
    async fn limit_goal_pause_needs_a_person(&self, task: &TaskRecord, notice: &str) {
        let limit_at = self.runtime(&task.id).limit_at_ms.load(Ordering::SeqCst);
        // A limit Claude Code did not date cannot be told from an earlier one: it is asked about.
        let asked = limit_at > 0
            && matches!(
                self.deps
                    .store
                    .last_audit(task.id.as_str(), AUDIT_ACTION, Timestamp(limit_at))
                    .await,
                Ok(Some(_))
            );
        if !asked {
            self.goal_retries_spent(task, notice).await;
        } else if let Err(err) = self.set_task_activity(task, Activity::NeedsAttention).await {
            tracing::warn!(task = %task.id, error = %err, "could not flag a goal pause at a limit again");
        }
    }

    /// The session goes on past the limit that cut its turn, with nothing typed: a request made
    /// for that limit is answered, be it for the goal pause written there (SPEC-464 UCL-007) or
    /// for a limit nothing says the end of (UCL-010). Any other request stays.
    pub(crate) async fn end_limit_requests(&self, task_id: &TaskId) {
        if matches!(
            self.deps.store.task_needs_you(task_id).await,
            Ok(Some(open)) if [RETRY_ACTION, crate::moves::LIMIT_WITHOUT_END_ACTION]
                .contains(&open.action.as_str())
        ) {
            self.end_needs_you(task_id).await;
        }
    }

    /// The goal ended (cleared, achieved or failed) while a retry was armed: Claude Code drops its
    /// check-in without a word, so the closed turn settles now (Your turn, or Done).
    pub(crate) async fn goal_ended_during_retry(
        self: &Arc<Self>,
        task_id: &TaskId,
        achieved: bool,
    ) {
        let runtime = self.runtime(task_id);
        if !runtime.goal_retry_armed.swap(false, Ordering::SeqCst) || runtime.turn_in_progress() {
            return;
        }
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status != TaskStatus::Running || task.activity != Some(Activity::Working) {
            return;
        }
        tracing::info!(task = %task_id, "the goal ended while Claude Code's retry was armed: the turn settles");
        if let Err(err) = self
            .set_task_activity(&task, Activity::after_clean_turn(achieved))
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not settle a goal retry");
        }
    }

    /// Claude Code stopped retrying the goal: the task is in Needs you, with a request saying what
    /// happened, unless one is open already (the agent's own stays).
    pub(crate) async fn goal_retries_spent(&self, task: &TaskRecord, notice: &str) {
        tracing::warn!(task = %task.id, notice, "Claude Code stopped retrying the goal: the task needs a person");
        // The turn is over: the record that closes it, or a verdict's wait, leaves it as it is.
        let runtime = self.runtime(&task.id);
        runtime.goal_retry_armed.store(false, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        runtime.end_turn(self.now());
        let store = &self.deps.store;
        if matches!(store.task_needs_you(&task.id).await, Ok(None)) {
            let now = self.now();
            let record = NeedsYouRecord {
                action: RETRY_ACTION.to_owned(),
                happened: Some(format!("{notice}.")),
                problem: Some(STOPPED.to_owned()),
                decision: false,
                link: None,
                at: now.0,
            };
            if let Err(err) = store.set_task_needs_you(&task.id, Some(&record), now).await {
                tracing::warn!(task = %task.id, error = %err, "could not record a goal's spent retries");
            }
        }
        if let Err(err) = self.set_task_activity(task, Activity::NeedsAttention).await {
            tracing::warn!(task = %task.id, error = %err, "could not flag a goal's spent retries");
        }
        self.publish_task(&task.id).await;
        if let Err(err) = store
            .audit(
                self.now(),
                "system",
                AUDIT_ACTION,
                Some(task.id.as_str()),
                Some(notice),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not log a goal's spent retries");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LimitGoalPause;

    /// SPEC-464 UCL-007: the pause Claude Code writes at a usage limit and the limit path's
    /// refusal come in either order (the move runs beside the transcript's reading). Held, the
    /// pause asks nobody; once the move is refused it asks a person, exactly once, whichever
    /// came first.
    #[test]
    fn a_goal_pause_at_a_limit_asks_a_person_only_when_the_move_is_refused() {
        let notice = "Goal paused · the request was rate limited";
        // The notice first, then the refusal.
        let mut pause = LimitGoalPause::default();
        assert!(pause.hold(notice), "held while Yulius moves the task");
        assert_eq!(pause, LimitGoalPause::Held(notice.to_owned()));
        assert_eq!(pause.refused().as_deref(), Some(notice));
        assert_eq!(pause, LimitGoalPause::None, "told once");
        assert_eq!(pause.refused(), None);
        // The refusal first, then the notice.
        let mut pause = LimitGoalPause::default();
        assert_eq!(pause.refused(), None);
        assert_eq!(pause, LimitGoalPause::Stays);
        assert!(!pause.hold(notice), "the session stays: it needs a person");
        assert_eq!(pause, LimitGoalPause::None, "told once");
        // A move that goes through says nothing: the pause stays held and asks nobody.
        let mut pause = LimitGoalPause::default();
        assert!(pause.hold(notice));
        assert!(pause.hold(notice), "read again after a restart");
        // Yulius asked a person about the limit itself (UCL-010): the pause asks nobody, read
        // before that or after it, and a move refused later finds nothing held to ask with.
        let mut pause = LimitGoalPause::default();
        assert!(pause.hold(notice));
        pause.asked();
        assert_eq!(pause.refused(), None, "the notice read first is dropped");
        assert_eq!(pause, LimitGoalPause::Asked);
        let mut pause = LimitGoalPause::default();
        pause.asked();
        assert!(
            pause.hold(notice),
            "the notice read afterwards is not asked about"
        );
        assert_eq!(pause, LimitGoalPause::Asked, "nor held");
        assert_eq!(pause.refused(), None);
        assert_eq!(pause, LimitGoalPause::Asked);
    }
}
