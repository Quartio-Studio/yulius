//! A session whose context is full (SPEC-060, #213): the API answers every prompt with "Prompt
//! is too long" and no Stop hook ends the turn. Yulius ends the turn itself, compacts the session
//! once with a focus built from the task, continues the task in a fresh session with a handoff
//! when compacting does not help, and asks a person when the fresh session is full too.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_domain::context_full::TaskFocus;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::request::Request;
use yulius_domain::steps::StepStatus;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::model::{AttemptRecord, TaskRecord};

/// Longest Yulius waits for the compaction it asked for before it counts as failed (CTX-004).
pub const COMPACT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Longest Yulius waits for a quiet terminal before it types.
const TYPE_WAIT: Duration = Duration::from_secs(30);

/// Output quiet this long means the TUI waits for input.
const TYPE_QUIET: Duration = Duration::from_millis(800);

/// The audit action of every step of a recovery.
const AUDIT_ACTION: &str = "task.context_full";

/// Where a task's recovery from a full context stands, kept by its runtime across runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum ContextEpisode {
    /// No full context since the last successful reply.
    #[default]
    Clear,
    /// `/compact` was typed into `attempt` at `since` (ms); its outcome is not known yet.
    Compacting {
        /// The attempt that was compacted.
        attempt: AttemptId,
        /// When the compaction was asked for (ms), which identifies it.
        since: i64,
    },
    /// The compaction succeeded and the agent was told to carry on.
    Compacted,
    /// The task continues in a fresh session; `from` is the full one, whose last lines do not
    /// count.
    Restarted {
        /// The attempt that ran out of context.
        from: AttemptId,
    },
    /// Neither helped: the task waits for a person.
    GaveUp,
}

/// What a full context leads to, given the episode so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Compact,
    Restart,
    /// Into [`ContextEpisode::GaveUp`]: said once.
    GiveUp,
    /// Already given up: the task is flagged again, silently.
    Reflag,
    Nothing,
}

/// Why the task needs a person when the fresh session is full too.
const STILL_FULL: &str = "the context is still full after compacting and a fresh session";

impl App {
    /// The API refused the attempt's prompt as too long (CTX-002..005).
    pub(crate) async fn context_full_observed(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
    ) {
        let runtime = self.runtime(task_id);
        // No Stop hook ends a turn the API refused: the turn ends here.
        runtime.end_turn(self.now());
        let next = {
            let Ok(mut episode) = runtime.context.lock() else {
                return;
            };
            let next = match &*episode {
                ContextEpisode::Clear => Next::Compact,
                // The compaction was cut short (the run ended): the continued session is new.
                ContextEpisode::Compacting { attempt: asked, .. } if *asked != attempt.id => {
                    Next::Compact
                }
                // A prompt sent before the compaction meets the full context too.
                ContextEpisode::Compacting { .. } => Next::Nothing,
                ContextEpisode::Compacted => Next::Restart,
                ContextEpisode::Restarted { from } if *from == attempt.id => Next::Nothing,
                ContextEpisode::Restarted { .. } => Next::GiveUp,
                ContextEpisode::GaveUp => Next::Reflag,
            };
            match next {
                Next::Compact => {
                    *episode = ContextEpisode::Compacting {
                        attempt: attempt.id.clone(),
                        since: self.now().0,
                    };
                }
                Next::Restart => {
                    *episode = ContextEpisode::Restarted {
                        from: attempt.id.clone(),
                    };
                }
                Next::GiveUp => *episode = ContextEpisode::GaveUp,
                Next::Reflag | Next::Nothing => {}
            }
            next
        };
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        match next {
            Next::Compact => {
                self.idle_after_full_context(&task).await;
                self.compact(&task, attempt).await;
            }
            Next::Restart => {
                self.idle_after_full_context(&task).await;
                self.restart_after_full_context(
                    &task,
                    attempt,
                    "the context is full again after compacting",
                );
            }
            Next::GiveUp => self.give_up_on_full_context(&task, STILL_FULL, true).await,
            Next::Reflag => self.give_up_on_full_context(&task, STILL_FULL, false).await,
            Next::Nothing => self.idle_after_full_context(&task).await,
        }
    }

    /// Claude Code compacted the conversation: the compaction Yulius asked for succeeded, and the
    /// idle session is told to carry on.
    pub(crate) async fn compacted_observed(self: &Arc<Self>, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        let attempt = {
            let Ok(mut episode) = runtime.context.lock() else {
                return;
            };
            let ContextEpisode::Compacting { attempt, .. } = &*episode else {
                return;
            };
            let attempt = attempt.clone();
            *episode = ContextEpisode::Compacted;
            attempt
        };
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        tracing::info!(task = %task_id, "the full context was compacted");
        runtime
            .hub
            .notice("✓ Yulius compacted the conversation; the task goes on.");
        self.audit_context(task_id, "compacted").await;
        let line = self.task_focus(&task).await.after_compaction();
        let asked_ms = self.now().0;
        let app = Arc::clone(self);
        let task_id = task_id.clone();
        tokio::spawn(async move {
            let compacted = |e: &ContextEpisode| *e == ContextEpisode::Compacted;
            app.type_line(&task_id, &attempt, &line, asked_ms, compacted)
                .await;
        });
    }

    /// The compaction Yulius asked for failed (the CLI said so).
    pub(crate) async fn compact_failed_observed(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
    ) {
        self.compaction_failed(task_id, attempt, None, "the compaction failed")
            .await;
    }

    /// The agent replied after the full context: the episode is over (CTX-006).
    pub(crate) fn context_replied(&self, task_id: &TaskId, attempt: &AttemptId) {
        let runtime = self.runtime(task_id);
        let Ok(mut episode) = runtime.context.lock() else {
            return;
        };
        let over = match &*episode {
            ContextEpisode::Clear => false,
            // The full session's last lines are not the fresh session's reply.
            ContextEpisode::Restarted { from } => from != attempt,
            _ => true,
        };
        if over {
            tracing::info!(task = %task_id, "the session works again after its context was full");
            *episode = ContextEpisode::Clear;
        }
    }

    /// The compaction of `attempt` failed (`since`: only the compaction asked for then): the
    /// task continues in a fresh session.
    async fn compaction_failed(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        since: Option<i64>,
        why: &str,
    ) {
        let runtime = self.runtime(task_id);
        {
            let Ok(mut episode) = runtime.context.lock() else {
                return;
            };
            match &*episode {
                ContextEpisode::Compacting {
                    attempt: compacted,
                    since: asked,
                } if *compacted == attempt.id && since.is_none_or(|s| s == *asked) => {}
                _ => return,
            }
            *episode = ContextEpisode::Restarted {
                from: attempt.id.clone(),
            };
        }
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        self.restart_after_full_context(&task, attempt, why);
    }

    /// A turn the API refused leaves the task idle, not `working` (CTX-002).
    async fn idle_after_full_context(&self, task: &TaskRecord) {
        if task.activity == Some(Activity::Working)
            && let Err(err) = self.set_task_activity(task, Activity::Waiting).await
        {
            tracing::warn!(task = %task.id, error = %err, "could not settle a full session");
        }
    }

    /// Types `/compact <focus>` into the attempt and bounds the wait for its outcome (CTX-003).
    async fn compact(self: &Arc<Self>, task: &TaskRecord, attempt: &AttemptRecord) {
        tracing::warn!(task = %task.id, "the session's context is full; compacting it");
        self.runtime(&task.id).hub.notice(
            "⚠ The session's context is full (\"Prompt is too long\"). Yulius compacts the \
             conversation to keep the task going…",
        );
        self.audit_context(&task.id, "context full: compacting")
            .await;
        let command = self.task_focus(task).await.compact_command();
        let since = match self.runtime(&task.id).context.lock().map(|e| e.clone()) {
            Ok(ContextEpisode::Compacting { since, .. }) => since,
            _ => return,
        };
        let app = Arc::clone(self);
        let (task_id, attempt) = (task.id.clone(), attempt.clone());
        tokio::spawn(async move {
            let asked_now = move |e: &ContextEpisode| matches!(e, ContextEpisode::Compacting { since: s, .. } if *s == since);
            if !app
                .type_line(&task_id, &attempt.id, &command, since, asked_now)
                .await
            {
                // Not typed (the run ended, a person kept typing): the next refusal starts over.
                app.context_reset_if(&task_id, asked_now);
                return;
            }
            let asked = tokio::time::Instant::now();
            while asked.elapsed() < COMPACT_TIMEOUT {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if !app.attempt_live(&task_id, &attempt.id).await {
                    // The run ended before its compaction did: a continued session starts over.
                    app.context_reset_if(&task_id, asked_now);
                    return;
                }
                if !app.context_is(&task_id, asked_now) {
                    return;
                }
            }
            app.compaction_failed(
                &task_id,
                &attempt,
                Some(since),
                "no compaction within 10 minutes",
            )
            .await;
        });
    }

    /// Continues the task in a fresh session on the same subscription, opened with a handoff
    /// (CTX-004). Runs apart: the switch waits for the full session to end, and this is called
    /// from its transcript watcher.
    fn restart_after_full_context(
        self: &Arc<Self>,
        task: &TaskRecord,
        attempt: &AttemptRecord,
        why: &str,
    ) {
        tracing::warn!(task = %task.id, why, "compacting did not help; starting a fresh session");
        self.runtime(&task.id).hub.notice(format!(
            "⚠ Compacting did not help ({why}). Yulius continues the task in a fresh session with \
             a handoff…"
        ));
        let app = Arc::clone(self);
        let (task, account) = (task.clone(), attempt.account_id.clone());
        let why = why.to_owned();
        tokio::spawn(async move {
            app.audit_context(&task.id, &format!("{why}: fresh session"))
                .await;
            let handoff = app.task_focus(&task).await.handoff();
            let why = match app.restart_fresh(&task.id, account, &handoff).await {
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "no fresh session after a full context");
                    format!("a fresh session could not start: {}", err.public_message())
                }
                // No subscription had capacity: the task paused, and its resume would bring the
                // full session back, not the handoff.
                Ok(()) if app.task_status(&task.id).await == Some(TaskStatus::WaitingCapacity) => {
                    "no subscription has capacity for a fresh session; when one does, stop the \
                     task and start a new one from its request"
                        .to_owned()
                }
                Ok(()) => return,
            };
            // The full session stays (a refused switch), the task paused or it stopped: only a
            // person can go on. A task a person is stopping needs no word of it.
            if let Ok(mut episode) = app.runtime(&task.id).context.lock() {
                *episode = ContextEpisode::GaveUp;
            }
            if let Ok(Some(task)) = app.deps.store.task(&task.id).await
                && matches!(
                    task.status,
                    TaskStatus::Running | TaskStatus::WaitingCapacity
                )
            {
                app.give_up_on_full_context(&task, &why, true).await;
            }
        });
    }

    /// Recovery did not help: the task waits for a person, told `why` when `announce` (once per
    /// episode, CTX-005).
    async fn give_up_on_full_context(&self, task: &TaskRecord, why: &str, announce: bool) {
        if task.status == TaskStatus::Running
            && let Err(err) = self.set_task_activity(task, Activity::NeedsAttention).await
        {
            tracing::warn!(task = %task.id, error = %err, "could not flag a full session");
        }
        if !announce {
            return;
        }
        tracing::error!(task = %task.id, why, "a full session needs a person");
        self.runtime(&task.id).hub.notice(format!(
            "⚠ The task needs you: {why}. Find what fills it (a huge file, tool output or \
             instructions), then type `/compact` or `/clear` here, or stop and continue the task."
        ));
        self.audit_context(&task.id, &format!("needs attention: {why}"))
            .await;
    }

    /// What the focus, the wake-up line and the handoff say about the task.
    async fn task_focus(&self, task: &TaskRecord) -> TaskFocus {
        let steps_left = self
            .deps
            .store
            .steps(&task.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                !matches!(
                    StepStatus::parse(&s.status),
                    Some(StepStatus::Done | StepStatus::Skipped)
                )
            })
            .map(|s| s.title)
            .collect();
        let pull_request = match self.deps.store.delivery(&task.id).await {
            Ok(Some(delivery)) => delivery.pr_number,
            _ => None,
        };
        TaskFocus {
            number: task.number,
            title: task.title.clone(),
            goal: Request::parse(&task.prompt).text,
            branch: task.branch.clone(),
            pull_request,
            steps_left,
        }
    }

    async fn task_status(&self, task_id: &TaskId) -> Option<TaskStatus> {
        match self.deps.store.task(task_id).await {
            Ok(Some(task)) => Some(task.status),
            _ => None,
        }
    }

    /// Whether the task's episode is one `wanted` accepts.
    fn context_is(&self, task_id: &TaskId, wanted: impl Fn(&ContextEpisode) -> bool) -> bool {
        self.runtime(task_id)
            .context
            .lock()
            .is_ok_and(|episode| wanted(&episode))
    }

    /// Ends the task's episode if it is still one `stale` accepts.
    fn context_reset_if(&self, task_id: &TaskId, stale: impl Fn(&ContextEpisode) -> bool) {
        if let Ok(mut episode) = self.runtime(task_id).context.lock()
            && stale(&episode)
        {
            *episode = ContextEpisode::Clear;
        }
    }

    /// The session changed inside the TUI (`/clear`, `/resume`): a person took the session in
    /// hand, so its recovery is over (CTX-006).
    pub(crate) fn context_session_changed(&self, task_id: &TaskId) {
        self.context_reset_if(task_id, |episode| {
            // A CLI whose `/compact` changed the session would end recoveries here: said, so it
            // shows.
            if *episode != ContextEpisode::Clear {
                tracing::info!(task = %task_id, ?episode, "the session changed during a full-context recovery");
            }
            true
        });
    }

    /// A replay after a restart ended in a refusal (CTX-007): recovery starts, unless the task
    /// already waits for a person (the recovery before the restart gave up).
    pub(crate) async fn context_full_replayed(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
    ) {
        match self.deps.store.task(task_id).await {
            Ok(Some(task)) if task.activity != Some(Activity::NeedsAttention) => {
                self.context_full_observed(task_id, attempt).await;
            }
            _ => {}
        }
    }

    /// Types one line into the attempt's TUI once it waits for input, then a separate Enter,
    /// while the episode is one `wanted` accepts; `false` when it was not typed (the attempt is
    /// no longer live, the episode moved on, a person typed since `asked_ms` and owns the prompt
    /// box, the terminal never went quiet, the input failed).
    async fn type_line(
        &self,
        task_id: &TaskId,
        attempt: &AttemptId,
        line: &str,
        asked_ms: i64,
        wanted: impl Fn(&ContextEpisode) -> bool,
    ) -> bool {
        let runtime = self.runtime(task_id);
        let started = tokio::time::Instant::now();
        let mut last = runtime.hub.output_seq();
        let mut quiet_since = started;
        let mut ready = false;
        while !ready && started.elapsed() < TYPE_WAIT {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if !self.attempt_live(task_id, attempt).await {
                return false;
            }
            let seq = runtime.hub.output_seq();
            if seq != last {
                last = seq;
                quiet_since = tokio::time::Instant::now();
            }
            // A person who typed since: the prompt box (and any draft in it) is theirs.
            if runtime.typed_at_ms.load(Ordering::SeqCst) > asked_ms {
                tracing::info!(task = %task_id, "a person types; Yulius does not type into the session");
                return false;
            }
            ready = quiet_since.elapsed() >= TYPE_QUIET;
        }
        if !ready {
            tracing::warn!(task = %task_id, "the terminal never went quiet; nothing typed for a full session");
            return false;
        }
        if !self.context_is(task_id, &wanted) || !self.attempt_live(task_id, attempt).await {
            return false;
        }
        if let Err(err) = self.send_input(task_id, line.as_bytes()).await {
            tracing::warn!(task = %task_id, error = %err, "could not type into a full session");
            return false;
        }
        // A separate Enter, so the TUI sees typed text first and then a submit.
        tokio::time::sleep(Duration::from_millis(300)).await;
        if !self.attempt_live(task_id, attempt).await {
            return false;
        }
        if let Err(err) = self.send_input(task_id, b"\r").await {
            tracing::warn!(task = %task_id, error = %err, "could not submit into a full session");
            return false;
        }
        true
    }

    async fn audit_context(&self, task_id: &TaskId, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "system",
                AUDIT_ACTION,
                Some(task_id.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(task = %task_id, error = %err, "could not log a full context");
        }
    }
}
