//! Parking a session: ending an idle task's run under the launch lock and settling the task
//! as parked once its end is recorded. When to park is `crate::parking`'s.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::task::{TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;

use super::{EndIntent, STOP_ROUNDS, request_shutdown};
use crate::context::App;
use crate::error::{AppError, AppResult};

impl App {
    /// A close, stop or loss can settle the run that originally committed to park.
    pub(super) fn clear_settled_park(&self, task_id: &TaskId, attempt_id: &AttemptId) {
        if let Some(runtime) = self.existing_runtime(task_id)
            && let Ok(mut kept) = runtime.parked_activity.lock()
        {
            kept.take_if(|(run, _)| run == attempt_id);
        }
    }

    /// The task whose parked run ended (SPEC-512 PRK-003): a running or switching one is
    /// `parked` with its activity and audited `task.parked`; one a stop or a capacity pause took
    /// meanwhile rests as that run's end leaves it.
    pub(super) async fn settle_parked(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        now: Timestamp,
    ) {
        let attempt = self.deps.store.attempt(attempt_id).await.ok().flatten();
        let decided = self.existing_runtime(task_id).and_then(|runtime| {
            runtime
                .parked_activity
                .lock()
                .ok()?
                .as_ref()
                .filter(|(run, _)| run == attempt_id)
                .map(|(_, activity)| *activity)
        });
        for _ in 0..STOP_ROUNDS {
            let Ok(Some(task)) = self.deps.store.task(task_id).await else {
                return;
            };
            crate::race::reached(crate::race::Point::EndDecided, Path::new(task_id.as_str()));
            // A switch the park overtook gave up: the task parks from `Switching` too. A stop
            // that landed meanwhile rests it stopped, and a capacity pause closed (SPEC-250
            // STW-005: an ended run never leaves its task live).
            let parked = task.status.apply(TaskEvent::Parked).or_else(|_| {
                task.status
                    .apply(TaskEvent::SwitchCompleted)
                    .and_then(|running| running.apply(TaskEvent::Parked))
            });
            // A switch's status change cleared the activity: the one the park was decided on.
            let (to, activity) = match parked {
                Ok(parked) => (parked, task.activity.or(decided)),
                Err(_) => match task.status.apply(TaskEvent::Exited { normal: true }) {
                    Ok(rests) => (rests, None),
                    Err(_) => {
                        if task.status.is_resting() {
                            self.clear_settled_park(task_id, attempt_id);
                        }
                        return;
                    }
                },
            };
            // The goal's state is in the run's directory before anyone reads the task parked.
            if to == TaskStatus::Parked {
                self.mark_parked_goal(task_id).await;
            }
            match self
                .deps
                .store
                .transition_task(task_id, Some(task.status), to, activity, now)
                .await
            {
                Ok(false) => {}
                Ok(true) => {
                    self.clear_settled_park(task_id, attempt_id);
                    if to == TaskStatus::Parked {
                        crate::race::reached(
                            crate::race::Point::ParkStateRecorded,
                            Path::new(task_id.as_str()),
                        );
                        if let Some(run) = &attempt
                            && let Some((cols, rows, screen)) =
                                crate::parking::publish_parked_screen(Path::new(&run.run_dir))
                        {
                            let hub = &self.runtime(task_id).hub;
                            hub.restore(
                                hub.output_seq().max(1),
                                &screen,
                                cols,
                                rows,
                                crate::hub::ScreenUpdate::Park,
                            );
                        }
                        crate::parking::audit_parked(self, &task).await;
                    }
                    return;
                }
                Err(err) => {
                    tracing::warn!(task = %task_id, error = %err, "the parked run's task was not moved");
                    return;
                }
            }
        }
        tracing::warn!(task = %task_id, "the parked run's task kept changing; left as it is");
    }

    /// Leaves in the parked run's directory whether its goal was unfinished, as the session's
    /// runtime knew it ([`crate::parking::GOAL_UNFINISHED_MARK`]); a failed write is logged.
    async fn mark_parked_goal(&self, task_id: &TaskId) {
        let unfinished = self.goal_unfinished(task_id);
        let Ok(runs) = self.deps.store.attempts(task_id).await else {
            return;
        };
        if let Some(run) = runs.last() {
            let mark = Path::new(&run.run_dir).join(crate::parking::GOAL_UNFINISHED_MARK);
            let written = if unfinished {
                std::fs::write(&mark, b"")
            } else {
                std::fs::remove_file(&mark).or_else(|err| match err.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(err),
                })
            };
            if let Err(err) = written {
                tracing::warn!(task = %task_id, error = %err, "the parked goal was not marked");
            }
        }
    }

    /// Parks the idle session of a running task (SPEC-512 PRK-003) when
    /// [`App::parkable_now`] still says so under the launch lock, so no start, turn or message
    /// that came meanwhile is overtaken: its session ends as a close ends it, and the run's end
    /// makes the task `parked`. `false` when it was not parked.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn park_task_now(
        self: &Arc<Self>,
        task_id: &TaskId,
        rule: crate::parking::ParkRule,
    ) -> AppResult<bool> {
        self.park_task_because(task_id, rule, None).await
    }

    /// [`Self::park_task_now`]; the parked screen's notice says `because` instead of how long
    /// the session was idle (a rebalance off a nearly used-up subscription, SPEC-629 RBL-006).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn park_task_because(
        self: &Arc<Self>,
        task_id: &TaskId,
        rule: crate::parking::ParkRule,
        because: Option<&str>,
    ) -> AppResult<bool> {
        let Some(candidate) = self.deps.store.task(task_id).await? else {
            return Ok(false);
        };
        let delivery = self.deps.store.delivery(task_id).await?;
        let before = self.deps.store.latest_merge_wait(task_id).await?;
        crate::race::reached(
            crate::race::Point::ParkVerifying,
            Path::new(task_id.as_str()),
        );
        // Neither a slow PR read nor the fresh process probe should hold up another launch.
        // Probe last so background work started during the PR read keeps its session.
        let Some(prs) = self
            .verify_parking_prs(
                &candidate,
                delivery.as_ref(),
                crate::delivery::PrVerification::WatchPending,
            )
            .await?
        else {
            return Ok(false);
        };
        crate::race::reached(
            crate::race::Point::ParkPrsVerified,
            Path::new(task_id.as_str()),
        );
        if self.background_or_wait_now(task_id).await != Some(false) {
            return Ok(false);
        }
        crate::race::reached(crate::race::Point::ParkProbed, Path::new(task_id.as_str()));
        let _guard = self.supervisor.launch.lock().await;
        let Some(task) = self.deps.store.task(task_id).await? else {
            return Ok(false);
        };
        if task.branch != candidate.branch || task.worktree_path != candidate.worktree_path {
            return Ok(false);
        }
        let Some(quiet_ms) = self.parkable_now(&task, rule).await else {
            return Ok(false);
        };
        // A prompt waiting in Claude Code's queue would be lost with the session (SPEC-471).
        if self
            .existing_runtime(task_id)
            .is_none_or(|runtime| runtime.queue_may_hold())
        {
            return Ok(false);
        }
        let Some(run) = self.deps.store.live_attempt(task_id).await? else {
            return Ok(false);
        };
        if run.harness == Harness::Codex && run.provider_session_id.is_none() {
            return Ok(false);
        }
        crate::race::reached(
            crate::race::Point::ParkPrepared,
            Path::new(task_id.as_str()),
        );
        if self.deps.store.delivery(task_id).await? != delivery {
            return Ok(false);
        }
        let runtime = self.runtime(task_id);
        // Step edits, spec reads and native goal updates share this lock. A writer already
        // at work keeps the session for this pass. Hold it through the park intent.
        let Ok(work) = runtime.steps_lock.try_lock() else {
            return Ok(false);
        };
        // A queue decision or replacement request must finish on one side of the park.
        let Ok(arm_writer) = runtime.arm_writer.try_write() else {
            return Ok(false);
        };
        if crate::sandboxes::plan_open(&self.deps.store.steps(task_id).await?)
            || self.read_spec_unfinished(task_id).await?
            || !self
                .parking_prs_still_current(&task, &prs, before.as_ref())
                .await?
            || runtime.goal_unfinished.load(Ordering::SeqCst)
        {
            return Ok(false);
        }
        {
            let mut intent = runtime.intent.lock().map_err(AppError::internal)?;
            // A reader may have taken Close since the idle probe, or finished the run.
            let Ok(attached) = runtime.run.try_lock() else {
                return Ok(false);
            };
            if attached
                .as_ref()
                .is_none_or(|current| current.attempt_id != run.id)
                || *runtime.terminal_cleanup.borrow() > 0
                || intent.is_some()
                || self.merge_is_armed(task_id)
            {
                return Ok(false);
            }
            crate::parking::save_parked_screen(
                &runtime.hub,
                Path::new(&run.run_dir),
                quiet_ms,
                because,
            )
            .map_err(AppError::internal)?;
            *intent = Some(EndIntent::Park);
            // Under the intent's lock: the run's end that takes the intent finds it.
            if let Ok(mut kept) = runtime.parked_activity.lock() {
                *kept = task.activity.map(|activity| (run.id.clone(), activity));
            }
        }
        drop(arm_writer);
        drop(work);
        crate::race::reached(crate::race::Point::ParkDecided, Path::new(task_id.as_str()));
        self.arm_escalation(task_id, Some(run.id.clone()));
        request_shutdown(&runtime, Some(&run.id)).await;
        Ok(true)
    }

    /// Whether a park of the task's session is under way (SPEC-512 PRK-003): nothing is typed
    /// into a session that is ending.
    pub(crate) fn parking(&self, task_id: &TaskId) -> bool {
        self.existing_runtime(task_id).is_some_and(|runtime| {
            runtime
                .intent
                .lock()
                .is_ok_and(|intent| matches!(*intent, Some(EndIntent::Park)))
        })
    }
}
