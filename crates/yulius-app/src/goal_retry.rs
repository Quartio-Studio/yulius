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
//!
//! The request made for such a pause says "the task does not go on by itself". It ends when that
//! stops being true (SPEC-469, ADR-0304): once the session's agent works again, whatever woke it
//! (Claude Code's own wake-up, a prompt Yulius typed, a session Yulius restarted, or no prompt
//! anyone sees). A prompt alone ends nothing: the API may refuse it again, and then the person
//! still has to act. The request Yulius makes for a usage limit nothing says the end of (SPEC-464
//! UCL-010) says the same and ends the same way. Both also end, sooner, when Yulius takes the
//! task on at the limit that cut its turn: it runs the session on past the limit, or pauses the
//! task and resumes it by itself.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;
use crate::moves::LIMIT_WITHOUT_END_ACTION;
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

/// The audit action of a request that ended, as when its agent resolves it.
const RESOLVED_ACTION: &str = "task.needs_you.resolved";

/// Why the request for a goal pause ended without a person.
const WORKS_AGAIN: &str = "the agent works again after Claude Code paused the goal";

/// Why the request for a usage limit nothing says the end of ended without a person.
const WORKS_PAST_LIMIT: &str = "the agent works again after the usage limit";

/// How Yulius takes a task on at the refusal that cut its session's turn: a usage limit, or a
/// subscription Claude refuses for good. Either way the task no longer waits for a message, so
/// a request that says it does ([`STALLED_ACTIONS`]) is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TakenOn {
    /// The session is run on past the refusal, on this subscription or another.
    Moved,
    /// The task waits for a subscription that can take it (a limit's end, or a sign-in when
    /// none is left, which Yulius then asks for), and the pool resumes it.
    Paused,
}

impl TakenOn {
    /// Why the request ended, for the audit log: what Yulius does at that moment (a session
    /// that is refused again where it was moved to is asked about again).
    fn why(self) -> &'static str {
        match self {
            Self::Moved => "Yulius takes the session on past the refusal that cut its turn",
            Self::Paused => {
                "Yulius waits for a subscription that can take the task and resumes the session then"
            }
        }
    }
}

/// The requests Yulius opens itself because a session does not go on by itself: for a goal
/// Claude Code stopped retrying, and for a usage limit nothing says the end of (SPEC-464
/// UCL-010). Each ends when that stops being true (ADR-0304).
const STALLED_ACTIONS: [&str; 2] = [RETRY_ACTION, LIMIT_WITHOUT_END_ACTION];

/// Whether the Needs you a task reads since `state_at` is the flag of the request asked at
/// `asked_ms` (one of [`STALLED_ACTIONS`]): the request is recorded first and the task flagged
/// next, so a state that began before the request was asked is an earlier flag, which the
/// request left as it was.
#[must_use]
fn flagged_by_the_pause(state_at: Timestamp, asked_ms: i64) -> bool {
    state_at.0 >= asked_ms
}

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
        } else if let Err(err) = self.flag_stalled(task).await {
            tracing::warn!(task = %task.id, error = %err, "could not flag a goal pause at a limit again");
        }
    }

    /// Flags `task` Needs you because its session does not go on by itself, for a request
    /// Yulius opened ([`STALLED_ACTIONS`]); called once the request is recorded. From here on
    /// the agent's work shows the session went on: it ends that request
    /// ([`Self::goal_pause_overtaken`]) and answers this flag, also when no prompt is seen to
    /// start that turn (STA-003). A task flagged before keeps that flag.
    ///
    /// # Errors
    /// The store's.
    pub(crate) async fn flag_stalled(&self, task: &TaskRecord) -> AppResult<()> {
        let runtime = self.runtime(&task.id);
        runtime.goal_pause_asked.store(true, Ordering::SeqCst);
        self.set_task_activity(task, Activity::NeedsAttention)
            .await?;
        if task.activity != Some(Activity::NeedsAttention) {
            runtime
                .notice_attention_ms
                .store(self.now().0.max(1), Ordering::SeqCst);
        }
        Ok(())
    }

    /// Yulius takes the task on at the limit that cut its session's turn, with nothing typed: it
    /// runs the session on past the limit, or pauses the task and resumes it by itself. A
    /// request that says the task does not go on by itself is answered, be it for a goal pause
    /// (the one written at that limit, SPEC-464 UCL-007, or an earlier one that the limit
    /// overtook) or for a limit nothing says the end of (UCL-010). Any other request stays.
    pub(crate) async fn end_limit_requests(&self, task_id: &TaskId, how: TakenOn) {
        match self.end_stalled_request(task_id).await {
            Ok(Some(_)) => self.stalled_request_resolved(task_id, how.why()).await,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "ending a limit's request failed");
            }
        }
    }

    /// Ends the request Yulius opened because the session did not go on by itself
    /// ([`STALLED_ACTIONS`]), while it is the open one; any other request stays (the agent's
    /// own, SPEC-134). It is ended by its action in one store statement, so a request made
    /// meanwhile is never the one ended. The action of the request it ended.
    ///
    /// # Errors
    /// The store's.
    async fn end_stalled_request(&self, task_id: &TaskId) -> AppResult<Option<&'static str>> {
        for action in STALLED_ACTIONS {
            if self
                .deps
                .store
                .end_task_needs_you_asking(task_id, action, self.now())
                .await?
            {
                self.publish_task(task_id).await;
                return Ok(Some(action));
            }
        }
        Ok(None)
    }

    /// Says in the audit log `why` a request Yulius opened ended without a person.
    async fn stalled_request_resolved(&self, task_id: &TaskId, why: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "system",
                RESOLVED_ACTION,
                Some(task_id.as_str()),
                Some(why),
            )
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not log a request's end");
        }
    }

    /// A run of `task_id` is watched from now on: a request Yulius opened because the session
    /// did not go on by itself ([`STALLED_ACTIONS`]) that is still open, made before this
    /// server started, is answered by the run's work too (SPEC-469 GPW-004). So is the flag
    /// raised with it (STA-003): a restart of Yulius forgot when it was raised, and a task that
    /// reads Needs you since the request was asked says it.
    pub(crate) async fn watch_goal_pause_request(&self, task_id: &TaskId) {
        let store = &self.deps.store;
        let Ok(Some(open)) = store.task_needs_you(task_id).await else {
            return;
        };
        if !STALLED_ACTIONS.contains(&open.action.as_str()) {
            return;
        }
        let runtime = self.runtime(task_id);
        runtime.goal_pause_asked.store(true, Ordering::SeqCst);
        if runtime.notice_attention_ms.load(Ordering::SeqCst) == 0
            && let Ok(Some(task)) = store.task(task_id).await
            && task.status == TaskStatus::Running
            && task.activity == Some(Activity::NeedsAttention)
            && let Ok(Some(times)) = store.task_times(task_id).await
            && flagged_by_the_pause(times.state_at, open.at)
        {
            runtime
                .notice_attention_ms
                .store(times.state_at.0.max(1), Ordering::SeqCst);
        }
    }

    /// The session's agent worked (the model's own line in its transcript) after Yulius asked a
    /// person because it did not go on by itself (Claude Code stopped retrying the goal, or a
    /// usage limit nothing says the end of): the task goes on by itself again, so that request
    /// is answered (SPEC-469 GPW-001, SPEC-464 UCL-010). Nothing without such a request, or
    /// once a person answered it. Whether it ended the request.
    pub(crate) async fn goal_pause_overtaken(&self, task_id: &TaskId) -> bool {
        let runtime = self.runtime(task_id);
        if !runtime.goal_pause_asked.swap(false, Ordering::SeqCst) {
            return false;
        }
        let ended = match self.end_stalled_request(task_id).await {
            Ok(Some(action)) => action,
            Ok(None) => return false,
            Err(err) => {
                // Asked again at the agent's next work.
                runtime.goal_pause_asked.store(true, Ordering::SeqCst);
                tracing::warn!(task = %task_id, error = %err, "ending a goal pause's request failed");
                return false;
            }
        };
        tracing::info!(task = %task_id, "the agent works again: the task needs nobody");
        let why = if ended == RETRY_ACTION {
            WORKS_AGAIN
        } else {
            WORKS_PAST_LIMIT
        };
        self.stalled_request_resolved(task_id, why).await;
        true
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
    /// happened, unless one is open already (the agent's own stays). The request ends when a
    /// person answers it, or when the agent works again ([`Self::goal_pause_overtaken`]).
    pub(crate) async fn goal_retries_spent(&self, task: &TaskRecord, notice: &str) {
        tracing::warn!(task = %task.id, notice, "Claude Code stopped retrying the goal: the task needs a person");
        // The turn is over: the record that closes it, or a verdict's wait, leaves it as it is.
        let runtime = self.runtime(&task.id);
        runtime.goal_retry_armed.store(false, Ordering::SeqCst);
        runtime.verdict_stop_ms.clear();
        runtime.resumed_turn.store(false, Ordering::SeqCst);
        runtime.end_turn();
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
        // From here on the agent's work shows the session went on (SPEC-469 GPW-001).
        if let Err(err) = self.flag_stalled(task).await {
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
    use super::*;

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

    /// SPEC-469 GPW-004: after a restart of Yulius the flag a goal pause raised is known by its
    /// time: at or after the request was asked. A task flagged before the pause keeps that flag.
    #[test]
    fn a_flag_raised_since_the_request_was_asked_is_the_pauses() {
        assert!(flagged_by_the_pause(Timestamp(1_000), 1_000));
        assert!(flagged_by_the_pause(Timestamp(1_007), 1_000));
        assert!(
            !flagged_by_the_pause(Timestamp(999), 1_000),
            "flagged before"
        );
    }
}
