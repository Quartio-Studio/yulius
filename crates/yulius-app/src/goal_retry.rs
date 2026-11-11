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
//! The request made for such a pause says "the task does not go on by itself". It ends when that
//! stops being true (SPEC-469, ADR-0304): once the session's agent works again, whatever woke it
//! (Claude Code's own wake-up, a prompt Yulius typed, a session Yulius restarted, or no prompt
//! anyone sees). A prompt alone ends nothing: the API may refuse it again, and then the person
//! still has to act.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::error::AppResult;
use crate::model::TaskRecord;
use crate::needs_you::NeedsYouRecord;

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

    /// Ends the request Yulius opened for a goal Claude Code stopped retrying, while it is the
    /// open one; any other request stays (the agent's own, SPEC-134). Whether it ended one.
    ///
    /// # Errors
    /// The store's.
    async fn end_goal_retry_request(&self, task_id: &TaskId) -> AppResult<bool> {
        let ended = self
            .deps
            .store
            .end_task_needs_you_asking(task_id, RETRY_ACTION, self.now())
            .await?;
        if ended {
            self.publish_task(task_id).await;
        }
        Ok(ended)
    }

    /// A run of `task_id` is watched from now on: a request for a goal pause that is still open,
    /// made before this server started, is answered by the run's work too (SPEC-469 GPW-004).
    pub(crate) async fn watch_goal_pause_request(&self, task_id: &TaskId) {
        if matches!(
            self.deps.store.task_needs_you(task_id).await,
            Ok(Some(open)) if open.action == RETRY_ACTION
        ) {
            self.runtime(task_id)
                .goal_pause_asked
                .store(true, Ordering::SeqCst);
        }
    }

    /// The session's agent worked (the model's own line in its transcript) after Claude Code
    /// stopped retrying the goal: the task goes on by itself again, so the request to send it a
    /// message is answered (SPEC-469 GPW-001). Nothing without such a pause, or once a person
    /// answered it. Whether it ended the request.
    pub(crate) async fn goal_pause_overtaken(&self, task_id: &TaskId) -> bool {
        let runtime = self.runtime(task_id);
        if !runtime.goal_pause_asked.swap(false, Ordering::SeqCst) {
            return false;
        }
        match self.end_goal_retry_request(task_id).await {
            Ok(true) => {}
            Ok(false) => return false,
            Err(err) => {
                // Asked again at the agent's next work.
                runtime.goal_pause_asked.store(true, Ordering::SeqCst);
                tracing::warn!(task = %task_id, error = %err, "ending a goal pause's request failed");
                return false;
            }
        }
        tracing::info!(task = %task_id, "the agent works again after a goal pause: the task needs nobody");
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "system",
                RESOLVED_ACTION,
                Some(task_id.as_str()),
                Some(WORKS_AGAIN),
            )
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not log a goal pause's end");
        }
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
        runtime.end_turn(self.now());
        // From here on the agent's work shows the session went on (SPEC-469 GPW-001).
        runtime.goal_pause_asked.store(true, Ordering::SeqCst);
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
        } else if task.activity != Some(Activity::NeedsAttention) {
            // The agent's work after this flag shows the session went on, also when no prompt is
            // seen to start that turn (STA-003). A task flagged before keeps that flag.
            runtime
                .notice_attention_ms
                .store(self.now().0.max(1), Ordering::SeqCst);
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
