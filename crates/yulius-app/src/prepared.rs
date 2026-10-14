//! Prepared tasks (SPEC-105, SPEC-001 TSK-019): a task written now and started later. It waits
//! as `prepared`, holding no session and no worktree, until a person presses Start now ("when I
//! decide") or until its time, when the background tick releases it.
//!
//! Rules, in full: a prepared task without a time never starts by itself. One with a time starts
//! on the first tick at or after it (a time passed while Yulius was down: the first tick after
//! the restart), unless its sandbox's onboarding still blocks tasks; it then starts on a tick
//! after onboarding. Released, it goes `prepared → queued` (compare-and-set) and starts like any
//! new task (session limit, supertask hold, capacity). Cancel ends it; its time can be moved or
//! cleared while it waits.

use std::sync::Arc;

use yulius_contracts::api::tasks::{ScheduleTask, TaskSummary};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::{TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::imports::parse_rfc3339;
use crate::model::TaskRecord;
use crate::supervisor::RunStart;

/// Prepared tasks waiting in one sandbox at most.
pub const MAX_PREPARED_PER_SANDBOX: usize = 32;
/// How far in the past a start time may be: a time picked for "now" that passed while it was
/// sent still starts, on the next tick.
const PAST_GRACE_MS: i64 = 60_000;
/// How far ahead a start time may be.
const MAX_AHEAD_MS: i64 = 366 * 24 * 3_600_000;

/// Parses and checks a start time against `now` (PRP-001).
///
/// # Errors
/// `Invalid` for a malformed time, one more than a minute past or more than 366 days ahead.
pub fn parse_start_at(text: &str, now: Timestamp) -> AppResult<Timestamp> {
    let at = parse_rfc3339(text.trim()).ok_or_else(|| {
        AppError::Invalid("`start_at` must be an RFC 3339 time, e.g. 2026-10-15T09:00:00Z".into())
    })?;
    if at.millis_since(now) < -PAST_GRACE_MS {
        return Err(AppError::Invalid(
            "that start time has passed: pick a later one, or start the task now".into(),
        ));
    }
    if at.millis_since(now) > MAX_AHEAD_MS {
        return Err(AppError::Invalid(
            "a task can be scheduled at most 366 days ahead".into(),
        ));
    }
    Ok(at)
}

impl App {
    /// Refuses one more prepared task when the sandbox already has the most it may hold.
    pub(crate) async fn ensure_prepared_room(&self, sandbox: &SandboxId) -> AppResult<()> {
        let waiting = self
            .deps
            .store
            .tasks(sandbox, false)
            .await?
            .iter()
            .filter(|t| t.status == TaskStatus::Prepared)
            .count();
        if waiting >= MAX_PREPARED_PER_SANDBOX {
            return Err(AppError::Capacity(format!(
                "this sandbox already has {MAX_PREPARED_PER_SANDBOX} prepared tasks; start or \
                 cancel one first"
            )));
        }
        Ok(())
    }

    /// When a prepared task starts by itself; `None` without a time or once it is not prepared.
    pub(crate) async fn prepared_start_at(&self, task: &TaskRecord) -> Option<Timestamp> {
        if task.status != TaskStatus::Prepared {
            return None;
        }
        match self.deps.store.task_schedule(&task.id).await {
            Ok(at) => at,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "start time could not be read");
                None
            }
        }
    }

    /// A new prepared task whose time is already due (within the grace minute) starts on its
    /// creation rather than up to a minute later.
    pub(crate) async fn release_new_prepared(self: &Arc<Self>, task: &TaskRecord) {
        self.release_if_due(task).await;
    }

    /// Releases every prepared task whose time has come (background tick, PRP-003). A task whose
    /// sandbox's onboarding still blocks tasks waits for a later tick.
    pub async fn release_due_prepared(self: &Arc<Self>) {
        let due = match self.deps.store.due_prepared(self.now()).await {
            Ok(due) => due,
            Err(err) => {
                tracing::warn!(error = %err, "due prepared tasks could not be read");
                return;
            }
        };
        for id in due {
            if let Ok(Some(task)) = self.deps.store.task(&id).await {
                self.release_if_due(&task).await;
            }
        }
    }

    /// Releases `task` when its time has come and its sandbox's onboarding no longer blocks
    /// tasks; otherwise leaves it prepared.
    async fn release_if_due(self: &Arc<Self>, task: &TaskRecord) {
        if self
            .ensure_tasks_may_start(&task.sandbox_id, task.origin)
            .await
            .is_err()
        {
            return;
        }
        // The time is checked again as the status changes: one moved or cleared after it was
        // read wins (a reschedule racing the tick).
        match self
            .deps
            .store
            .release_scheduled(&task.id, self.now())
            .await
        {
            Ok(true) => self.after_release(task).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "scheduled task did not start");
            }
        }
    }

    /// Start now on a prepared task (PRP-004): it starts whatever its time.
    ///
    /// # Errors
    /// `Conflict` while the sandbox's onboarding blocks tasks or when it changed concurrently.
    pub(crate) async fn start_prepared_now(self: &Arc<Self>, task: &TaskRecord) -> AppResult<()> {
        if self
            .ensure_tasks_may_start(&task.sandbox_id, task.origin)
            .await
            .is_err()
        {
            return Err(AppError::Conflict(
                "this project is being prepared for Yulius; start this task once onboarding has \
                 made it ready (a scheduled task starts by itself then)"
                    .into(),
            ));
        }
        let next = TaskStatus::Prepared
            .apply(TaskEvent::PreparedReleased)
            .map_err(AppError::internal)?;
        if !self
            .deps
            .store
            .transition_task(&task.id, Some(TaskStatus::Prepared), next, None, self.now())
            .await?
        {
            return Err(AppError::Conflict("task state changed concurrently".into()));
        }
        self.after_release(task).await;
        Ok(())
    }

    /// A released task (now `queued`) is shown and starts like a new task.
    async fn after_release(self: &Arc<Self>, task: &TaskRecord) {
        self.publish_task(&task.id).await;
        let app = Arc::clone(self);
        let id = task.id.clone();
        tokio::spawn(async move {
            if let Err(err) = app
                .start_run(&id, RunStart::Fresh { prompt: None }, None)
                .await
            {
                tracing::warn!(error = %err, task = %id, "prepared task failed to start");
            }
        });
    }

    /// `PUT /api/tasks/{tid}/schedule`: sets or clears when a prepared task starts by itself.
    ///
    /// # Errors
    /// Access errors, `Invalid` for a time outside the window, `Conflict` when the task is not
    /// prepared.
    pub async fn schedule_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ScheduleTask,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        if task.status != TaskStatus::Prepared {
            return Err(AppError::Conflict(format!(
                "the task is {}, not prepared",
                task.status.as_str()
            )));
        }
        let now = self.now();
        let start_at = match req.start_at.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => Some(parse_start_at(text, now)?),
        };
        self.deps
            .store
            .set_task_schedule(&task.id, start_at, now)
            .await?;
        self.deps
            .store
            .audit(
                now,
                &self.login_of(&ctx.user.id).await,
                "task.scheduled",
                Some(task.id.as_str()),
                Some(&start_at.map_or_else(|| "when I decide".into(), crate::auth::rfc3339)),
            )
            .await?;
        self.publish_task(&task.id).await;
        // A time already due starts it on this call rather than up to a minute later.
        self.release_if_due(&task).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp(1_791_331_200_000); // 2026-10-07T00:00:00Z

    #[test]
    fn start_times_are_checked_against_now() {
        assert_eq!(
            parse_start_at("2026-10-07T01:00:00Z", NOW).ok(),
            Some(NOW.plus_millis(3_600_000))
        );
        // A local offset is the same instant.
        assert_eq!(
            parse_start_at("2026-10-07T03:00:00+02:00", NOW).ok(),
            Some(NOW.plus_millis(3_600_000))
        );
        assert!(parse_start_at("2026-10-06T23:59:30Z", NOW).is_ok());
        for (text, says) in [
            ("2026-10-06T23:58:00Z", "passed"),
            ("2027-10-09T00:00:00Z", "366 days"),
            ("tomorrow", "RFC 3339"),
        ] {
            let err = parse_start_at(text, NOW).err().map(|e| e.to_string());
            assert!(
                err.as_deref().is_some_and(|e| e.contains(says)),
                "{text}: {err:?}"
            );
        }
    }
}
