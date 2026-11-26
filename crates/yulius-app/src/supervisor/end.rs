//! The end of a run: its exit or loss recorded once and settled, and runs lost while the
//! server was down reconciled.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::task::{Activity, AttemptState, StopReason, TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;

use super::{
    EndIntent, FlagPending, PendingGoal, STOP_ROUNDS, SwitchEnd, TakenRequest, TaskRuntime,
    could_not_end, opening_pending, opening_unsubmitted, person_typed_marked, personal_mcp_marked,
    replace_intent, save_screen, session_started_marked, typed_goal_awaits_opening,
};
use crate::attention::AttentionRecord;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, MetricsRecord, TaskMode, TaskRecord};
use crate::ports::harness::UsageSnapshot;
use crate::ports::isolation::LimitCounters;
use crate::ports::session::{RunExit, RunInspection};
use crate::ports::store::AttemptUpdate;

/// A close ends a session cleanly, including one whose switch the close overtook.
fn after_close(task: &TaskRecord) -> Option<TaskStatus> {
    task.status
        .apply(TaskEvent::Exited { normal: true })
        .or_else(|_| {
            task.status
                .apply(TaskEvent::SwitchCompleted)
                .and_then(|running| running.apply(TaskEvent::Exited { normal: true }))
        })
        .ok()
}

/// A stop reaps the run even if its reader beat the stop's change to Stopping.
fn after_cancel(task: &TaskRecord) -> Option<TaskStatus> {
    task.status
        .apply(TaskEvent::Reaped)
        .or_else(|_| {
            task.status
                .apply(TaskEvent::CancelRequested)
                .and_then(|stopping| stopping.apply(TaskEvent::Reaped))
        })
        .ok()
}

/// Counts a run ending as being recorded until dropped (SPEC-065 END-003).
struct Finalizing<'a>(&'a tokio::sync::watch::Sender<u32>);

impl<'a> Finalizing<'a> {
    fn start(count: &'a tokio::sync::watch::Sender<u32>) -> Self {
        count.send_modify(|n| *n += 1);
        Self(count)
    }
}

impl Drop for Finalizing<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// The statuses of a task whose run lives, starts or ends.
const LIVE_WITH_A_RUN: [TaskStatus; 4] = [
    TaskStatus::Starting,
    TaskStatus::Switching,
    TaskStatus::Stopping,
    TaskStatus::Running,
];

/// How long the restart of a reattached run waits for the replay of its transcript.
const REPLAY_WAIT: Duration = Duration::from_secs(30);

/// Exit codes of a process ended by SIGHUP, SIGINT, SIGKILL or SIGTERM as a shell — or
/// bubblewrap for its sandboxed child — reports them (128 + the signal); Claude Code's own
/// handlers exit with 129 (SIGHUP) and 143 (SIGTERM).
const KILLED_EXIT_CODES: [i32; 4] = [129, 130, 137, 143];
/// SIGHUP, SIGINT, SIGKILL and SIGTERM.
const KILL_SIGNALS: [i32; 4] = [1, 2, 9, 15];

/// The limit a sandbox was at while a run lived, from its counters when the run started and
/// when it ended (SPEC-268 PID-005): `pids` when forks were refused at a task limit (Claude Code
/// aborts when it cannot start a thread or a process), else `memory` when the OOM killer ended a
/// process.
#[must_use]
pub fn limit_cause(start: LimitCounters, end: LimitCounters) -> Option<&'static str> {
    if end.task_limit_hits > start.task_limit_hits {
        Some("pids")
    } else if end.oom_kills > start.oom_kills {
        Some("memory")
    } else {
        None
    }
}

/// The task event of a run that ended without Yulius ending it (F 2026-10-10: the task follows
/// its session). The person left the session — `/exit`, Ctrl+C twice, Ctrl+D or `/logout`, which
/// Claude Code reports through its `SessionEnd` hook — or the process exited cleanly: `closed`.
/// Ended by a signal from outside (`kill`, the OOM killer): `stopped(killed)`. Anything else:
/// `stopped(crashed)`.
#[must_use]
pub fn exit_event(exit: &RunExit, session_end: Option<&str>) -> TaskEvent {
    use yulius_contracts::hooks::session_end::{LOGOUT, PROMPT_INPUT_EXIT};
    let left = matches!(session_end, Some(PROMPT_INPUT_EXIT | LOGOUT));
    if !exit.spawn_failed && (left || exit.is_clean()) {
        TaskEvent::Exited { normal: true }
    } else if exit.signal.is_some_and(|s| KILL_SIGNALS.contains(&s))
        || exit.code.is_some_and(|c| KILLED_EXIT_CODES.contains(&c))
    {
        TaskEvent::Killed
    } else {
        TaskEvent::Exited { normal: false }
    }
}

/// The stored figures of a run's transcript snapshot: no turn and no active time are unknown,
/// not zero (SPEC-087; the live saves and the recount of old runs, SPEC-091).
pub(crate) fn metrics_of(snapshot: UsageSnapshot) -> MetricsRecord {
    MetricsRecord {
        model: snapshot.model,
        input_tokens: snapshot.tokens.input,
        output_tokens: snapshot.tokens.output,
        cache_read: snapshot.tokens.cache_read,
        cache_write: snapshot.tokens.cache_write,
        turns: u32::try_from(snapshot.turns).ok().filter(|t| *t > 0),
        active_ms: snapshot.active_ms.filter(|ms| *ms > 0),
        parts: snapshot.parts,
    }
}

impl App {
    /// Settles a live run on record that this server holds no connection to (ADR-0050).
    ///
    /// # Errors
    /// `Conflict` when the run could not be ended: it stays live.
    pub(super) async fn settle_unheld_run(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
    ) -> AppResult<()> {
        tracing::warn!(task = %task_id, run = %attempt_id, "settling a live run no server holds");
        self.resolve_lost_connection(task_id, attempt_id).await;
        match self.deps.store.attempt(attempt_id).await? {
            Some(run) if !run.state.is_terminal() => Err(AppError::Conflict(
                "the task's run could not be ended; try again".into(),
            )),
            _ => Ok(()),
        }
    }

    pub(super) async fn resolve_lost_connection(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
    ) {
        let Ok(Some(attempt)) = self.deps.store.attempt(attempt_id).await else {
            return;
        };
        // Its end is recorded already (the escalation ended it).
        if attempt.state.is_terminal() {
            return;
        }
        match self.deps.session_host.inspect(Path::new(&attempt.run_dir)) {
            RunInspection::Exited(exit) => self.finalize_attempt(task_id, attempt_id, exit).await,
            // The host or the agent may still run (an agent that outlived its killed host).
            RunInspection::Recorded {
                host_alive,
                child_alive,
                ..
            } if host_alive || child_alive => {
                crate::race::reached(
                    crate::race::Point::LostRunTermination,
                    Path::new(&attempt.run_dir),
                );
                self.terminate_then_mark_lost(task_id, &attempt, StopReason::Lost)
                    .await;
            }
            _ => self.mark_lost(task_id, attempt_id, StopReason::Lost).await,
        }
    }

    /// Ends a run that did not exit after its stop or close escalation, or whose host or agent
    /// lives on after the connection to it was lost or the server restarted: the host and agent
    /// are killed, and only once they are proven gone is the run recorded as lost (AGENTS.md: a
    /// timeout is not proof a subprocess died). When they cannot be killed the run stays live,
    /// which keeps the task from being deleted; the failure is logged, said in the task's terminal
    /// and flags a running task for a person (SPEC-065 END-001). Stopping the task again retries.
    pub(super) async fn terminate_then_mark_lost(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        reason: StopReason,
    ) {
        match self
            .deps
            .session_host
            .terminate(Path::new(&attempt.run_dir))
        {
            Ok(()) => self.mark_lost(task_id, &attempt.id, reason).await,
            Err(err) => {
                // A park that cannot end its session gives up: the session goes on as it was and
                // takes messages again (SPEC-512 PRK-003); its flag keeps it from parking again.
                let runtime = self.runtime(task_id);
                let park_given_up = runtime.intent.lock().is_ok_and(|mut intent| {
                    let parking = matches!(*intent, Some(EndIntent::Park));
                    if parking {
                        *intent = None;
                        if let Ok(mut kept) = runtime.parked_activity.lock() {
                            *kept = None;
                        }
                    }
                    parking
                });
                let action = if park_given_up {
                    "Nothing was lost: the task goes on in this session. Stop it to end it."
                } else {
                    "Stop the task again to retry."
                };
                self.run_stays_live(task_id, attempt, &err, action).await;
            }
        }
    }

    /// A run whose host could not be terminated stays live: logged, said in the task's terminal,
    /// and a running task is flagged for a person with `action` (SPEC-065 END-001).
    pub(super) async fn run_stays_live(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        err: &AppError,
        action: &str,
    ) {
        tracing::warn!(task = %task_id, error = %err, "the run's host could not be terminated; the run stays live");
        let runtime = self.runtime(task_id);
        let _pending = {
            let _gate = runtime.end_gate.lock().await;
            if let Ok(mut failed) = runtime.end_failed.lock() {
                *failed = Some((attempt.id.clone(), err.to_string()));
            }
            FlagPending::raise(&runtime.end_flag_pending)
        };
        crate::race::reached(
            crate::race::Point::EndFailedRecorded,
            Path::new(task_id.as_str()),
        );
        let problem = could_not_end(&err.to_string());
        runtime.hub.notice(format!("{problem} {action}"));
        self.flag_task(task_id, AttentionRecord::yulius(&problem, action))
            .await;
        crate::race::reached(
            crate::race::Point::EndFailedFlagged,
            Path::new(task_id.as_str()),
        );
        // From now on the stored flag says whether the session's writes are held (`_pending`
        // drops, also when this is cancelled or panics).
    }

    /// Publish the attached attempt's final grid before its terminal state becomes visible.
    /// Exit and loss can race. The second observer checks the durable outcome under the
    /// same lock and leaves the first observer's file in place (SPEC-301 LRS-001).
    async fn record_terminal_attempt(
        &self,
        runtime: Option<&TaskRuntime>,
        id: &AttemptId,
        state: AttemptState,
        update: &AttemptUpdate,
        run: Option<&AttemptRecord>,
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let Some(runtime) = runtime else {
            return store.transition_attempt(id, state, update).await;
        };
        let _recording = runtime.recording_exit.lock().await;
        if store.attempt_state(id).await?.is_terminal() {
            return Ok(false);
        }
        if let Some(run) = run
            && runtime
                .screen_of
                .lock()
                .is_ok_and(|shown| shown.as_ref() == Some(id))
        {
            save_screen(&runtime.hub, Path::new(&run.run_dir));
        }
        store.transition_attempt(id, state, update).await
    }

    pub(super) async fn mark_lost(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        reason: StopReason,
    ) {
        // `existing_runtime`: a deleted task's runtime is not brought back.
        let runtime = self.existing_runtime(task_id);
        let _finalizing = runtime.as_ref().map(|rt| Finalizing::start(&rt.finalizing));
        let terminal_cleanup = runtime
            .as_ref()
            .map(|rt| Finalizing::start(&rt.terminal_cleanup));
        let now = self.now();
        let update = AttemptUpdate {
            ended_at: Some(now),
            exit_reason: Some("lost".into()),
            ..AttemptUpdate::default()
        };
        let run = self.deps.store.attempt(attempt_id).await.ok().flatten();
        let first = self
            .record_terminal_attempt(
                runtime.as_deref(),
                attempt_id,
                AttemptState::Lost,
                &update,
                run.as_ref(),
            )
            .await
            .unwrap_or(false);
        if !first {
            return; // another observer already recorded the outcome
        }
        crate::race::reached(
            crate::race::Point::LossRecorded,
            Path::new(task_id.as_str()),
        );
        // A lost Codex run's processes are gone too: its refresh is kept and its file goes, as
        // after a run that ended (SPEC-503 CDX-006).
        self.codex_run_ended(task_id, attempt_id).await;
        self.antigravity_run_ended(attempt_id).await;
        let mut reason = reason;
        let mut request = None;
        if let Some(rt) = &runtime
            && let Ok(mut intent) = rt.intent.lock()
        {
            // The loss of a run a switch gave up is that switch's, whoever saw it first: the
            // run's reader sees its host go too (SPEC-483 SWT-002).
            if matches!(*intent, Some(EndIntent::SwitchTimeout)) {
                reason = StopReason::SwitchTimeout;
            }
            let taken = rt.take_reader_request(attempt_id);
            // A current intent takes precedence. Only a park changes the task outcome
            // when the loss recorder consumes the current intent directly (SPEC-512).
            request = match *intent {
                Some(EndIntent::Park) => Some(TakenRequest::Park),
                None => taken,
                _ => None,
            };
            // No exit will consume it: the next run must not inherit it.
            replace_intent(&mut intent, None, SwitchEnd::Lost);
        }
        match request {
            Some(TakenRequest::Close) => {
                self.settle_ended(task_id, attempt_id, now, after_close)
                    .await
            }
            Some(TakenRequest::Cancel) => {
                self.settle_ended(task_id, attempt_id, now, after_cancel)
                    .await
            }
            Some(TakenRequest::Park) => self.settle_parked(task_id, attempt_id, now).await,
            None => {
                self.settle_ended(task_id, attempt_id, now, |task| {
                    task.status.after_loss(task.activity, reason).ok()
                })
                .await;
            }
        }
        self.end_pause(task_id, "ended").await;
        if let Some(rt) = &runtime {
            rt.verdict_stop_ms.clear();
            rt.watcher_alive.store(false, Ordering::SeqCst);
            *rt.run.lock().await = None;
            // What the store recorded, whoever won the transition.
            let ended = match self.deps.store.task(task_id).await {
                Ok(Some(task)) if task.status.is_resting() || task.status == TaskStatus::Parked => {
                    task.status
                }
                _ => TaskStatus::Stopped(reason),
            };
            rt.hub.exit(ended.as_str(), None);
        }
        drop(terminal_cleanup);
        self.publish_task(task_id).await;
        self.advance_chains(task_id).await;
        self.drain_queue().await;
    }

    /// Moves the task whose run ended to `next` of the status read (`None`: no change). A change
    /// that loses to a concurrent one (a stop landing meanwhile) is made again from the status read
    /// anew, so an ended run never leaves its task live (SPEC-250 STW-005); the rounds are bounded
    /// like a stop's.
    async fn settle_ended(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        now: Timestamp,
        next: impl Fn(&TaskRecord) -> Option<TaskStatus>,
    ) {
        for _ in 0..STOP_ROUNDS {
            let Ok(Some(task)) = self.deps.store.task(task_id).await else {
                return;
            };
            let Some(to) = next(&task) else {
                if task.status.is_resting() {
                    self.clear_settled_park(task_id, attempt_id);
                }
                return;
            };
            crate::race::reached(crate::race::Point::EndDecided, Path::new(task_id.as_str()));
            match self
                .deps
                .store
                .transition_task(task_id, Some(task.status), to, None, now)
                .await
            {
                Ok(false) => {}
                Ok(true) => {
                    self.clear_settled_park(task_id, attempt_id);
                    return;
                }
                Err(err) => {
                    tracing::warn!(task = %task_id, error = %err, "the ended run's task was not moved");
                    return;
                }
            }
        }
        tracing::warn!(task = %task_id, "the ended run's task kept changing; left as it is");
    }

    /// Records the end of a run exactly once and moves the task to its next state.
    pub(super) async fn finalize_attempt(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        exit: RunExit,
    ) {
        let store = &self.deps.store;
        let runtime = self.runtime(task_id);
        let _finalizing = Finalizing::start(&runtime.finalizing);
        let terminal_cleanup = Finalizing::start(&runtime.terminal_cleanup);
        crate::race::reached(
            crate::race::Point::RunEndTaking,
            Path::new(task_id.as_str()),
        );
        // Escalation may already have settled this attempt and admitted its replacement.
        // Only a still-live attempt can consume the current session's end intent.
        match store.attempt_state(attempt_id).await {
            Ok(state) if state.is_terminal() => return,
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, attempt = %attempt_id, error = %err, "could not read the ending attempt state");
                return;
            }
        }
        let intent = runtime.intent.lock().ok().and_then(|mut g| {
            let intent = g.take();
            let request = match &intent {
                Some(EndIntent::Park) => Some(TakenRequest::Park),
                Some(EndIntent::Close) => Some(TakenRequest::Close),
                Some(EndIntent::Cancel) => Some(TakenRequest::Cancel),
                _ => None,
            };
            if let Some(request) = request
                && let Ok(mut taken) = runtime.taken_request.lock()
            {
                *taken = Some((attempt_id.clone(), request));
            }
            intent
        });
        crate::race::reached(crate::race::Point::RunEndTaken, Path::new(task_id.as_str()));
        let session_end = runtime.session_end.lock().ok().and_then(|end| end.clone());
        let state = if intent.is_some() {
            AttemptState::Killed
        } else {
            AttemptState::Exited
        };
        // A run that ended by itself, not cleanly: was the sandbox at a limit meanwhile?
        let start = runtime.start_counters(attempt_id);
        let unexpected = intent.is_none()
            && !matches!(
                exit_event(&exit, session_end.as_deref()),
                TaskEvent::Exited { normal: true }
            );
        let cause = match start {
            Some(start) if unexpected => match store.task(task_id).await {
                Ok(Some(task)) => self
                    .limit_counters(&task.sandbox_id)
                    .await
                    .and_then(|end| limit_cause(start, end)),
                _ => None,
            },
            _ => None,
        };
        let now = self.now();
        let update = AttemptUpdate {
            ended_at: Some(now),
            exit_code: exit.code,
            exit_reason: Some(match (&intent, exit.spawn_failed) {
                (_, true) => "spawn_failed".into(),
                (Some(EndIntent::Cancel), _) => "cancelled".into(),
                (Some(EndIntent::Close), _) => "closed".into(),
                (Some(EndIntent::Park), _) => "parked".into(),
                (Some(EndIntent::Switch(_)), _) => "switched".into(),
                (Some(EndIntent::SwitchTimeout), _) => "switch_timeout".into(),
                (None, _) if exit.is_clean() => "exited".into(),
                (None, _) => match cause {
                    Some(cause) => format!(
                        "exited code={:?} signal={:?} limit={cause}",
                        exit.code, exit.signal
                    ),
                    None => format!("exited code={:?} signal={:?}", exit.code, exit.signal),
                },
            }),
            ..AttemptUpdate::default()
        };
        // Optional metadata may be unreadable on a historical run. Its confirmed exit still counts.
        let run = store.attempt(attempt_id).await.ok().flatten();
        let first = self
            .record_terminal_attempt(Some(&runtime), attempt_id, state, &update, run.as_ref())
            .await
            .unwrap_or(false);
        if !first {
            return; // another observer already recorded the outcome
        }
        crate::race::reached(crate::race::Point::RunReaped, Path::new(task_id.as_str()));
        // The losing reader leaves the winner's screen and session state intact.
        if let Ok(mut end) = runtime.session_end.lock() {
            *end = None;
        }
        runtime.verdict_stop_ms.clear();
        runtime.watcher_alive.store(false, Ordering::SeqCst);
        let session_started = run
            .as_ref()
            .is_none_or(|attempt| session_started_marked(Path::new(&attempt.run_dir)));
        // This reader won and settles its request below.
        runtime.take_reader_request(attempt_id);
        runtime.clear_start_counters(attempt_id);
        // A session restarted on a newer Claude Code that ended by itself before it started
        // there: that version gets no other session (SPEC-461 CCM-013).
        self.claude_code_run_ended(task_id, attempt_id, session_started, intent.is_none())
            .await;
        self.codex_run_ended(task_id, attempt_id).await;
        self.antigravity_run_ended(attempt_id).await;
        // The snapshot is the run's once its watcher read the run's transcript to its end;
        // otherwise it is empty or partial, and the transcript is counted (SPEC-143 RDM-001).
        let read = runtime
            .usage_read
            .lock()
            .is_ok_and(|read| read.as_ref() == Some(attempt_id));
        if read {
            self.save_metrics(task_id, attempt_id, &runtime).await;
        } else {
            self.save_counted_metrics(attempt_id).await;
        }
        {
            let mut run = runtime.run.lock().await;
            if run.as_ref().is_some_and(|r| &r.attempt_id == attempt_id) {
                *run = None;
            }
        }
        match intent {
            Some(EndIntent::Switch(done)) => {
                let _ = done.send(SwitchEnd::Ended);
                return; // the switch controller launches the next run and drains the queue
            }
            Some(EndIntent::SwitchTimeout) => {
                // The run ended while the switch that gave it up was ending it: the task rests
                // as it does when that switch records the loss itself, from `Switching` or, after
                // a refused kill, from `Running` (SPEC-483 SWT-002).
                self.settle_ended(task_id, attempt_id, now, |task| {
                    task.status
                        .apply(TaskEvent::Lost(StopReason::SwitchTimeout))
                        .ok()
                })
                .await;
            }
            Some(EndIntent::Close) => {
                self.settle_ended(task_id, attempt_id, now, after_close)
                    .await
            }
            Some(EndIntent::Park) => self.settle_parked(task_id, attempt_id, now).await,
            Some(EndIntent::Cancel) => {
                self.settle_ended(task_id, attempt_id, now, after_cancel)
                    .await
            }
            None => {
                // A run that ends by itself while a switch that has not claimed its end yet
                // holds the task `Switching` ends the task as from `Running`: the switch finds
                // it resting and gives way (SPEC-483 SWT-002).
                let event = exit_event(&exit, session_end.as_deref());
                self.settle_ended(task_id, attempt_id, now, |task| {
                    task.status
                        .apply(event)
                        .or_else(|_| {
                            task.status
                                .apply(TaskEvent::SwitchCompleted)
                                .and_then(|running| running.apply(event))
                        })
                        .ok()
                })
                .await;
            }
        }
        // A paused run that ended (the user left the session, or it was stopped) is no longer
        // paused.
        self.end_pause(task_id, "ended").await;
        let status = store
            .task(task_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "stopped_lost".into(), |t| t.status.as_str().to_owned());
        runtime.hub.exit_at(&status, exit.code, cause);
        drop(terminal_cleanup);
        self.publish_task(task_id).await;
        if let Ok(Some(task)) = self.deps.store.task(task_id).await
            && let Err(err) = self.refresh_onboarding(&task.sandbox_id).await
        {
            tracing::warn!(error = %err, task = %task_id, "onboarding refresh failed");
        }
        // Tasks chained after this one start (it is Done) or need a person (it ended).
        self.advance_chains(task_id).await;
        self.drain_queue().await;
    }

    /// Stores a run's metrics: while it goes (live, #31) and once more when it ends. The store
    /// upserts by attempt, so the last save wins and nothing is counted twice. Every figure comes
    /// from the run's transcript, which a restart reads again, so a save after a restart never
    /// replaces them with less (SPEC-087 MET-002).
    pub(super) async fn save_metrics(
        &self,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        runtime: &TaskRuntime,
    ) {
        let snapshot = runtime.usage.lock().map(|u| u.clone()).unwrap_or_default();
        let metrics = metrics_of(snapshot);
        let _ = self
            .deps
            .store
            .put_metrics(attempt_id, task_id, &metrics, self.now())
            .await;
    }

    /// Reconciles recorded live runs after a server start (SPEC-002 §7.1). Never respawns.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile(self: &Arc<Self>) -> AppResult<()> {
        self.reconcile_lost().await.map(|_| ())
    }

    /// [`App::reconcile`], returning the tasks whose runs were lost, each with the status it had:
    /// what the first start after a move brings back (SPEC-460 MIG-050).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn reconcile_lost(self: &Arc<Self>) -> AppResult<Vec<(TaskId, TaskStatus)>> {
        let store = &self.deps.store;
        // What the server before this one left live. Only these can be without a run for its
        // death: a task this server starts meanwhile (the queue does, as soon as a lost run frees
        // a slot) is `Starting` with no run on record until its launch records one (SPEC-458
        // RUN-001, #439).
        let left: HashSet<TaskId> = store
            .tasks_with_status(&LIVE_WITH_A_RUN)
            .await?
            .into_iter()
            .map(|task| task.id)
            .collect();
        let mut held_again = HashSet::new();
        let mut lost = Vec::new();
        for attempt in store.live_attempts().await? {
            let host_token: Option<[u8; 32]> = self
                .deps
                .crypto
                .open(&attempt.host_token)
                .ok()
                .and_then(|b| b.try_into().ok());
            // The run's transcript so far is history: its watcher catches up on it (TSK-014).
            // Until it did, what waits in Claude Code's queue is not known (SPEC-471 CCQ-001).
            let runtime = self.runtime(&attempt.task_id);
            let recovery = runtime.reattach_move.clone().lock_owned().await;
            runtime.replay_transcript.store(true, Ordering::SeqCst);
            runtime.queue_unread.store(true, Ordering::SeqCst);
            // Before the run is attached, so no message is typed meanwhile (SPEC-230): when it
            // was spawned, and whether its session started (a SessionStart this server did not
            // see left its marker).
            runtime
                .spawned_ms
                .store(attempt.started_at.0, Ordering::SeqCst);
            // The opening turn of a session whose goal is still to be typed (SPEC-483 RST-002):
            // its Stop ends it as it is, as before the restart, and the goal's watch waits for it.
            if typed_goal_awaits_opening(Path::new(&attempt.run_dir)) {
                runtime.opening_turn.store(true, Ordering::SeqCst);
            }
            // Only ever set here: a SessionStart hook served meanwhile is not undone.
            if session_started_marked(Path::new(&attempt.run_dir)) {
                runtime.session_started.store(true, Ordering::SeqCst);
            }
            // A draft a person typed before this server started may still sit in the prompt
            // box: its mark says since when (SPEC-480 DRF-009). A keystroke this server saw is
            // newer and stays.
            // Never later than now: a time ahead of the clock would never grow old.
            {
                let mut mark = runtime.draft_mark.lock().await;
                let drafted = person_typed_marked(Path::new(&attempt.run_dir)).min(self.now().0);
                if drafted > 0 {
                    runtime.typed_at_ms.fetch_max(drafted, Ordering::SeqCst);
                    mark.written = true;
                    mark.recovered_at = Some(self.now().0);
                    if runtime.typed_seq.load(Ordering::SeqCst) == 0 {
                        runtime
                            .viewer_prompt
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .restored();
                    }
                }
            }
            if let Some(task) = store.task(&attempt.task_id).await? {
                self.initialize_run_goal(&task, &attempt, false).await;
            }
            let reattached = match host_token {
                Some(token) => self
                    .attach_run(&attempt.task_id, &attempt, &token)
                    .await
                    .is_ok(),
                None => false,
            };
            if !reattached {
                let runtime = self.runtime(&attempt.task_id);
                runtime.replay_transcript.store(false, Ordering::SeqCst);
                runtime.queue_unread.store(false, Ordering::SeqCst);
            }
            if reattached {
                tracing::info!(task = %attempt.task_id, "reattached live run");
                // An idle run's screen is kept now, from the host's snapshot: its next settled
                // turn may never come before a reboot (SPEC-229 RBT-002).
                if let Ok(Some(task)) = store.task(&attempt.task_id).await
                    && task.status == TaskStatus::Running
                    && task.activity.is_some_and(Activity::is_idle)
                {
                    save_screen(
                        &self.runtime(&attempt.task_id).hub,
                        Path::new(&attempt.run_dir),
                    );
                }
                if let Ok(Some(task)) = store.task(&attempt.task_id).await
                    && matches!(task.status, TaskStatus::Starting | TaskStatus::Switching)
                {
                    let _ = store
                        .transition_task(
                            &task.id,
                            Some(task.status),
                            TaskStatus::Running,
                            Some(Activity::Waiting),
                            self.now(),
                        )
                        .await;
                }
                // The old server was typing the run's first message (or about to): how much of
                // it reached the prompt box is unknown, so a person finishes it (SPEC-068
                // OPN-002). A submitted one that a hold keeps goes on when the hold ends.
                if opening_unsubmitted(&attempt.run_dir) {
                    if matches!(store.open_pause(&attempt.task_id).await, Ok(None)) {
                        self.opening_never_sent(&attempt.task_id).await;
                    }
                } else if opening_pending(&attempt.run_dir)
                    && matches!(store.open_pause(&attempt.task_id).await, Ok(None))
                    && matches!(
                        store.task(&attempt.task_id).await,
                        Ok(Some(task)) if task.mode == TaskMode::Goal
                    )
                {
                    // A goal run an older server started typed its first message before the goal,
                    // which this version never types after it (ADR-0095): a person sets it. One
                    // held for capacity starts afresh with its launch goal when capacity returns.
                    self.goal_not_taken(
                        &attempt.task_id,
                        "Yulius restarted before it set this task's goal",
                    )
                    .await;
                }
                // A launch goal the old server was watching is watched again: the transcript
                // replay says whether Claude Code recorded it meanwhile (SPEC-146 GWR-002).
                let run_dir = Path::new(&attempt.run_dir);
                if PendingGoal::Launch.pending(run_dir) {
                    self.spawn_goal_watch(
                        &attempt.task_id,
                        &attempt.id,
                        run_dir,
                        PendingGoal::Launch,
                    );
                }
                // A goal the old server still had to type has no typist any more: whether
                // Claude Code recorded it before the restart is watched, and a task left
                // without a goal is flagged (SPEC-483 RST-002).
                if PendingGoal::Typed.pending(run_dir) {
                    self.spawn_goal_watch(
                        &attempt.task_id,
                        &attempt.id,
                        run_dir,
                        PendingGoal::Typed,
                    );
                }
                // Whose personal MCP servers the run holds (SPEC-483 RST-001): never over what
                // this server recorded itself.
                let marked = personal_mcp_marked(run_dir);
                if let Ok(mut held) = self.runtime(&attempt.task_id).personal_mcp_of.lock()
                    && held.is_none()
                {
                    held.clone_from(&marked);
                }
                // A hand-over the old server did not finish (its restart of the run was cut):
                // the run still holds the previous owner's servers, and is restarted now.
                if let Some(marked) = marked
                    && let Ok(Some(task)) = store.task(&attempt.task_id).await
                    && crate::vault::Holder::of_task(&task).person() != Some(&marked)
                {
                    let app = Arc::clone(self);
                    let (task_id, account) = (task.id.clone(), attempt.account_id.clone());
                    tokio::spawn(async move {
                        // How the run is resumed (past a usage limit, with the workflows it
                        // left) is read from its transcript: the restart waits for the replay.
                        let _recovery = recovery;
                        let asked = tokio::time::Instant::now();
                        while app.runtime(&task_id).queue_unread.load(Ordering::SeqCst)
                            && asked.elapsed() < REPLAY_WAIT
                        {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        app.restart_for_new_owner(&task_id, account).await;
                    });
                }
                held_again.insert(attempt.task_id.clone());
                continue;
            }
            let inspection = self.deps.session_host.inspect(Path::new(&attempt.run_dir));
            if !matches!(inspection, RunInspection::Exited(_))
                && let Ok(Some(task)) = store.task(&attempt.task_id).await
            {
                lost.push((task.id, task.status));
            }
            match inspection {
                // It ended while Yulius was down: no watcher of this process read it, so its
                // figures are counted from its transcript (SPEC-143 RDM-001).
                RunInspection::Exited(exit) => {
                    // A task the dead server left switching lost that switch with it, and the
                    // run found ended is most likely the one the switch had asked to end: the
                    // task is lost to the restart, as the sweep below rests one with no run,
                    // before the run's end is recorded (which ends a switching task as a
                    // session that left by itself, SPEC-483 SWT-002).
                    if let Ok(Some(task)) = store.task(&attempt.task_id).await
                        && task.status == TaskStatus::Switching
                    {
                        let next = task
                            .status
                            .after_loss(task.activity, StopReason::ServerLost)
                            .unwrap_or(TaskStatus::Stopped(StopReason::ServerLost));
                        let rested = store
                            .transition_task(&task.id, Some(task.status), next, None, self.now())
                            .await;
                        if matches!(rested, Ok(true)) {
                            lost.push((task.id, task.status));
                        }
                    }
                    self.finalize_attempt(&attempt.task_id, &attempt.id, exit)
                        .await
                }
                RunInspection::Recorded {
                    host_alive,
                    child_alive,
                    ..
                } if host_alive || child_alive => {
                    self.terminate_then_mark_lost(
                        &attempt.task_id,
                        &attempt,
                        StopReason::ServerLost,
                    )
                    .await;
                }
                RunInspection::Recorded { .. } => {
                    self.mark_lost(&attempt.task_id, &attempt.id, StopReason::ServerLost)
                        .await;
                }
                RunInspection::Missing => {
                    self.mark_lost(&attempt.task_id, &attempt.id, StopReason::ServerLost)
                        .await
                }
            }
        }
        crate::race::reached(crate::race::Point::ReconcileSweep, &self.config.data_dir);
        // Tasks that claim to be starting/switching/stopping without a live attempt: those the
        // dead server left so. One whose run this server holds again, or which it is launching,
        // is this server's: its own code moves it on.
        for task in store.tasks_with_status(&LIVE_WITH_A_RUN).await? {
            if !left.contains(&task.id) || held_again.contains(&task.id) {
                continue;
            }
            // Asked before the run: a launch records its run before it stops being the launch.
            let launching = self
                .supervisor
                .launching
                .lock()
                .is_ok_and(|launching| launching.as_ref() == Some(&task.id));
            if !launching && store.live_attempt(&task.id).await?.is_none() {
                let next = task
                    .status
                    .after_loss(task.activity, StopReason::ServerLost)
                    .unwrap_or(TaskStatus::Stopped(StopReason::ServerLost));
                let rested = store
                    .transition_task(&task.id, Some(task.status), next, None, self.now())
                    .await;
                if matches!(rested, Ok(true)) {
                    lost.push((task.id, task.status));
                }
            }
        }
        self.drain_queue().await;
        // A server that stopped between a Codex run's end and the removal of its `auth.json`
        // left the file: no run of this server uses one yet (SPEC-503 CDX-006).
        self.sweep_codex_homes().await;
        Ok(lost)
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-268 PID-005: refused forks win over OOM kills; counters that did not rise say nothing.
    #[test]
    fn a_rise_of_a_limit_counter_is_the_runs_cause() {
        use crate::ports::isolation::LimitCounters;
        let at = |task_limit_hits, oom_kills| LimitCounters {
            task_limit_hits,
            oom_kills,
        };
        assert_eq!(super::limit_cause(at(3, 1), at(3, 1)), None);
        assert_eq!(super::limit_cause(at(3, 1), at(4, 1)), Some("pids"));
        assert_eq!(super::limit_cause(at(3, 1), at(4, 2)), Some("pids"));
        assert_eq!(super::limit_cause(at(3, 1), at(3, 2)), Some("memory"));
        // A group made again (counts from 0) is no rise.
        assert_eq!(super::limit_cause(at(3, 1), at(0, 0)), None);
    }

    /// F 2026-10-10: every way a session ends maps to one task event. Leaving the session
    /// (`SessionEnd` `prompt_input_exit`/`logout`) is a normal close whatever the exit code; a
    /// signal from outside (or Claude Code's 128 + signal exit) is a kill; the rest is a crash.
    #[test]
    fn session_ends_map_to_one_task_event() {
        use yulius_domain::task::TaskEvent;
        let exit = |code: Option<i32>, signal: Option<i32>| crate::ports::session::RunExit {
            code,
            signal,
            spawn_failed: false,
            usage: None,
        };
        let normal = TaskEvent::Exited { normal: true };
        let crashed = TaskEvent::Exited { normal: false };
        assert_eq!(super::exit_event(&exit(Some(0), None), None), normal);
        assert_eq!(
            super::exit_event(&exit(Some(1), None), Some("prompt_input_exit")),
            normal
        );
        assert_eq!(
            super::exit_event(&exit(Some(1), None), Some("logout")),
            normal
        );
        assert_eq!(super::exit_event(&exit(Some(3), None), None), crashed);
        assert_eq!(
            super::exit_event(&exit(Some(3), None), Some("other")),
            crashed
        );
        assert_eq!(super::exit_event(&exit(None, Some(11)), None), crashed);
        for killed in [
            exit(Some(143), None),
            exit(Some(137), None),
            exit(Some(129), None),
        ] {
            assert_eq!(super::exit_event(&killed, Some("other")), TaskEvent::Killed);
        }
        assert_eq!(
            super::exit_event(&exit(None, Some(9)), None),
            TaskEvent::Killed
        );
        assert_eq!(
            super::exit_event(&exit(None, Some(15)), None),
            TaskEvent::Killed
        );
        let spawn = crate::ports::session::RunExit {
            code: Some(0),
            signal: None,
            spawn_failed: true,
            usage: None,
        };
        assert_eq!(
            super::exit_event(&spawn, Some("prompt_input_exit")),
            crashed
        );
    }
}
