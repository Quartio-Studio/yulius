//! Why a task's live run is paused, as people see it, and a person's pause (SPEC-191, ADR-0154):
//! an instance update holds the run, or a person paused it. A capacity pause is its own status
//! (`waiting_capacity`, ROT-010).
//!
//! A person's pause freezes the run as an update does (ADR-0131): the agent stops mid-token and
//! is never told. A run that cannot be frozen, or whose freeze the kernel does not confirm, is
//! held at its turn boundary instead (its `Stop` and `UserPromptSubmit` hooks). The pause is
//! stored with the attempt it acts on, so it survives a restart and never touches a later run.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_contracts::api::common::UserRef;
use yulius_contracts::api::tasks::{TaskPause, TaskSummary};
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::ports::session::RunFreeze;

/// How a person's pause of the current run stands (`TaskRuntime::person_freeze`).
const NOT_ASKED: u8 = 0;
/// The kernel reports the run frozen.
const FROZEN: u8 = 1;
/// Asked; not confirmed yet.
const FREEZING: u8 = 2;
/// Not frozen: the run pauses at its turn boundary.
const AT_TURN_END: u8 = 3;

/// Freeze requests before an unconfirmed freeze is given up for the turn boundary (as the
/// update's `MAX_UNSETTLED_PASSES`).
const FREEZE_PASSES: u32 = 5;
/// A hook held in the last 15 s means the turn-boundary pause has hold (as the update's).
const HELD_FRESH_MS: i64 = 15_000;
/// How often the agent's hook client asks again while held.
pub(crate) const HOLD_RETRY_SECS: u64 = 3;

impl App {
    /// The pause of `task` for its summary (PAU-001): only a live run is paused, and a person's
    /// pause of the current run wins over an update's.
    pub(crate) async fn pause_view(&self, task: &TaskRecord) -> Option<TaskPause> {
        if !matches!(task.status, TaskStatus::Running | TaskStatus::Switching) {
            return None;
        }
        if let Some(pause) = self.current_person_pause(&task.id).await {
            return Some(TaskPause {
                by: "person".to_owned(),
                user: Some(UserRef {
                    login: self.login_of(&pause.by).await,
                }),
                since: Some(rfc3339(pause.at)),
                pending: !self.person_pause_holds(&task.id),
            });
        }
        self.held_for_update(&task.id).then(|| TaskPause {
            by: "update".to_owned(),
            user: None,
            since: None,
            pending: !self.update_pause_holds(&task.id),
        })
    }

    /// A person's pause of `task`'s live run (none for a pause left on an older attempt).
    async fn current_person_pause(&self, task: &TaskId) -> Option<crate::model::PersonPause> {
        let store = &self.deps.store;
        let pause = store.person_pause(task).await.ok()??;
        let attempt = store.live_attempt(task).await.ok()??;
        (attempt.id == pause.attempt).then_some(pause)
    }

    /// Whether a person paused `task`'s live run.
    pub(crate) async fn is_paused_by_person(&self, task: &TaskId) -> bool {
        self.current_person_pause(task).await.is_some()
    }

    /// Whether a person's pause of `task` took hold: frozen, or its hook held lately.
    fn person_pause_holds(&self, task: &TaskId) -> bool {
        let runtime = self.runtime(task);
        if runtime.person_freeze.load(Ordering::SeqCst) == FROZEN {
            return true;
        }
        let held = runtime.person_held_ms.load(Ordering::SeqCst);
        held > 0 && self.now().0.saturating_sub(held) <= HELD_FRESH_MS
    }

    /// `POST /api/tasks/{tid}/pause` (PER-001): the owner pauses a running task.
    ///
    /// # Errors
    /// `Forbidden` for anyone but its owner; `Conflict` unless it is running; store failures.
    pub async fn pause_task_for_person(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        if self.current_person_pause(&task.id).await.is_none() {
            let attempt = match (task.status, store.live_attempt(&task.id).await?) {
                (TaskStatus::Running, Some(attempt)) => attempt,
                (TaskStatus::WaitingCapacity, _) => {
                    return Err(AppError::Conflict(
                        "It is paused already: no subscription has capacity left.".into(),
                    ));
                }
                _ => {
                    return Err(AppError::Conflict(
                        "Only a running task can be paused.".into(),
                    ));
                }
            };
            // A pause left on an ended run goes first.
            store.clear_person_pause(&task.id, self.now()).await?;
            store
                .set_person_pause(&task.id, &ctx.user.id, &attempt.id, self.now())
                .await?;
            let _ = store
                .audit(
                    self.now(),
                    &self.login_of(&ctx.user.id).await,
                    "task.paused_by_person",
                    Some(task.id.as_str()),
                    None,
                )
                .await;
            self.freeze_for_person(&task.id, &attempt.id, &attempt.run_dir)
                .await;
            self.publish_task(&task.id).await;
        }
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// `POST /api/tasks/{tid}/resume` (PER-002): the owner resumes a task they paused. While an
    /// update holds it, the update thaws and releases it with the others.
    ///
    /// # Errors
    /// `Forbidden` for anyone but its owner; store failures.
    pub async fn resume_task_for_person(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        if let Some(pause) = store.person_pause(&task.id).await? {
            store.clear_person_pause(&task.id, self.now()).await?;
            let _ = store
                .audit(
                    self.now(),
                    &self.login_of(&ctx.user.id).await,
                    "task.resumed_by_person",
                    Some(task.id.as_str()),
                    None,
                )
                .await;
            self.unfreeze_for_person(&task.id, &pause.attempt).await;
            self.publish_task(&task.id).await;
        }
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Freezes the run of a person's pause; an unconfirmed freeze is asked again in the
    /// background and given up for the turn boundary (PER-001).
    async fn freeze_for_person(
        self: &Arc<Self>,
        task: &TaskId,
        attempt: &AttemptId,
        run_dir: &str,
    ) {
        let runtime = self.runtime(task);
        runtime.person_held_ms.store(0, Ordering::SeqCst);
        let state = match self.freeze_run_now(run_dir).await {
            RunFreeze::Frozen => FROZEN,
            RunFreeze::Freezing => FREEZING,
            RunFreeze::Unsupported => AT_TURN_END,
        };
        runtime.person_freeze.store(state, Ordering::SeqCst);
        if state != FREEZING {
            return;
        }
        let app = Arc::clone(self);
        let (task, attempt, run_dir) = (task.clone(), attempt.clone(), run_dir.to_owned());
        tokio::spawn(async move {
            let pass = Duration::from_millis(app.config.person_freeze_pass_ms.max(1));
            for _ in 1..FREEZE_PASSES {
                tokio::time::sleep(pass).await;
                if !app.still_paused(&task, &attempt).await {
                    return;
                }
                match app.freeze_run_now(&run_dir).await {
                    RunFreeze::Frozen => {
                        app.runtime(&task)
                            .person_freeze
                            .store(FROZEN, Ordering::SeqCst);
                        app.publish_task(&task).await;
                        return;
                    }
                    RunFreeze::Freezing => {}
                    RunFreeze::Unsupported => break,
                }
            }
            if !app.still_paused(&task, &attempt).await {
                return;
            }
            tracing::warn!(task = %task, "the run did not freeze; it pauses at its turn boundary");
            if !app.held_for_update(&task) {
                app.thaw_run_now(&run_dir).await;
            }
            app.runtime(&task)
                .person_freeze
                .store(AT_TURN_END, Ordering::SeqCst);
            app.publish_task(&task).await;
        });
    }

    async fn still_paused(&self, task: &TaskId, attempt: &AttemptId) -> bool {
        matches!(self.deps.store.person_pause(task).await, Ok(Some(p)) if p.attempt == *attempt)
    }

    /// Ends a person's pause of `attempt` in memory and thaws its run, unless an update holds
    /// the task (it thaws it with the others) or nothing was frozen.
    async fn unfreeze_for_person(&self, task: &TaskId, attempt: &AttemptId) {
        let runtime = self.runtime(task);
        let state = runtime.person_freeze.swap(NOT_ASKED, Ordering::SeqCst);
        runtime.person_held_ms.store(0, Ordering::SeqCst);
        if state == AT_TURN_END || self.held_for_update(task) {
            return;
        }
        if let Ok(Some(live)) = self.deps.store.live_attempt(task).await
            && live.id == *attempt
        {
            self.thaw_run_now(&live.run_dir).await;
        }
    }

    /// Whether a hook of the agent's run `attempt` is held for a person's pause (`Some`), and
    /// whether the hold just took it (`Some(true)`: worth publishing). Records it.
    pub(crate) async fn hold_for_person(&self, task: &TaskId, attempt: &AttemptId) -> Option<bool> {
        if !self.still_paused(task, attempt).await {
            return None;
        }
        let newly = !self.person_pause_holds(task);
        let now = self.now().0;
        let runtime = self.runtime(task);
        runtime.person_held_ms.store(now, Ordering::SeqCst);
        // Polled: a capacity pause of the same task must not take the hold as given up.
        runtime.hold_polled_ms.store(now, Ordering::SeqCst);
        Some(newly)
    }

    /// At start (PER-003): the live runs people paused are frozen again (idempotent), as this
    /// server knows nothing of them yet.
    pub async fn restore_person_pauses(self: &Arc<Self>) {
        let paused = match self.deps.store.person_paused_tasks().await {
            Ok(paused) => paused,
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the tasks people paused");
                return;
            }
        };
        for (task, pause) in paused {
            if let Ok(Some(live)) = self.deps.store.live_attempt(&task).await
                && live.id == pause.attempt
            {
                self.freeze_for_person(&task, &live.id, &live.run_dir).await;
            }
        }
    }

    /// The run directories of live runs a person paused that an update must leave frozen (all
    /// but those paused at their turn boundary, which the update's thaw does not wake). An error
    /// when the pauses cannot be read.
    pub(crate) async fn person_frozen_runs(&self) -> AppResult<Vec<String>> {
        let mut runs = Vec::new();
        for (task, pause) in self.deps.store.person_paused_tasks().await? {
            if self.runtime(&task).person_freeze.load(Ordering::SeqCst) == AT_TURN_END {
                continue;
            }
            if let Some(live) = self.deps.store.live_attempt(&task).await?
                && live.id == pause.attempt
            {
                runs.push(live.run_dir);
            }
        }
        Ok(runs)
    }

    /// Before Yulius ends a run (stop, close, a switch's restart, PER-004): a person's pause of it
    /// ends and its run is thawed, so the agent takes the interrupt as it would unpaused.
    pub(crate) async fn end_person_pause(&self, task: &TaskId) {
        let store = &self.deps.store;
        let Ok(Some(pause)) = store.person_pause(task).await else {
            return;
        };
        if let Err(err) = store.clear_person_pause(task, self.now()).await {
            tracing::warn!(error = %err, task = %task, "cannot clear the pause of an ending run");
            return;
        }
        self.unfreeze_for_person(task, &pause.attempt).await;
    }
}
