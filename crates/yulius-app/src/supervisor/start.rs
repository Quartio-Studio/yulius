//! Starting a run: the start kinds and conditions, admission under the launch lock, resuming a
//! parked session and draining the queue of tasks that wait for a session.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, TaskId};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use super::BoxFuture;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{
    AttemptRecord, PauseRecord, QueuedStart, QueuedStartKind, TaskMode, TaskRecord,
};

/// Opens the next turn in a saved session whose work was stopped.
pub(crate) const CONTINUE_TASK_PROMPT: &str = "Continue the task exactly where you left off.";

/// How the next run starts.
#[derive(Debug, Clone)]
pub enum RunStart {
    /// New session with the task prompt (or an explicit prompt).
    Fresh {
        /// Prompt override.
        prompt: Option<String>,
    },
    /// Resume the latest provider session of the task.
    Resume {
        /// Continuation prompt.
        prompt: Option<String>,
    },
    /// A fresh session carrying an automatic goal Stop instruction.
    GoalFresh {
        /// Instruction issued by the goal's Stop gate.
        prompt: String,
    },
    /// A resumed session carrying an automatic goal Stop instruction.
    GoalResume {
        /// Instruction issued by the goal's Stop gate.
        prompt: String,
    },
    /// Resume the latest provider session past the usage limit that cut its turn (SPEC-034):
    /// the transcript steps back over the limit and the agent redoes the lost turn by itself,
    /// with nothing typed or sent.
    AfterLimit,
}

impl RunStart {
    pub(super) fn prompt(&self) -> Option<&str> {
        match self {
            Self::Fresh { prompt } | Self::Resume { prompt } => prompt.as_deref(),
            Self::GoalFresh { prompt } | Self::GoalResume { prompt } => Some(prompt),
            Self::AfterLimit => None,
        }
    }

    /// Mark a prompt supplied by the goal's Stop gate, where its source is known.
    pub(crate) fn with_goal_continuation(self, issued_by_goal: bool) -> Self {
        match (self, issued_by_goal) {
            (
                Self::Fresh {
                    prompt: Some(prompt),
                },
                true,
            ) => Self::GoalFresh {
                prompt: crate::hooks::retained_goal_instruction(Some(prompt)),
            },
            (
                Self::Resume {
                    prompt: Some(prompt),
                },
                true,
            ) => Self::GoalResume {
                prompt: crate::hooks::retained_goal_instruction(Some(prompt)),
            },
            (start, _) => start,
        }
    }

    /// This start for `account`, as the queue keeps it for a task whose start has to wait
    /// (SPEC-483 QST-001).
    pub(crate) fn kept(&self, account: Option<&AccountId>) -> QueuedStart {
        let kind = match self {
            Self::Fresh { .. } => QueuedStartKind::Fresh,
            Self::Resume { .. } => QueuedStartKind::Resume,
            Self::GoalFresh { .. } => QueuedStartKind::GoalFresh,
            Self::GoalResume { .. } => QueuedStartKind::GoalResume,
            Self::AfterLimit => QueuedStartKind::AfterLimit,
        };
        QueuedStart {
            kind,
            prompt: self.prompt().map(str::to_owned),
            account_id: account.cloned(),
        }
    }

    /// The start and the subscription the queue kept.
    fn of_kept(kept: QueuedStart) -> (Self, Option<AccountId>) {
        let start = match kept.kind {
            QueuedStartKind::Fresh => Self::Fresh {
                prompt: kept.prompt,
            },
            QueuedStartKind::Resume => Self::Resume {
                prompt: kept.prompt,
            },
            QueuedStartKind::GoalFresh => Self::GoalFresh {
                prompt: crate::hooks::retained_goal_instruction(kept.prompt),
            },
            QueuedStartKind::GoalResume => Self::GoalResume {
                prompt: crate::hooks::retained_goal_instruction(kept.prompt),
            },
            QueuedStartKind::AfterLimit => Self::AfterLimit,
        };
        (start, kept.account_id)
    }
}

pub(super) enum StartCondition<'a> {
    OwnerContinue(&'a TaskRecord),
    Any,
    Status(TaskStatus),
    Parked(Option<AttemptId>),
}

/// Which task state the requested switch may take over.
pub(crate) enum SwitchCondition<'a> {
    Any,
    Status(TaskStatus),
    Paused(&'a PauseRecord),
    /// An idle session moved with nothing typed (a Codex move, SPEC-611; a rebalance move,
    /// SPEC-629): claimed only while its activity, update time and live attempt are still the
    /// ones read.
    Idle {
        from: (Activity, Timestamp),
        attempt: AttemptId,
    },
}

/// A capacity worker selects its subscription again if queued. A Move keeps its person's choice.
#[derive(Clone, Copy)]
pub(super) enum PausedStart<'a> {
    Capacity(&'a PauseRecord),
    Move(&'a PauseRecord),
}

/// What an idle session is after a switch that types nothing into it (a person's move of an idle
/// task, SPEC-447 MOV-002; a restart on a newer Claude Code, SPEC-461 CCM-013): its launch gives
/// the task this back, so it never reads Working, which viewers would take for work and a
/// person's notifications for news (SPEC-468 CCF-008).
///
/// No turn can begin in the new run before its launch gives the task this: nothing is typed
/// into it, and nobody can type before it is attached, which comes after. A prompt sent right
/// after that sets the task working through its hook, as any prompt does.
#[derive(Debug, Clone, Copy)]
pub(super) struct IdleRest {
    /// What the task read: waiting, or done.
    pub(super) activity: Activity,
    /// Whether Claude Code last recorded the session's goal as unfinished (SPEC-429): in the
    /// same session, with nothing typed, that is still what its agent left.
    pub(super) goal_unfinished: bool,
}

/// Names the task being launched until dropped (SPEC-458 RUN-001).
struct Launching<'a>(&'a Mutex<Option<TaskId>>);

impl<'a> Launching<'a> {
    fn start(slot: &'a Mutex<Option<TaskId>>, task: &TaskId) -> Self {
        if let Ok(mut launching) = slot.lock() {
            *launching = Some(task.clone());
        }
        Self(slot)
    }
}

impl Drop for Launching<'_> {
    fn drop(&mut self) {
        if let Ok(mut launching) = self.0.lock() {
            *launching = None;
        }
    }
}

impl App {
    /// Starts (or queues) a run for a task currently in `Queued`, `Starting` or a resting state.
    ///
    /// # Errors
    /// Precondition failures (no repository, no account), spawn failures.
    pub fn start_run<'a>(
        self: &'a Arc<Self>,
        task_id: &'a TaskId,
        start: RunStart,
        account: Option<AccountId>,
    ) -> BoxFuture<'a, AppResult<()>> {
        Box::pin(async move {
            self.start_run_if(task_id, start, account, StartCondition::Any, None, None)
                .await
                .map(|_| ())
        })
    }

    /// A person's Continue can start afresh if Codex stopped before creating its rollout.
    pub(crate) async fn start_ended_run(
        self: &Arc<Self>,
        task: &TaskRecord,
        account: Option<AccountId>,
    ) -> AppResult<()> {
        if self
            .start_run_if(
                &task.id,
                RunStart::Resume { prompt: None },
                account,
                StartCondition::OwnerContinue(task),
                None,
                None,
            )
            .await?
        {
            Ok(())
        } else {
            Err(AppError::Conflict(
                "the task changed while it was continued; try again".into(),
            ))
        }
    }

    /// Claims the capacity worker's observed pause under the launch lock. A Stop or a
    /// subsequent Continue can end that pause before the worker gets the lock.
    pub(crate) async fn start_paused_run(
        self: &Arc<Self>,
        pause: &PauseRecord,
        start: RunStart,
        account: Option<AccountId>,
    ) -> AppResult<bool> {
        self.start_run_if(
            &pause.task_id,
            start,
            account,
            StartCondition::Status(TaskStatus::WaitingCapacity),
            None,
            Some(PausedStart::Capacity(pause)),
        )
        .await
    }

    /// [`App::start_run`], only while the task still meets its start condition, checked
    /// under the launch lock: `false` when it no longer has it, so nothing was done. With
    /// `rest`, the run's task is idle again from its launch on ([`IdleRest`]).
    pub(super) fn start_run_if<'a>(
        self: &'a Arc<Self>,
        task_id: &'a TaskId,
        start: RunStart,
        account: Option<AccountId>,
        condition: StartCondition<'a>,
        rest: Option<IdleRest>,
        pause: Option<PausedStart<'a>>,
    ) -> BoxFuture<'a, AppResult<bool>> {
        Box::pin(async move {
            let resume = matches!(start, RunStart::Resume { .. } | RunStart::GoalResume { .. });
            let settled = if matches!(condition, StartCondition::Parked(_)) {
                self.wait_for_terminal_cleanup(task_id, true).await?;
                crate::race::reached(
                    crate::race::Point::ParkedResumePrepared,
                    Path::new(task_id.as_str()),
                );
                None
            } else {
                let selected = match condition {
                    StartCondition::OwnerContinue(selected) => Some(selected),
                    _ => None,
                };
                self.settle_resting_run(task_id, selected, resume).await?
            };
            // The Codex or Antigravity CLI is installed before the launch lock, so a first install
            // does not hold other tasks' starts; the launch asks again and reports a failure.
            if let Ok(Some(task)) = self.deps.store.task(task_id).await {
                let _ = self
                    .deps
                    .harness
                    .provide_cli(task.harness.provider(), true)
                    .await;
            }
            crate::race::reached(
                crate::race::Point::StartPreflightFinished,
                Path::new(task_id.as_str()),
            );
            let guard = self.supervisor.launch.lock().await;
            let task = self
                .deps
                .store
                .task(task_id)
                .await?
                .ok_or(AppError::NotFound)?;
            let owner_continue = matches!(condition, StartCondition::OwnerContinue(_));
            let mut start = start;
            match &condition {
                // A direct resume (including onboarding) can lose its place while acquiring the
                // lock. Only the resting selection checked outside it has finished settling.
                StartCondition::Any
                    if resume && (settled.is_some() || task.status.is_resting()) =>
                {
                    if !settled
                        .as_ref()
                        .is_some_and(|original| task.matches_continuation(original))
                    {
                        return Err(AppError::Conflict(
                            "The task changed during resume. Read it again after terminal exit."
                                .into(),
                        ));
                    }
                }
                StartCondition::OwnerContinue(selected) => {
                    if !task.matches_continuation(selected) {
                        return Ok(false);
                    }
                    if task.harness == Harness::Codex
                        && matches!(task.status, TaskStatus::Stopped(_) | TaskStatus::Failed(_))
                    {
                        start = self.codex_restart(&task).await?;
                    }
                }
                StartCondition::Status(status) if *status != task.status => return Ok(false),
                StartCondition::Parked(expected) => {
                    if task.status != TaskStatus::Parked
                        || task.resume_attempt_id.as_ref() != expected.as_ref()
                    {
                        return Ok(false);
                    }
                    if task.harness == Harness::Codex {
                        self.codex_resume_session(&task).await?;
                    }
                }
                _ => {}
            }
            if let Some(PausedStart::Capacity(record) | PausedStart::Move(record)) = pause
                && !self.pause_open(record).await
            {
                return Ok(false);
            }
            let moving_pause = matches!(pause, Some(PausedStart::Move(_)));
            let pause_outcome = if moving_pause { "switched" } else { "started" };
            // Already running: another start won the race (the queue and a direct start).
            if task.status == TaskStatus::Running {
                tracing::info!(task = %task.id, "start of a running task skipped");
                return Ok(true);
            }
            // A fresh start is for a task that waits to start (queued, paused for capacity). One
            // that ended meanwhile — closed before the start spawned at its creation ran — stays
            // ended. Only Continue authorizes a fresh start from the observed resting state.
            if matches!(condition, StartCondition::Any)
                && task.status.is_resting()
                && matches!(start, RunStart::Fresh { .. } | RunStart::GoalFresh { .. })
            {
                tracing::info!(
                    task = %task.id,
                    status = task.status.as_str(),
                    "fresh start of an ended task skipped"
                );
                return Ok(true);
            }
            // A pending supertask of the project, or the running work a supertask waits for,
            // keeps a new run queued (SPEC-071); a live run is never held.
            let held = !task.status.is_live() && self.held_by_supertask(&task).await?;
            let (working, mut cleared) = self.working_capacity().await?;
            let live = u32::try_from(working.len()).unwrap_or(u32::MAX);
            // While an update pauses the tasks, starts queue like at the session limit and run
            // once it is over (SPEC-002 ADR-18).
            let full = held || live >= self.max_live_sessions() || self.update_holds_starts();
            if full {
                cleared.clear();
            }
            if full && task.status != TaskStatus::Switching {
                let queued = self
                    .deps
                    .store
                    .tasks_with_status(&[TaskStatus::Queued])
                    .await?;
                if u32::try_from(queued.len()).unwrap_or(u32::MAX) >= self.config.queue_max
                    && task.status != TaskStatus::Queued
                {
                    return Err(AppError::Capacity(
                        "the session queue is full; try again later".into(),
                    ));
                }
                if task.status != TaskStatus::Queued {
                    // How it was to start waits with it, written with the status in one step:
                    // the queue starts every task with a plain start, which this one replaces
                    // (SPEC-483 QST-001). A subscription is kept when a person chose it (their
                    // Continue of a resting task); the pool's own choice for a paused task is
                    // made again at the launch, when it may be another.
                    let chosen = account
                        .as_ref()
                        .filter(|_| task.status.is_resting() || moving_pause);
                    let kept = start.kept(chosen);
                    let queued = if owner_continue {
                        self.claim_owner_continuation(&task, Some(&kept), chosen)
                            .await?
                            .is_some()
                    } else {
                        self.deps
                            .store
                            .queue_task(&task.id, task.status, &kept, self.now())
                            .await?
                    };
                    // It changed meanwhile (a stop takes no launch lock): nothing was done.
                    if !queued {
                        return Ok(false);
                    }
                    // A paused task's start waits in the queue now, not for capacity.
                    self.end_pause(&task.id, pause_outcome).await;
                    self.publish_task(&task.id).await;
                }
                return Ok(true);
            }
            // A queued task starts as the start that had to wait was to be, whoever starts it
            // now (SPEC-483 QST-001); one queued without a start takes the one given.
            let (start, account) = match task.status {
                TaskStatus::Queued => match self.deps.store.queued_start(&task.id).await? {
                    Some(kept) => {
                        let (start, chosen) = RunStart::of_kept(kept);
                        (start, self.kept_choice(&task, chosen).await?)
                    }
                    None => (start, account),
                },
                _ => (start, account),
            };
            // This start reserves the free slot under the launch lock. Completed owners
            // can accept a replacement merge once the reservation has been made.
            crate::race::reached(
                crate::race::Point::CandidateCapacityCleared,
                Path::new(task.id.as_str()),
            );
            let launching = Launching::start(&self.supervisor.launching, &task.id);
            drop(cleared);
            let admission = if owner_continue {
                self.prepare_start_claim(&task.id).await?;
                let Some(admitted) = self
                    .claim_owner_continuation(&task, None, account.as_ref())
                    .await?
                else {
                    return Ok(false);
                };
                self.publish_task(&task.id).await;
                Some(admitted)
            } else {
                None
            };
            // Launch failure cleanup answers subscription questions. A refused Continue
            // returns above, while the owner's original controls still belong to it.
            let result = self
                .launch_attempt(&task, start, account, rest, pause_outcome, admission)
                .await;
            if let Err(err) = &result {
                // Settle the failed launch before another Continue can install a new pause.
                self.end_pause(task_id, "ended").await;
                let reason = crate::account_gate::fail_reason(err);
                if let Ok(Some(current)) = self.deps.store.task(task_id).await
                    && matches!(current.status, TaskStatus::Starting | TaskStatus::Queued)
                {
                    let _ = self
                        .deps
                        .store
                        .transition_task(
                            task_id,
                            Some(current.status),
                            TaskStatus::Failed(reason),
                            None,
                            self.now(),
                        )
                        .await;
                }
            }
            drop(launching);
            drop(guard);
            crate::race::reached(
                crate::race::Point::StartLockReleased,
                Path::new(task.id.as_str()),
            );
            if let Err(err) = &result {
                self.runtime(task_id)
                    .hub
                    .notice(format!("Could not start: {}", err.public_message()));
                self.publish_task(task_id).await;
                // A supertask that could not start no longer holds its project (SUP-005).
                self.drain_queue().await;
            }
            result
        })
    }

    /// The subscription a person chose for a start that waited in the queue, when the task's
    /// turn comes (SPEC-483 QST-001): still theirs while it would take the task, as their
    /// choice of it would be accepted now. One that would be refused (at its limit, needing a
    /// sign-in, removed) is given up and the pool places the task, said in its terminal: a
    /// refusal now would fail a task whose person is no longer looking.
    async fn kept_choice(
        &self,
        task: &TaskRecord,
        chosen: Option<AccountId>,
    ) -> AppResult<Option<AccountId>> {
        let Some(chosen) = chosen else {
            return Ok(None);
        };
        let input = self.rotation_input(task, None).await?;
        let asked = Trigger::Manual {
            account: chosen.clone(),
        };
        if !matches!(rotation::decide(&input, asked).0, Decision::Stay) {
            return Ok(Some(chosen));
        }
        self.runtime(&task.id).hub.notice(
            "↻ The subscription chosen for this task cannot take it now, so Yulius chose one.",
        );
        Ok(None)
    }

    async fn claim_owner_continuation(
        &self,
        task: &TaskRecord,
        queue: Option<&QueuedStart>,
        requested: Option<&AccountId>,
    ) -> AppResult<Option<crate::capacity::ContinuationAdmission>> {
        let accepted = self
            .deps
            .store
            .claim_continuation(task, queue, requested, self.now())
            .await?;
        if accepted.is_some() {
            crate::race::reached(
                crate::race::Point::OwnerContinueAccepted,
                Path::new(task.id.as_str()),
            );
            // A resting owner's Continue clears only this task's browser actions (SPEC-494).
            self.clear_task_browser_actions(&task.sandbox_id, &task.id, None)
                .await;
        }
        Ok(accepted)
    }

    pub(super) async fn prepare_start_claim(&self, task_id: &TaskId) -> AppResult<()> {
        // A waiting or resting task with no run left: an intent of a stop or close that ended
        // no run is not for the run this launch starts (SPEC-140 STP-002). One whose run still
        // ends keeps it for that run's end.
        if self.deps.store.live_attempt(task_id).await?.is_none()
            && let Ok(mut intent) = self.runtime(task_id).intent.lock()
        {
            *intent = None;
        }
        crate::race::reached(
            crate::race::Point::StartClaimReady,
            Path::new(task_id.as_str()),
        );
        Ok(())
    }

    /// Whether this server holds the task's run: attached, watched, its end being recorded, or
    /// being launched (between the run's record and its attach it is none of the first three).
    pub(super) async fn holds_run(&self, task_id: &TaskId) -> bool {
        let launching = self
            .supervisor
            .launching
            .lock()
            .is_ok_and(|launching| launching.as_ref() == Some(task_id));
        if launching {
            return true;
        }
        let runtime = self.runtime(task_id);
        runtime.has_run().await
            || runtime.watcher_alive.load(Ordering::SeqCst)
            || *runtime.finalizing.borrow() > 0
    }

    /// Before a start (SPEC-458 RUN-002, #439): a resting task with a run still on record. A run
    /// this server holds lives on, so the start is refused before anything changes: started all
    /// the same it would be refused later, as "task already has a live run", and leave the task
    /// `failed_precondition` with that run going on. A run no server holds is settled as after a
    /// restart (ADR-0050): ended and counted, or killed and, once proven gone, recorded lost.
    ///
    /// # Errors
    /// `Conflict` while the run lives on or cannot be ended.
    async fn settle_resting_run(
        self: &Arc<Self>,
        task_id: &TaskId,
        selected: Option<&TaskRecord>,
        resume: bool,
    ) -> AppResult<Option<TaskRecord>> {
        let store = &self.deps.store;
        let Some(task) = store.task(task_id).await? else {
            return Ok(None);
        };
        let changed =
            || AppError::Conflict("the task changed while it was continued; try again".into());
        if selected.is_some_and(|seen| !task.matches_continuation(seen)) {
            return Err(changed());
        }
        if !task.status.is_resting() {
            return Ok(None);
        }
        if let Some(run) = store.live_attempt(task_id).await? {
            if selected.is_some_and(|seen| seen.resume_attempt_id.as_ref() != Some(&run.id)) {
                return Err(changed());
            }
            if self.holds_run(task_id).await {
                return Err(AppError::Conflict(
                    "the task still has a live run: stop it first".into(),
                ));
            }
            self.settle_unheld_run(task_id, &run.id).await?;
        }
        if resume {
            self.wait_for_terminal_cleanup(task_id, false).await?;
            return Ok(Some(task));
        }
        Ok(None)
    }

    /// Wait outside the launch lock for terminal cleanup. Successor and onboarding checks
    /// continue independently after the observer releases this count.
    async fn wait_for_terminal_cleanup(&self, task_id: &TaskId, parked: bool) -> AppResult<()> {
        let Some(runtime) = self.existing_runtime(task_id) else {
            return Ok(());
        };
        let mut ending = runtime.terminal_cleanup.subscribe();
        if *ending.borrow_and_update() == 0 {
            return Ok(());
        }
        let (point, retry) = if parked {
            (
                crate::race::Point::ParkedResumeWaitsForReader,
                "The parked screen is still being saved. Retry after the session finishes parking.",
            )
        } else {
            (
                crate::race::Point::ContinueWaitsForReader,
                "The previous session is still settling. Retry Continue after terminal exit.",
            )
        };
        crate::race::reached(point, Path::new(task_id.as_str()));
        let bound = Duration::from_millis(self.config.stop_escalation_ms);
        tokio::time::timeout(bound, ending.wait_for(|count| *count == 0))
            .await
            .map_err(|_| AppError::Conflict(retry.into()))?
            .map(|_| ())
            .map_err(AppError::internal)
    }

    /// Sessions that count against the session limit: live ones, except those of Done tasks.
    /// A Done task keeps its idle session for follow-ups, but it no longer works, so it does not
    /// keep queued tasks waiting (F 2026-10-12: "tasks put in Done doesn't unlock queue",
    /// SPEC-050 SES-101). A follow-up prompt can take the instance over the limit for a while.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn sessions_in_use(&self) -> AppResult<u32> {
        Ok(u32::try_from(self.working_attempts().await?.len()).unwrap_or(u32::MAX))
    }

    /// Live attempts except those of Done tasks: what holds a session slot (SES-101) and a
    /// subscription (SPEC-057 SUB-001, SUB-002). A Done task typed into again counts again.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn working_attempts(&self) -> AppResult<Vec<AttemptRecord>> {
        Ok(self.working_capacity().await?.0)
    }

    /// Collect idle owners first, then validate their deliveries together. Later rows
    /// can take long enough for a previously idle owner's replacement arm to arrive.
    async fn working_capacity(
        &self,
    ) -> AppResult<(
        Vec<AttemptRecord>,
        Vec<tokio::sync::OwnedRwLockReadGuard<()>>,
    )> {
        let store = &self.deps.store;
        let mut rows = Vec::new();
        for attempt in store.live_attempts().await? {
            let completion = match store.task(&attempt.task_id).await? {
                Some(task) => self.local_completion(task).await,
                None => None,
            };
            rows.push((attempt, completion));
        }
        let mut working = Vec::new();
        let mut cleared = Vec::new();
        for (attempt, completion) in rows {
            if let Some((task, record)) = completion
                && let Some(guard) = self.hold_live_delivery(&task, record.as_ref()).await
            {
                cleared.push(guard);
            } else {
                working.push(attempt);
            }
        }
        Ok((working, cleared))
    }

    /// Starts queued tasks while capacity allows, supertasks first, then FIFO (SPEC-071 SUP-004);
    /// none while an update pauses tasks. One pass runs at a time; a call during a pass makes it
    /// go once more (ADR-0054), so a burst of run ends costs at most two passes.
    pub async fn drain_queue(self: &Arc<Self>) {
        if self.update_holds_starts() || self.supertask_holds.begin_drain() {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                while app.supertask_holds.take_drain() {
                    app.drain_queue_now().await;
                }
                if !app.supertask_holds.release_drain() {
                    break;
                }
            }
        });
    }

    /// One pass of the queue, awaited ([`App::drain_queue`] runs them one at a time in the
    /// background). Every queued task's supertask hold is refreshed first (its label follows, also
    /// at the session limit, SUP-006); then the tasks no supertask holds are offered a slot one
    /// after the other (launches are serialized anyway), so the first in that order takes a free
    /// slot first.
    pub async fn drain_queue_now(self: &Arc<Self>) {
        if self.update_holds_starts() {
            return;
        }
        let Ok(queued) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Queued])
            .await
        else {
            return;
        };
        self.supertask_holds
            .keep(&queued.iter().map(|t| t.id.clone()).collect());
        let mut free = Vec::new();
        // Each project is read once for the pass, not once per queued task (SPEC-483 slice 37).
        let mut projects: std::collections::HashMap<SandboxId, Vec<TaskRecord>> =
            std::collections::HashMap::new();
        for task in queued {
            let project = match projects.entry(task.sandbox_id.clone()) {
                std::collections::hash_map::Entry::Occupied(read) => read.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    match self.deps.store.tasks(&task.sandbox_id, false).await {
                        Ok(read) => slot.insert(read),
                        // Unreadable: the task stays queued for this pass, as a failed read did.
                        Err(_) => continue,
                    }
                }
            };
            if !self
                .held_by_supertask_among(&task, project)
                .await
                .unwrap_or(true)
            {
                free.push(task);
            }
        }
        // A capacity resume a supertask held waits for the resume loop (SPEC-071): wake it.
        if self.supertask_holds.take_capacity_held() {
            self.capacity_changed();
        }
        for task in crate::supertask::start_order(free) {
            let live = self.sessions_in_use().await.unwrap_or(u32::MAX);
            if live >= self.max_live_sessions() {
                return;
            }
            let has_lineage = self
                .deps
                .store
                .provider_sessions(&task.id)
                .await
                .map(|l| !l.is_empty())
                .unwrap_or(false);
            let start = if has_lineage {
                RunStart::Resume { prompt: None }
            } else {
                RunStart::Fresh { prompt: None }
            };
            // Only while it is still queued, checked under the launch lock: a task stopped or
            // started meanwhile is not resumed here.
            if let Err(err) = self
                .start_run_if(
                    &task.id,
                    start,
                    None,
                    StartCondition::Status(TaskStatus::Queued),
                    None,
                    None,
                )
                .await
            {
                tracing::warn!(error = %err, task = %task.id, "queued task failed to start");
            }
        }
    }

    /// Resumes a parked task in its own session (SPEC-512 PRK-005), with `prompt` as the turn it
    /// opens with, or with nothing typed: a prompt-mode session that gets nothing reads as it
    /// did, idle, from its launch on ([`IdleRest`]). At the session limit it waits in the queue
    /// with its prompt. `false` when the task was no longer parked.
    ///
    /// # Errors
    /// As [`App::start_run`].
    pub(crate) async fn resume_parked(
        self: &Arc<Self>,
        task: &TaskRecord,
        prompt: Option<String>,
    ) -> AppResult<bool> {
        if task.status != TaskStatus::Parked {
            return Ok(false);
        }
        let goal_unfinished = self.goal_unfinished_of(task).await;
        let rest = task
            .activity
            .filter(|activity| {
                activity.is_idle() && prompt.is_none() && task.mode == TaskMode::Prompt
            })
            .map(|activity| IdleRest {
                activity,
                goal_unfinished,
            });
        // A pinned task resumes on its pin while the pin can take it (SPEC-629 RBL-005).
        let pinned = self.pinned_start(task).await;
        self.start_run_if(
            &task.id,
            RunStart::Resume { prompt },
            pinned,
            StartCondition::Parked(task.resume_attempt_id.clone()),
            rest,
            None,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-483 QST-001: a start the queue keeps comes back as it was, with its prompt and the
    /// subscription a person chose.
    #[test]
    fn a_kept_start_comes_back_as_it_was() -> Result<(), Box<dyn std::error::Error>> {
        use super::RunStart;
        let chosen = yulius_domain::ids::AccountId::parse("a-chosen")?;
        for (start, account) in [
            (RunStart::AfterLimit, None),
            (
                RunStart::Resume {
                    prompt: Some("the held prompt".into()),
                },
                Some(chosen.clone()),
            ),
            (RunStart::Resume { prompt: None }, None),
            (
                RunStart::Fresh {
                    prompt: Some("a hand-over".into()),
                },
                None,
            ),
            (RunStart::Fresh { prompt: None }, Some(chosen)),
        ] {
            let (back, on) = RunStart::of_kept(start.kept(account.as_ref()));
            assert_eq!(format!("{back:?}"), format!("{start:?}"));
            assert_eq!(on, account);
        }
        Ok(())
    }
}
