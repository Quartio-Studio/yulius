//! Switching a run's subscription or provider: the switch entry points, the wait for the old
//! run's end, its timeout and overtaking, the preflight and the cooldowns.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::oneshot;
use yulius_domain::harness::{Harness, claude_model_name};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::task::{STOP_INTERRUPT_GRACE_MS, StopReason, TaskEvent, TaskStatus};
use yulius_domain::usage::{ModelFamily, ReportedLimit};

use super::start::{IdleRest, PausedStart, RunStart, StartCondition, SwitchCondition};
use super::{EndIntent, SwitchEnd};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, SwitchDecisionRecord, TaskMode, TaskRecord};

/// After a switch was refused before it started, automatic switches of the task wait this long
/// (the agent keeps working on its current subscription meanwhile).
pub const SWITCH_COOLDOWN_MS: i64 = 10 * 60 * 1000;

/// Whether a switch for `reason` restarts the session where it is and asks for its subscription
/// only to keep it: a newer Claude Code, a fresh session after a full context, a hand-over. A
/// person's choice and the pool's own decisions name the subscription they mean.
fn keeps_subscription(reason: &str) -> bool {
    matches!(reason, "context_full" | "handover") || reason == crate::claude_code::MOVE_REASON
}

/// Whether a switch for `reason` interrupts the turn before ending the run: a person's switch
/// and a hand-over may come mid-turn (SPEC-332), and so may a removal of the subscription (SPEC-447)
/// and a move for a review round's Fable reviewer, whose refused Agent call the agent answers
/// (SPEC-493 REV-007);
/// automatic switches come while the agent is idle, or at a usage limit, where its turn can go
/// no further: its own thread's when the limit cut its turn, and whatever its thread was doing
/// when the limit refused a workflow's agents (its next request would be refused, SPEC-486).
fn interrupts_first(reason: &str) -> bool {
    matches!(reason, "manual" | "handover" | "removed")
        || reason == crate::review::MOVE_FOR_FABLE
        || reason == crate::review::MOVE_FOR_ROOM
}

impl App {
    /// Switches a running task to another account (manual or rotation-driven).
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn switch_task_now(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
        prompt: Option<String>,
    ) -> AppResult<()> {
        self.switch_with(task_id, account, reason, RunStart::Resume { prompt })
            .await
    }

    /// Records a usage limit the CLI reported on `account` to a session that runs a model of
    /// `family`: what the CLI was told is the account's state (ROT-009), the limited window full
    /// until its reset, even when the provider's usage request does not answer. A limit of one
    /// model family fills that family's own window, never the subscription's week (SPEC-464,
    /// [`AccountUsage::record_limit`]). Shared by tasks and the Cockpit (SPEC-074).
    pub(crate) async fn record_limit(
        &self,
        account: &AccountId,
        limit: ReportedLimit,
        family: ModelFamily,
    ) {
        crate::race::reached(crate::race::Point::LimitRecord, Path::new(account.as_str()));
        let now = self.now();
        self.change_account_usage(account, |mut usage| {
            usage.record_limit(limit, family, now);
            Some(usage)
        })
        .await;
    }

    /// Switches a task whose turn a usage limit cut (SPEC-034): the session resumes on `account`
    /// (the policy's choice when `None`) past the limit, with nothing typed or sent.
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub async fn switch_after_limit(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
    ) -> AppResult<()> {
        self.switch_with(task_id, account, reason, RunStart::AfterLimit)
            .await
    }

    /// Continues a task whose session ran out of context in a fresh session on `account`, opened
    /// with `handoff` (SPEC-060 CTX-004).
    ///
    /// # Errors
    /// `Conflict` when the task is not running.
    pub(crate) async fn restart_fresh(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        handoff: &str,
    ) -> AppResult<()> {
        let start = RunStart::Fresh {
            prompt: Some(handoff.to_owned()),
        };
        self.switch_with(task_id, account, "context_full", start)
            .await
    }

    /// Switches a live task with an explicitly selected session start.
    ///
    /// # Errors
    /// Refuses an unavailable account or a task that cannot switch.
    pub async fn switch_with(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
        start: RunStart,
    ) -> AppResult<()> {
        self.switch_from(task_id, account, reason, start, SwitchCondition::Any)
            .await
    }

    /// Resumes a task paused at a usage limit past it on `account` (SPEC-034), only while it is
    /// still waiting for capacity: the CLI may have continued the turn by itself meanwhile
    /// (SPEC-072 LRS-001), and then the run is left alone.
    ///
    /// # Errors
    /// `Conflict` when the task no longer waits for capacity.
    pub(crate) async fn resume_paused_after_limit(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
    ) -> AppResult<()> {
        self.switch_from(
            task_id,
            account,
            reason,
            RunStart::AfterLimit,
            SwitchCondition::Status(TaskStatus::WaitingCapacity),
        )
        .await
    }

    /// [`Self::switch_with`] of a task that still meets `condition`.
    pub(crate) async fn switch_from(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
        start: RunStart,
        condition: SwitchCondition<'_>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let (expected, pause) = match &condition {
            SwitchCondition::Any => (None, None),
            SwitchCondition::Status(status) => (Some(*status), None),
            SwitchCondition::Paused(pause) => (Some(TaskStatus::WaitingCapacity), Some(*pause)),
            SwitchCondition::Idle { .. } => (Some(TaskStatus::Running), None),
        };
        if expected.is_some_and(|status| status != task.status) {
            return Err(AppError::Conflict(format!(
                "task is {}",
                task.status.as_str()
            )));
        }
        let attempt = store.live_attempt(task_id).await?;
        let idle_claim = match &condition {
            SwitchCondition::Idle { from, attempt } => Some((*from, attempt)),
            _ if task.harness == Harness::Codex
                && task.status == TaskStatus::Running
                && matches!(start, RunStart::Resume { prompt: None }) =>
            {
                task.activity
                    .filter(|activity| activity.is_idle())
                    .zip(attempt.as_ref())
                    .map(|(activity, run)| ((activity, task.updated_at), &run.id))
            }
            _ => None,
        };
        if pause.is_some() && attempt.is_some() {
            return Err(AppError::Conflict(
                "the paused start has already run".into(),
            ));
        }
        let next = task
            .status
            .apply(TaskEvent::SwitchStarted)
            .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?;
        // A restart that only keeps its subscription lets the pool place the run when that
        // subscription cannot take it and another can (SPEC-482 FBK-004).
        let account = match account {
            Some(kept) if keeps_subscription(reason) => {
                let input = self.rotation_input(&task, None).await?;
                // A replayed access refusal can make the kept subscription unusable.
                // Only relinquish it for a pool placement that can actually take the turn.
                if reason == "handover" && matches!(start, RunStart::AfterLimit) {
                    let asked = Trigger::Manual {
                        account: kept.clone(),
                    };
                    match (
                        rotation::decide(&input, asked).0,
                        rotation::decide(&input, Trigger::Start).0,
                    ) {
                        (Decision::Stay, Decision::Switch { account }) => Some(account),
                        _ => Some(kept),
                    }
                } else {
                    rotation::keeps_account(&input, &kept).then_some(kept)
                }
            }
            other => other,
        };
        // The new session must be able to start before the running one ends (ROT-014): a
        // refusal leaves the task as it is.
        let fallback = match self.preflight_start(&task, account.as_ref()).await {
            Ok(fallback) => fallback,
            Err(err) => {
                self.refuse_switch(&task, account.as_ref(), reason, &err)
                    .await;
                return Err(err);
            }
        };
        crate::race::reached(
            crate::race::Point::SwitchPrepared,
            Path::new(task_id.as_str()),
        );
        // A saved start has no live run to switch out. Keep its pause until the launch lock
        // claims that exact pause, even if Stop and Continue created a new one during preflight.
        if pause.is_none() {
            let claimed = if let Some((from, attempt)) = idle_claim {
                let runtime = self.runtime(task_id);
                let _order = runtime.turn_order.lock().await;
                store
                    .claim_idle_switch(task_id, from, attempt, self.now())
                    .await?
            } else {
                store
                    .transition_task(task_id, Some(task.status), next, None, self.now())
                    .await?
            };
            if !claimed {
                return Err(AppError::Conflict("task state changed concurrently".into()));
            }
            self.end_pause(task_id, "switched").await;
        }
        // The turn a limit cut is continued by the new session itself (SPEC-034): nobody has to
        // send the message a goal pause at that limit asked for (SPEC-464 UCL-007).
        if matches!(start, RunStart::AfterLimit) {
            self.end_limit_requests(task_id, crate::goal_retry::TakenOn::Moved)
                .await;
        }
        let from_desc = match &attempt {
            Some(a) => self.account_desc(a.harness, a.account_id.as_ref()).await,
            None => task.harness.as_str().to_owned(),
        };
        let decision_id = self.deps.crypto.new_id();
        let mut to_desc = self.account_desc(task.harness, account.as_ref()).await;
        // A move onto the fallback names the model (SPEC-482 FBK-008).
        if let Some(fallback) = fallback {
            to_desc = format!("{to_desc}, {}", claude_model_name(fallback));
        }
        store
            .insert_switch(&SwitchDecisionRecord {
                id: decision_id.clone(),
                task_id: task_id.clone(),
                at: self.now(),
                reason: reason.to_owned(),
                inputs: serde_json::json!({"account": account.as_ref().map(AccountId::as_str)})
                    .to_string(),
                from_desc,
                to_desc,
                outcome: "started".into(),
            })
            .await?;
        let runtime = self.runtime(task_id);
        // The subscription changes under the hood (F 2026-10-10): viewers see that the session
        // continues, not which accounts are involved (Task details lists the switches).
        // The policy may pick the same subscription (a limit that reset): no "another" then.
        let elsewhere = account.is_some()
            && attempt.as_ref().map(|a| a.account_id.as_ref()) != Some(account.as_ref());
        let fresh = matches!(start, RunStart::Fresh { .. } | RunStart::GoalFresh { .. });
        // A restart for a newer Claude Code says that, in its one line (SPEC-461 CCM-013).
        let for_claude_code = reason == crate::claude_code::MOVE_REASON;
        if for_claude_code {
            runtime.hub.notice(self.claude_code_move_notice());
        } else {
            let line = if fresh {
                "↻ Yulius starts a fresh session for this task…".to_owned()
            } else if elsewhere {
                format!(
                    "↻ Yulius continues this session on another {} subscription…",
                    crate::account_gate::account_kind(task.harness.provider())
                )
            } else {
                "↻ Yulius continues this session…".to_owned()
            };
            // A workflow the old process leaves unfinished is resumed (SPEC-486 WFL-004).
            let resumes_workflow =
                !fresh && runtime.with_workflows(|workflows| !workflows.runs().is_empty());
            if resumes_workflow {
                let resumed = crate::dynamic_workflow::RESUMED_NOTICE;
                runtime.hub.notice(format!("{line}{resumed}"));
            } else {
                runtime.hub.notice(line);
            }
        }
        self.publish_task(task_id).await;
        if let Some(attempt) = attempt {
            // A paused run takes the end as it would unpaused (SPEC-191 PER-004).
            self.end_person_pause(task_id).await;
            let (tx, rx) = oneshot::channel();
            crate::race::reached(
                crate::race::Point::SwitchDecided,
                Path::new(task_id.as_str()),
            );
            // A stop or close that came since the status change owns the old run's end: its intent
            // is kept, and the switch gives up (SPEC-250 STW-006).
            let taken = match runtime.intent.lock() {
                Ok(mut intent) => match *intent {
                    Some(EndIntent::Cancel) => Some(SwitchEnd::Stopped),
                    Some(EndIntent::Close) => Some(SwitchEnd::Closed),
                    // A park ends the run as a close does (SPEC-512 PRK-003).
                    Some(EndIntent::Park) => Some(SwitchEnd::Parked),
                    _ => {
                        *intent = Some(EndIntent::Switch(tx));
                        None
                    }
                },
                Err(_) => Some(SwitchEnd::Lost),
            };
            if taken.is_some() || self.switch_goes_on(task_id).await.is_none() {
                if let Ok(mut intent) = runtime.intent.lock()
                    && matches!(*intent, Some(EndIntent::Switch(_)))
                {
                    *intent = None;
                }
                return self.switch_overtaken(task_id, &decision_id, taken).await;
            }
            if let Some(run) = runtime.run.lock().await.as_mut() {
                // An automatic switch happens at a boundary (or a held hook, or a limit): the
                // agent is idle, so it is terminated without an interrupt that could start or
                // cancel anything. A person's switch interrupts first, so the transcript keeps
                // the interrupted turn.
                if !interrupts_first(reason) {
                    let _ = run
                        .writer
                        .signal(crate::ports::session::RunSignal::Terminate)
                        .await;
                }
                let _ = run
                    .writer
                    .shutdown(u32::try_from(STOP_INTERRUPT_GRACE_MS).unwrap_or(5000))
                    .await;
            }
            let budget = Duration::from_millis(self.config.switch_end_ms);
            let mut rx = rx;
            let mut word = tokio::time::timeout(budget, &mut rx).await;
            if word.is_err() {
                crate::race::reached(
                    crate::race::Point::SwitchWaitOver,
                    Path::new(task_id.as_str()),
                );
                // The switch gives the run up, in the same step in which it sees that the
                // run's end is still its own. A slot that no longer holds its sender was taken
                // by the run's end, which is being recorded right now, or by a stop or a close:
                // their word comes, and is waited for once more.
                let gave_up = runtime.intent.lock().is_ok_and(|mut intent| {
                    let ours = matches!(*intent, Some(EndIntent::Switch(_)));
                    if ours {
                        *intent = Some(EndIntent::SwitchTimeout);
                    }
                    ours
                });
                if !gave_up {
                    word = tokio::time::timeout(budget, &mut rx).await;
                }
            }
            match word {
                Ok(Ok(SwitchEnd::Ended)) => {}
                // The switch's intent was replaced: a stop or close took over the old run's end,
                // or its loss dropped it. No new run starts (SPEC-250 STW-006).
                Ok(Ok(why)) => {
                    return self
                        .switch_overtaken(task_id, &decision_id, Some(why))
                        .await;
                }
                Ok(Err(_)) => return self.switch_overtaken(task_id, &decision_id, None).await,
                Err(_) => {
                    return self
                        .switch_timed_out(task_id, &decision_id, &attempt, for_claude_code)
                        .await;
                }
            }
        }
        // Only while the task is still switching, checked under the launch lock: one stopped or
        // closed meanwhile is not started again, nor queued (STW-006).
        let from = match pause {
            Some(_) => TaskStatus::WaitingCapacity,
            None => match self.switch_goes_on(task_id).await {
                Some(status) => status,
                None => return self.switch_overtaken(task_id, &decision_id, None).await,
            },
        };
        crate::race::reached(
            crate::race::Point::SwitchLaunching,
            Path::new(task_id.as_str()),
        );
        // An idle session that opens without a turn retains its activity (SPEC-447 MOV-002).
        // Codex reopens at its prompt in both task modes (SPEC-611 CQS-001).
        let rest = task
            .activity
            .filter(|activity| {
                activity.is_idle()
                    && task.status == TaskStatus::Running
                    && (task.mode == TaskMode::Prompt || task.harness == Harness::Codex)
                    && matches!(start, RunStart::Resume { prompt: None })
                    // A session told to resume its workflow opens with that turn (SPEC-486).
                    && runtime.with_workflows(|workflows| workflows.runs().is_empty())
            })
            .map(|activity| IdleRest {
                activity,
                goal_unfinished: runtime.goal_unfinished.load(Ordering::SeqCst),
            });
        let started = self.start_run_if(
            task_id,
            start,
            account,
            StartCondition::Status(from),
            rest,
            pause.map(PausedStart::Move),
        );
        match started.await {
            // Another start (the capacity loop's) ran the paused task first: it won, the switch
            // is made.
            Ok(false)
                if pause.is_some()
                    || from != TaskStatus::WaitingCapacity
                    || !matches!(
                        store.task(task_id).await,
                        Ok(Some(t)) if t.status == TaskStatus::Running
                    ) =>
            {
                self.switch_overtaken(task_id, &decision_id, None).await
            }
            Ok(_) => {
                // The switch found no subscription with capacity: the task paused instead.
                let paused = matches!(
                    store.task(task_id).await,
                    Ok(Some(t)) if matches!(t.status, TaskStatus::WaitingCapacity | TaskStatus::Queued)
                );
                if paused {
                    store.set_switch_outcome(&decision_id, "waiting").await?;
                } else {
                    store.set_switch_outcome(&decision_id, "completed").await?;
                    if for_claude_code {
                        runtime.claude_move_refused_ms.store(0, Ordering::SeqCst);
                    } else {
                        runtime.switch_refused_ms.store(0, Ordering::SeqCst);
                    }
                    if !for_claude_code {
                        runtime.hub.notice(if fresh {
                            "↻ The task continues in a fresh session."
                        } else {
                            "↻ The session continues where it left off."
                        });
                    }
                }
                Ok(())
            }
            Err(err) => {
                store.set_switch_outcome(&decision_id, "failed").await?;
                tracing::warn!(task = %task_id, error = %err, "the session could not continue after a switch");
                runtime.hub.notice(format!(
                    "↻ The session could not continue: {}",
                    err.public_message()
                ));
                if pause.is_none()
                    && let Ok(Some(current)) = store.task(task_id).await
                    && current.status == TaskStatus::Switching
                {
                    let _ = store
                        .transition_task(
                            task_id,
                            Some(current.status),
                            TaskStatus::Stopped(StopReason::SwitchFailed),
                            None,
                            self.now(),
                        )
                        .await;
                }
                self.publish_task(task_id).await;
                Err(err)
            }
        }
    }

    /// The status a switch goes on from: still `Switching`, or paused for capacity meanwhile (a
    /// Stop hook's wait, which the launch takes as before). `None` once a stop, a close or the
    /// old run's loss moved the task on, or the store cannot say.
    async fn switch_goes_on(&self, task_id: &TaskId) -> Option<TaskStatus> {
        match self.deps.store.task(task_id).await {
            Ok(Some(task))
                if matches!(
                    task.status,
                    TaskStatus::Switching | TaskStatus::WaitingCapacity
                ) =>
            {
                Some(task.status)
            }
            _ => None,
        }
    }

    /// Ends a switch whose old run did not end in time (SPEC-483 SWT-002). Its host is
    /// terminated and no new run starts: the task rests as `stopped_switch_timeout`, whether
    /// the switch, the run's reader or the run's own exit records the end first (the task's end
    /// intent says whose it is). A host that cannot be terminated leaves the run the task's
    /// live run (SPEC-065 END-001): no session starts beside it, the task runs on, says so to a
    /// person, and the next turn's end does not try the switch again at once.
    ///
    /// # Errors
    /// Always `Conflict`, saying how the switch ended; a store error when recording it.
    async fn switch_timed_out(
        self: &Arc<Self>,
        task_id: &TaskId,
        decision_id: &str,
        attempt: &AttemptRecord,
        for_claude_code: bool,
    ) -> AppResult<()> {
        let runtime = self.runtime(task_id);
        crate::race::reached(
            crate::race::Point::SwitchEndsRun,
            Path::new(task_id.as_str()),
        );
        let Err(err) = self
            .deps
            .session_host
            .terminate(Path::new(&attempt.run_dir))
        else {
            self.mark_lost(task_id, &attempt.id, StopReason::SwitchTimeout)
                .await;
            return self
                .switch_overtaken(task_id, decision_id, Some(SwitchEnd::TimedOut))
                .await;
        };
        let store = &self.deps.store;
        // The task runs, in the session it had: a stop of it tries the end again. Made while
        // the run's end is still the switch's, so a run that ends by itself at this moment
        // rests the task from either status; only then is its end nobody's.
        store
            .transition_task(
                task_id,
                Some(TaskStatus::Switching),
                TaskStatus::Running,
                None,
                self.now(),
            )
            .await?;
        let given_up = runtime.intent.lock().is_ok_and(|mut intent| {
            let still = matches!(*intent, Some(EndIntent::SwitchTimeout));
            if still {
                *intent = None;
            }
            still
        });
        // Someone took the slot: the run's end is being recorded (or a stop owns it). The
        // record is waited for, so that a run which ended is not asked about too early.
        if !given_up {
            let mut finalizing = runtime.finalizing.subscribe();
            let bound = Duration::from_millis(self.config.switch_end_ms);
            let _ = tokio::time::timeout(bound, finalizing.wait_for(|count| *count == 0)).await;
        }
        // The run ended by itself while its host was asked to go: nothing is left to say.
        if !matches!(store.attempt(&attempt.id).await, Ok(Some(run)) if !run.state.is_terminal()) {
            return self
                .switch_overtaken(task_id, decision_id, Some(SwitchEnd::TimedOut))
                .await;
        }
        store.set_switch_outcome(decision_id, "failed").await?;
        // A restart for a newer Claude Code is no switch a person reads about (SPEC-461
        // CCM-013): the session that could not be ended is.
        if for_claude_code {
            runtime
                .claude_move_refused_ms
                .store(self.now().0, Ordering::SeqCst);
        } else {
            runtime
                .switch_refused_ms
                .store(self.now().0, Ordering::SeqCst);
            runtime
                .hub
                .notice("↻ The session was not switched: it could not be ended.");
        }
        self.run_stays_live(task_id, attempt, &err, "Stop the task to try again.")
            .await;
        self.publish_task(task_id).await;
        Err(AppError::Conflict(
            "the session could not be ended, so it was not switched".into(),
        ))
    }

    /// Ends a switch that a stop, a close or the old run's loss overtook (SPEC-250 STW-006): no
    /// new run starts, a task left stopping with no run rests, the decision is recorded
    /// `cancelled` and viewers are told why.
    ///
    /// # Errors
    /// Always `Conflict`, saying what overtook the switch; a store error when recording it.
    async fn switch_overtaken(
        self: &Arc<Self>,
        task_id: &TaskId,
        decision_id: &str,
        told: Option<SwitchEnd>,
    ) -> AppResult<()> {
        crate::race::reached(
            crate::race::Point::SwitchOvertaken,
            Path::new(task_id.as_str()),
        );
        // A switch that ended its own old run failed; one that was overtaken is cancelled.
        let outcome = if told == Some(SwitchEnd::TimedOut) {
            "failed"
        } else {
            "cancelled"
        };
        self.deps
            .store
            .set_switch_outcome(decision_id, outcome)
            .await?;
        // A stop that came once the old run had ended found no run to end: the task is stopping
        // with no run, and rests now, as a stop of it would rest it (one that a concurrent stop
        // rested first is left as it is).
        if matches!(
            self.deps.store.task(task_id).await,
            Ok(Some(task)) if task.status == TaskStatus::Stopping
        ) && matches!(self.deps.store.live_attempt(task_id).await, Ok(None))
            && let Err(err) = self.stop_round(task_id, true).await
        {
            tracing::warn!(task = %task_id, error = %err, "the overtaken switch's task was not rested");
        }
        let status = self
            .deps
            .store
            .task(task_id)
            .await
            .ok()
            .flatten()
            .map(|t| t.status);
        let intent = self
            .runtime(task_id)
            .intent
            .lock()
            .map(|intent| match *intent {
                Some(EndIntent::Close) => Some(true),
                Some(EndIntent::Cancel) => Some(false),
                _ => None,
            })
            .unwrap_or(None);
        let parking = self.parking(task_id);
        // What overtook it says so; without word, the status and the intent tell.
        let why = match (told, status, intent) {
            (Some(SwitchEnd::TimedOut), _, _) => "the session did not end in time",
            (Some(SwitchEnd::Parked), _, _) | (None, Some(TaskStatus::Parked), _) => {
                "the idle session was parked"
            }
            (None, _, None) if parking => "the idle session was parked",
            (Some(SwitchEnd::Closed), _, _)
            | (None, Some(TaskStatus::Closed), _)
            | (None, _, Some(true)) => "the task was closed",
            (Some(SwitchEnd::Stopped), _, _)
            | (None, Some(TaskStatus::Stopping | TaskStatus::Stopped(StopReason::Cancelled)), _)
            | (None, _, Some(false)) => "the task was stopped",
            _ => "the session ended",
        };
        self.runtime(task_id)
            .hub
            .notice(format!("↻ The session was not switched: {why}."));
        self.publish_task(task_id).await;
        Err(AppError::Conflict(format!("{why} during the switch")))
    }

    /// What a launch would need, checked without changing anything: the repository, a usable
    /// subscription credential and the sandbox's isolation (disk quota, holder). A switch runs
    /// it before it ends the running session (ROT-014). `Ok` names the fallback model the
    /// launch would be placed with (SPEC-482), `None` for the task's own.
    ///
    /// # Errors
    /// The reason the launch would be refused.
    pub async fn preflight_start(
        &self,
        task: &TaskRecord,
        requested: Option<&AccountId>,
    ) -> AppResult<Option<&'static str>> {
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let worktree = self
            .sandbox_dir(&sandbox.id)
            .join("worktrees")
            .join(task.id.as_str());
        if !worktree.join(".git").exists() && !self.sandbox_repo(&sandbox.id).join(".git").exists()
        {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        // No subscription with capacity is not a refusal: the switch pauses the task instead.
        let placed = self.choose_account(task, requested).await?;
        if let Some(placed) = &placed {
            self.open_credential(&placed.account)?;
        }
        self.isolation_join(&sandbox).await?;
        Ok(placed.and_then(|placed| placed.fallback))
    }

    /// Whether a switch of `task` was refused so recently that an automatic one waits.
    #[must_use]
    pub fn switch_cooling(&self, task: &TaskId) -> bool {
        let at = self.runtime(task).switch_refused_ms.load(Ordering::SeqCst);
        at > 0 && self.now().0 - at < SWITCH_COOLDOWN_MS
    }

    /// Whether a restart of `task`'s session on a newer Claude Code was refused so recently
    /// that the next one waits (SPEC-461 CCM-013).
    #[must_use]
    pub fn claude_move_cooling(&self, task: &TaskId) -> bool {
        let at = self
            .runtime(task)
            .claude_move_refused_ms
            .load(Ordering::SeqCst);
        at > 0 && self.now().0 - at < SWITCH_COOLDOWN_MS
    }

    /// Records and shows a switch that was refused before it ended the running session.
    async fn refuse_switch(
        &self,
        task: &TaskRecord,
        account: Option<&AccountId>,
        reason: &str,
        err: &AppError,
    ) {
        let message = err.public_message();
        // A restart for a newer Claude Code that cannot start now is no switch of subscriptions
        // and nothing a person must read: it has its own wait, is neither shown nor recorded,
        // and a later turn's end tries again (SPEC-461 CCM-013).
        if reason == crate::claude_code::MOVE_REASON {
            self.runtime(&task.id)
                .claude_move_refused_ms
                .store(self.now().0, Ordering::SeqCst);
            tracing::warn!(task = %task.id, error = %err, "restart on the newer Claude Code refused; the running session stays");
            return;
        }
        // A repeat of an automatic switch inside the cooldown is only logged: one record and
        // one notice per cooldown, not one per retry.
        let repeat = reason != "manual" && self.switch_cooling(&task.id);
        self.runtime(&task.id)
            .switch_refused_ms
            .store(self.now().0, Ordering::SeqCst);
        tracing::warn!(task = %task.id, error = %err, "switch refused; the running session stays");
        // At a usage limit the session stays at Claude Code's refusal (SPEC-464 UCL-007).
        self.limit_move_refused(task).await;
        if repeat {
            return;
        }
        self.runtime(&task.id)
            .hub
            .notice(if reason == "context_full" {
                format!(
                    "↻ A fresh session could not start, so this session stays as it is: {message}"
                )
            } else {
                format!(
                    "↻ Switching subscriptions was not possible, so this session stays as it is: \
                 {message}"
                )
            });
        let record = SwitchDecisionRecord {
            id: self.deps.crypto.new_id(),
            task_id: task.id.clone(),
            at: self.now(),
            reason: reason.to_owned(),
            inputs: serde_json::json!({
                "account": account.map(AccountId::as_str),
                "refused": message,
            })
            .to_string(),
            from_desc: self.account_desc(task.harness, None).await,
            to_desc: self.account_desc(task.harness, account).await,
            outcome: "refused".into(),
        };
        if let Err(err) = self.deps.store.insert_switch(&record).await {
            tracing::warn!(task = %task.id, error = %err, "the refused switch was not recorded");
        }
    }

    async fn account_desc(&self, harness: Harness, account: Option<&AccountId>) -> String {
        let label = match account {
            Some(id) => self
                .deps
                .store
                .account(id)
                .await
                .ok()
                .flatten()
                .map(|a| a.label),
            None => None,
        };
        match label {
            Some(l) => format!("{} ({l})", harness.display_name()),
            None => harness.display_name().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Review of SPEC-332 (round 3): a person's switch and a hand-over may come mid-turn, so they
    /// interrupt the turn first (the transcript keeps it); automatic switches, made while the agent
    /// is idle, terminate without an interrupt.
    #[test]
    fn a_hand_over_interrupts_like_a_persons_switch() {
        assert!(super::interrupts_first("manual"));
        assert!(super::interrupts_first("handover"));
        // SPEC-493 REV-007: the agent goes on after its reviewer was refused for the move.
        assert!(super::interrupts_first(crate::review::MOVE_FOR_FABLE));
        assert!(super::interrupts_first(crate::review::MOVE_FOR_ROOM));
        assert!(!super::interrupts_first(crate::review::REVIEW_RESUME));
        for reason in ["rotation", "limit", "context_full", "proactive"] {
            assert!(!super::interrupts_first(reason), "{reason}");
        }
    }
}
