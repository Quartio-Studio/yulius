//! Stopping and closing a run: the stop rounds, the close and its wait, the escalation that
//! ends a run that does not, and the shutdown request.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::task::{STOP_INTERRUPT_GRACE_MS, StopReason, TaskEvent, TaskStatus};

use super::{EndIntent, STOP_ROUNDS, SwitchEnd, TaskRuntime, replace_intent};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;

/// How long a closed run may take to be recorded as ended after the close escalation (SPEC-039).
const CLOSE_SETTLE_MS: u64 = 5_000;
/// How long asking a run to shut down may take: one frame on a local socket; a host that takes
/// longer is stuck, which the escalation handles (SPEC-065 END-005).
const SHUTDOWN_WRITE_BOUND: Duration = Duration::from_secs(2);
/// How often a close waiting for its run looks again without an update (SPEC-039).
const CLOSE_RECHECK: Duration = Duration::from_secs(1);

/// Asks the task's run, if one is attached, to shut down gracefully (interrupt, then TERM, then
/// kill on the host's side). Taking the run lock and writing are bounded together (SPEC-065
/// END-005): a stuck host must not hold the lock that ending the run needs.
pub(super) async fn request_shutdown(runtime: &TaskRuntime, selected: Option<&AttemptId>) {
    let asked = tokio::time::timeout(SHUTDOWN_WRITE_BOUND, async {
        if let Some(run) = runtime.run.lock().await.as_mut()
            && selected.is_none_or(|id| id == &run.attempt_id)
        {
            let _ = run
                .writer
                .shutdown(u32::try_from(STOP_INTERRUPT_GRACE_MS).unwrap_or(5000))
                .await;
        }
    })
    .await;
    if asked.is_err() {
        tracing::warn!("the run did not take the shutdown request in time; the escalation ends it");
    }
}

impl App {
    /// Closes a finished, running task (TSK-014): its session is shut down gracefully and the
    /// task ends `closed`, not stopped. It stays Running until the session is gone.
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn close_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        self.close_task_selected(task_id, None).await
    }

    pub(crate) async fn close_task_selected(
        self: &Arc<Self>,
        task_id: &TaskId,
        achieved: Option<(&TaskRecord, Option<&crate::delivery::DeliveryRecord>)>,
    ) -> AppResult<()> {
        let task = self
            .deps
            .store
            .task(task_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if task.status == TaskStatus::Parked {
            return self.close_parked(&task).await;
        }
        if task.status != TaskStatus::Running {
            return Err(AppError::Conflict(format!(
                "task is {}",
                task.status.as_str()
            )));
        }
        crate::race::reached(
            crate::race::Point::CloseDecided,
            Path::new(task_id.as_str()),
        );
        let closed = self.live_attempt_id(task_id).await;
        let runtime = self.runtime(task_id);
        let delivery = match achieved {
            Some((selected, record)) => Some(
                self.hold_live_delivery(selected, record)
                    .await
                    .ok_or_else(|| {
                        AppError::Conflict(
                            "The task’s delivery is still being verified or has just changed; \
                             try again shortly, or stop the task first."
                                .into(),
                        )
                    })?,
            ),
            None => None,
        };
        if let Ok(mut intent) = runtime.intent.lock() {
            replace_intent(&mut intent, Some(EndIntent::Close), SwitchEnd::Closed);
        }
        drop(delivery);
        self.arm_escalation(task_id, closed);
        self.end_person_pause(task_id).await;
        request_shutdown(&runtime, None).await;
        Ok(())
    }

    /// Closes a parked task (SPEC-512 PRK-001): it has no session to end, so it is `closed` at
    /// once, as a close leaves a running one, and tasks chained after it learn it.
    ///
    /// # Errors
    /// `Conflict` when it is no longer parked.
    async fn close_parked(self: &Arc<Self>, task: &TaskRecord) -> AppResult<()> {
        let closed = TaskStatus::Parked
            .apply(TaskEvent::CloseRequested)
            .map_err(|err| AppError::Conflict(err.to_string()))?;
        if !self
            .deps
            .store
            .transition_task(&task.id, Some(TaskStatus::Parked), closed, None, self.now())
            .await?
        {
            return Err(AppError::Conflict(
                "the task changed meanwhile; try again".into(),
            ));
        }
        self.publish_task(&task.id).await;
        self.advance_chains(&task.id).await;
        Ok(())
    }

    /// Closes a finished, running task like [`Self::close_task_now`] and waits until its run has
    /// ended, the task rests and the run's ending is fully recorded (SPEC-039, SPEC-065 END-003),
    /// so its records and directories can go. A run that already ended by itself is not a
    /// conflict. The close and the wait are bounded together by the close escalation, after which
    /// the run's host is terminated, plus `CLOSE_SETTLE_MS`; the wait follows the task's update
    /// events and looks again every `CLOSE_RECHECK`, on an interval other events do not reset.
    ///
    /// # Errors
    /// `Conflict` when the task was not closed and is neither resting nor stopping, when its
    /// run's host could not be terminated, or when the run is still there at the bound.
    pub async fn close_task_and_wait(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        self.close_selected_and_wait(task_id, None).await
    }

    pub(crate) async fn close_selected_and_wait(
        self: &Arc<Self>,
        task_id: &TaskId,
        achieved: Option<(&TaskRecord, Option<&crate::delivery::DeliveryRecord>)>,
    ) -> AppResult<()> {
        // Subscribed before closing, so the update that says the run ended is never missed.
        let mut events = self.events.subscribe();
        let runtime = self.runtime(task_id);
        let mut finalizing = runtime.finalizing.subscribe();
        let bound = Duration::from_millis(self.config.stop_escalation_ms + CLOSE_SETTLE_MS);
        let ended = tokio::time::timeout(bound, async {
            match self.close_task_selected(task_id, achieved).await {
                Ok(()) => {}
                // Not running any more: a task that rests or is being stopped can go once its
                // run has ended; any other (switching, starting) was not closed.
                Err(AppError::Conflict(why)) => {
                    let status = self.deps.store.task(task_id).await?.map(|t| t.status);
                    // Still running: the owner's delivery could not be held; say so.
                    if achieved.is_some() && status == Some(TaskStatus::Running) {
                        return Err(AppError::Conflict(why));
                    }
                    // Archive and delete both close this way: the message names neither.
                    if status.is_some_and(|s| !s.is_resting() && s != TaskStatus::Stopping) {
                        return Err(AppError::Conflict(
                            "the task is changing state; stop the task first".into(),
                        ));
                    }
                }
                Err(err) => return Err(err),
            }
            let mut recheck = tokio::time::interval_at(
                tokio::time::Instant::now() + CLOSE_RECHECK,
                CLOSE_RECHECK,
            );
            loop {
                let store = &self.deps.store;
                let rests = store
                    .task(task_id)
                    .await?
                    .is_none_or(|t| t.status.is_resting());
                let live = store.live_attempt(task_id).await?;
                if let Some(why) = live.as_ref().and_then(|a| runtime.end_failed_for(&a.id)) {
                    return Err(AppError::Conflict(format!(
                        "the task's session could not be ended: {why}"
                    )));
                }
                if rests && live.is_none() && *finalizing.borrow_and_update() == 0 {
                    return Ok::<bool, AppError>(true);
                }
                loop {
                    tokio::select! {
                        _ = recheck.tick() => break,
                        _ = finalizing.changed() => break,
                        event = events.recv() => match event {
                            Ok(envelope) => {
                                if matches!(&envelope.event, UiEvent::TaskUpdated { task }
                                    if task.id == task_id.as_str())
                                {
                                    break;
                                }
                            }
                            // Updates were missed: look again.
                            Err(broadcast::error::RecvError::Lagged(_)) => break,
                            Err(broadcast::error::RecvError::Closed) => return Ok(false),
                        },
                    }
                }
            }
        })
        .await;
        match ended {
            Ok(Ok(true)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Ok(Ok(false)) | Err(_) => Err(AppError::Conflict(
                "the task's session is still closing; try again".into(),
            )),
        }
    }

    /// The task's live attempt, if any; a store error is logged and means none is known.
    async fn live_attempt_id(&self, task_id: &TaskId) -> Option<AttemptId> {
        match self.deps.store.live_attempt(task_id).await {
            Ok(attempt) => attempt.map(|a| a.id),
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "the ended run is unknown; no escalation");
                None
            }
        }
    }

    /// Arms the escalation of a stop or close: if `attempt` is still the task's live run after
    /// `stop_escalation_ms`, its host is terminated (SPEC-065 END-001). Armed before the run lock
    /// is taken, so neither a blocked writer nor a cancelled caller leaves the run without it
    /// (END-005). Only that run: a task continued meanwhile has a new one.
    pub(super) fn arm_escalation(self: &Arc<Self>, task_id: &TaskId, attempt: Option<AttemptId>) {
        let Some(attempt) = attempt else {
            return;
        };
        if let Ok(mut failed) = self.runtime(task_id).end_failed.lock() {
            *failed = None;
        }
        let app = self.clone();
        let tid = task_id.clone();
        let wait = Duration::from_millis(self.config.stop_escalation_ms);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Ok(Some(live)) = app.deps.store.live_attempt(&tid).await
                && live.id == attempt
            {
                crate::race::reached(crate::race::Point::EndEscalating, Path::new(tid.as_str()));
                tracing::warn!(task = %tid, "run did not exit after escalation; terminating host");
                app.terminate_then_mark_lost(&tid, &live, StopReason::Lost)
                    .await;
            }
        });
    }

    /// Stops a task: graceful shutdown with escalation, then (if needed) termination and `lost`.
    /// A status change that loses to a concurrent one (a switch or a start completing) is made
    /// again from the status read anew, so the stop always takes effect (SPEC-250 STW-001).
    ///
    /// # Errors
    /// `Conflict` when the task is not live, or still changes after `STOP_ROUNDS` rounds.
    pub async fn stop_task_now(self: &Arc<Self>, task_id: &TaskId) -> AppResult<()> {
        for round in 0..STOP_ROUNDS {
            if self.stop_round(task_id, round > 0).await? {
                return Ok(());
            }
        }
        // The stop failed: its intent goes, so no run ends later as if stopped.
        if let Ok(mut intent) = self.runtime(task_id).intent.lock()
            && matches!(*intent, Some(EndIntent::Cancel))
        {
            *intent = None;
        }
        Err(AppError::Conflict(
            "the task kept changing while it was stopped; try again".into(),
        ))
    }

    /// One round of [`Self::stop_task_now`]: `false` when its status change lost to a concurrent
    /// one. After a lost round (`again`), a task that rests was ended meanwhile, by this stop's
    /// intent or by its run's own end: the stop is done (STW-002).
    pub(super) async fn stop_round(
        self: &Arc<Self>,
        task_id: &TaskId,
        again: bool,
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        if again && task.status.is_resting() {
            // A run still live is ended all the same: the intent set is this stop's.
            if store.live_attempt(task_id).await?.is_some() {
                self.end_stopped_run(task_id).await;
            }
            return Ok(true);
        }
        // A resting task with a run still on record (SPEC-458 RUN-003, #439): the stop ends that
        // run instead of answering "task is …" and leaving it. One no server holds is settled as
        // after a restart (ADR-0050).
        if task.status.is_resting()
            && let Some(run) = store.live_attempt(task_id).await?
        {
            if !self.holds_run(task_id).await {
                self.settle_unheld_run(task_id, &run.id).await?;
                return Ok(true);
            }
            if let Ok(mut intent) = self.runtime(task_id).intent.lock() {
                replace_intent(&mut intent, Some(EndIntent::Cancel), SwitchEnd::Stopped);
            }
            self.end_stopped_run(task_id).await;
            return Ok(true);
        }
        // A parked task has no run either (SPEC-512 PRK-001).
        if task.status == TaskStatus::Queued
            || task.status == TaskStatus::Chained
            || task.status == TaskStatus::Prepared
            || task.status == TaskStatus::Parked
            || task.status == TaskStatus::WaitingCapacity
                && store.live_attempt(task_id).await?.is_none()
        {
            if !store
                .transition_task(
                    task_id,
                    Some(task.status),
                    TaskStatus::Stopped(StopReason::Cancelled),
                    None,
                    self.now(),
                )
                .await?
            {
                return Ok(false);
            }
            self.end_pause(task_id, "cancelled").await;
            self.publish_task(task_id).await;
            self.advance_chains(task_id).await;
            // A cancelled supertask no longer holds its project (SUP-005): the queue may go on.
            // Any other task that waited frees no session, so it starts nothing (a pass here
            // would start the next waiting task of a bulk stop before the stop reaches it).
            if task.supertask {
                self.drain_queue().await;
            }
            return Ok(true);
        }
        // A task already stopping is stopped again: its escalation is armed anew (SPEC-065). One
        // whose run has ended gets no intent (nothing would consume it): it rests by itself while
        // its ending is recorded, or now when nothing records it (records cut by a crash).
        let next = if task.status == TaskStatus::Stopping {
            if store.live_attempt(task_id).await?.is_none() {
                if *self.runtime(task_id).finalizing.borrow() == 0
                    && store
                        .transition_task(
                            task_id,
                            Some(TaskStatus::Stopping),
                            TaskStatus::Stopped(StopReason::Cancelled),
                            None,
                            self.now(),
                        )
                        .await?
                {
                    // An earlier stop's intent that no run consumed goes with it (STP-002).
                    if let Ok(mut intent) = self.runtime(task_id).intent.lock() {
                        *intent = None;
                    }
                    self.end_pause(task_id, "cancelled").await;
                    self.publish_task(task_id).await;
                    self.advance_chains(task_id).await;
                }
                return Ok(true);
            }
            TaskStatus::Stopping
        } else {
            task.status
                .apply(TaskEvent::CancelRequested)
                .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?
        };
        let runtime = self.runtime(task_id);
        if let Ok(mut intent) = runtime.intent.lock() {
            replace_intent(&mut intent, Some(EndIntent::Cancel), SwitchEnd::Stopped);
        }
        crate::race::reached(crate::race::Point::StopDecided, Path::new(task_id.as_str()));
        // Lost to a concurrent change: the task is read again. The intent stays: it is this
        // stop's, and a run that ends on it meanwhile rests its task (STW-003).
        if task.status != TaskStatus::Stopping
            && !store
                .transition_task(task_id, Some(task.status), next, None, self.now())
                .await?
        {
            return Ok(false);
        }
        self.end_pause(task_id, "cancelled").await;
        self.publish_task(task_id).await;
        self.end_stopped_run(task_id).await;
        Ok(true)
    }

    /// Ends the run of a task a stop decided to end: graceful shutdown, with the escalation armed.
    async fn end_stopped_run(self: &Arc<Self>, task_id: &TaskId) {
        // The run this stop ends; without it (a store error) nothing is escalated.
        let stopped = self.live_attempt_id(task_id).await;
        self.arm_escalation(task_id, stopped);
        self.end_person_pause(task_id).await;
        request_shutdown(&self.runtime(task_id), None).await;
    }
}
