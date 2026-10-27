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

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
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
