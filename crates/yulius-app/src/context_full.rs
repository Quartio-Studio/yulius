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
use yulius_domain::task::{Activity, StopReason, TaskStatus};

use crate::attention::AttentionRecord;
use crate::context::App;
use crate::model::{AttemptRecord, TaskRecord};
use crate::task_message::{TYPING_TURN_WAIT, Typing};

/// Longest Yulius waits for the compaction it asked for before it counts as failed (CTX-004).
pub const COMPACT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long `/compact` waits for a person's possible draft to leave the prompt box before it
/// is typed all the same (SPEC-483 TYP-001): long enough for a person who is there to send or
/// clear what they typed, short enough that a task nobody watches goes on.
pub const COMPACT_DRAFT_HOLD: Duration = Duration::from_secs(10 * 60);

/// Longest Yulius waits for a quiet terminal before it types.
const TYPE_WAIT: Duration = Duration::from_secs(30);

/// Output quiet this long means the TUI waits for input.
const TYPE_QUIET: Duration = Duration::from_millis(800);

/// What the terminal says when a person's typing leaves the full session to them.
const LEFT_TO_PERSON: &str = "Yulius leaves the full session to you: you typed into it. Type \
                              `/compact` when you are done, or send a prompt to let Yulius try \
                              again.";

/// Why a task waits for its person while its compaction is held for what they may have left
/// in the prompt box (SPEC-483 TYP-001).
const COMPACT_HELD_PROBLEM: &str = "The session's context is full, and Yulius does not compact \
    it yet: someone typed in the terminal and sent no prompt since, so their text may still be \
    in the prompt box.";
/// What the person does about [`COMPACT_HELD_PROBLEM`], and what happens when nobody does.
const COMPACT_HELD_ACTION: &str = "Send what you typed in the terminal, or clear it and send any \
    prompt: Yulius then compacts the session. In ten minutes Yulius types its command all the \
    same: text still in the prompt box then is sent with it and lost, and the task continues in \
    a fresh session.";
/// The same, as one line in the session's terminal.
const COMPACT_HELD_NOTICE: &str = "Yulius holds the compaction of this full session: text typed \
    here was not sent and may still be in the prompt box. Send it, or clear it and send any \
    prompt. In ten minutes Yulius types its command all the same: text still in the box then \
    is sent with it and lost, and the task continues in a fresh session.";
/// What the terminal says when the line that wakes a compacted session is not typed.
const WAKE_WITHHELD: &str = "Yulius did not tell the session to carry on: text typed here may \
    still be in the prompt box. Send it, or clear it and send any prompt.";
/// Why the compaction failed when its command, typed after the hold, was refused as a prompt.
const SENT_WITH_A_DRAFT: &str = "the command was sent with text left in the prompt box";

/// The reason of the flag that says a compaction waits for a person's possible draft.
fn held_for_a_draft() -> AttentionRecord {
    AttentionRecord::yulius(COMPACT_HELD_PROBLEM, COMPACT_HELD_ACTION)
}

/// The lines Yulius types into a full session, and what each does about a possible draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// `/compact <focus>`: held for the draft, its person told, and typed when the hold ends.
    Compact,
    /// The line that wakes a compacted session: not typed behind a draft, which the agent
    /// would act on; the terminal says so.
    Wake,
}

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
        /// The command was typed after its hold, behind what may sit in the prompt box
        /// (SPEC-483 TYP-001): with text there it is no command but a prompt, which the full
        /// context refuses, and that refusal is the compaction's failure.
        behind_draft: bool,
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
    /// The command went out as a prompt behind a person's text and was refused.
    Failed,
    Nothing,
}

/// Why the task needs a person when the fresh session is full too.
const STILL_FULL: &str = "the context is still full after compacting and a fresh session";

/// `text` with its first letter in upper case: a reason read on its own.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

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
                ContextEpisode::Compacting {
                    behind_draft: true, ..
                } => Next::Failed,
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
                        behind_draft: false,
                    };
                }
                Next::Restart => {
                    *episode = ContextEpisode::Restarted {
                        from: attempt.id.clone(),
                    };
                }
                Next::GiveUp => *episode = ContextEpisode::GaveUp,
                Next::Reflag | Next::Failed | Next::Nothing => {}
            }
            next
        };
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        match next {
            Next::Compact => {
                self.recovering_from_full_context(&task).await;
                self.compact(&task, attempt).await;
            }
            Next::Restart => {
                self.recovering_from_full_context(&task).await;
                self.restart_after_full_context(
                    &task,
                    attempt,
                    "the context is full again after compacting",
                );
            }
            Next::GiveUp => self.give_up_on_full_context(&task, STILL_FULL, true).await,
            Next::Reflag => self.give_up_on_full_context(&task, STILL_FULL, false).await,
            // No wait for the compaction's time: the task continues in a fresh session now.
            Next::Failed => {
                self.compaction_failed(task_id, attempt, None, SENT_WITH_A_DRAFT)
                    .await;
            }
            // A refusal while Yulius compacts or restarts the session: its recovery goes on.
            Next::Nothing => {}
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
        runtime.hub.notice("✓ Yulius compacted the conversation.");
        self.audit_context(task_id, "compacted").await;
        let line = self.task_focus(&task).await.after_compaction();
        let asked_ms = self.now().0;
        let app = Arc::clone(self);
        let task_id = task_id.clone();
        tokio::spawn(async move {
            let compacted = |e: &ContextEpisode| *e == ContextEpisode::Compacted;
            // A person who typed after the compaction just goes on; one who typed during it
            // is told that the session was not woken.
            if !app
                .type_line(&task_id, &attempt, (&line, Line::Wake), asked_ms, compacted)
                .await
            {
                app.recovery_left_to_person(&task_id).await;
            }
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
                    ..
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

    /// A turn the API refused ends, and Yulius recovers the session by itself (it compacts it,
    /// or continues the task in a fresh session): until it types the prompt that carries on, the
    /// task reads Working, since nobody has to act (CTX-002, SPEC-085 STT-006).
    async fn recovering_from_full_context(&self, task: &TaskRecord) {
        if task.status == TaskStatus::Running
            && matches!(
                task.activity,
                Some(Activity::Working | Activity::Waiting | Activity::Done)
            )
            && let Err(err) = self.set_task_activity(task, Activity::Working).await
        {
            tracing::warn!(task = %task.id, error = %err, "could not mark a full session's recovery");
        }
    }

    /// Yulius types nothing more into the full session (a person typed into it, the terminal
    /// never went quiet, the input failed): the recovery is theirs, and a task Working for it
    /// waits for them, as a refused turn did before (CTX-002). A turn a person's prompt started
    /// meanwhile goes on.
    async fn recovery_left_to_person(&self, task_id: &TaskId) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        if task.status == TaskStatus::Running
            && task.activity == Some(Activity::Working)
            && !self.runtime(task_id).turn_in_progress()
            && let Err(err) = self.set_task_activity(&task, Activity::Waiting).await
        {
            tracing::warn!(task = %task_id, error = %err, "could not settle a full session");
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
                .type_line(
                    &task_id,
                    &attempt.id,
                    (&command, Line::Compact),
                    since,
                    asked_now,
                )
                .await
            {
                // Not typed (the run ended, a person typed): the next refusal starts over.
                app.context_reset_if(&task_id, asked_now);
                app.recovery_left_to_person(&task_id).await;
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
                    "no subscription has capacity for a fresh session; once one has, start a new \
                     task from this one's request (continuing it brings the full session back)"
                        .to_owned()
                }
                Ok(()) => return,
            };
            // The full session stays (a refused switch), the task paused or the switch failed:
            // only a person can go on. A task a person is stopping needs no word of it.
            if let Ok(mut episode) = app.runtime(&task.id).context.lock() {
                *episode = ContextEpisode::GaveUp;
            }
            if let Ok(Some(task)) = app.deps.store.task(&task.id).await
                && !matches!(
                    task.status,
                    TaskStatus::Stopping | TaskStatus::Stopped(StopReason::Cancelled)
                )
            {
                app.give_up_on_full_context(&task, &why, true).await;
            }
        });
    }

    /// Recovery did not help: the task waits for a person, told `why` when `announce` (once per
    /// episode, CTX-005).
    async fn give_up_on_full_context(&self, task: &TaskRecord, why: &str, announce: bool) {
        // What its person does, and where: the notice is read in the terminal ("here").
        let to_do = |place: &str| {
            format!(
                "Find what fills it (a huge file, tool output or instructions), then type \
                 `/compact` or `/clear` {place}, or stop and continue the task."
            )
        };
        // The task says it too, for whoever opens it later (SPEC-472).
        if task.status == TaskStatus::Running
            && let Err(err) = self
                .flag_attention(
                    &task.id,
                    AttentionRecord::yulius(
                        &format!("{}.", sentence(why)),
                        &to_do("in the terminal"),
                    ),
                )
                .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not flag a full session");
        }
        if !announce {
            return;
        }
        tracing::error!(task = %task.id, why, "a full session needs a person");
        // Only a live session can be typed into.
        self.runtime(&task.id)
            .hub
            .notice(if task.status == TaskStatus::Running {
                format!("⚠ The task needs you: {why}. {}", to_do("here"))
            } else {
                format!("⚠ The task needs you: {why}.")
            });
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
        // A compaction that was held for a possible draft when Yulius restarted did not give
        // up: its flag ends and the recovery starts over, hold and all (SPEC-483 TYP-001).
        self.end_compaction_hold(task_id, Activity::Working).await;
        match self.deps.store.task(task_id).await {
            Ok(Some(task)) if task.activity != Some(Activity::NeedsAttention) => {
                self.context_full_observed(task_id, attempt).await;
            }
            _ => {}
        }
    }

    /// Types one line into the attempt's TUI once it waits for input, then a separate Enter,
    /// while the episode is one `wanted` accepts; `false` when it was not typed (the attempt is
    /// no longer live, the episode moved on, a person owns the prompt box, the terminal never
    /// went quiet, the input failed).
    ///
    /// The box is a person's when they typed since `asked_ms`. Something typed before may
    /// still sit in it too (any key since the box was last emptied counts: Yulius sees keys,
    /// not the box), and the line would be appended to it and sent with it (SPEC-480 DRF-001;
    /// SPEC-483 TYP-001). What then happens is the line's own ([`Line`]): the compaction is
    /// held for at most `compact_hold_ms` with the person told, the wake-up is not typed and
    /// the terminal says so. From the line to its Enter nothing else is typed ([`Typing`],
    /// TYP-002), and that Enter empties the box, whatever was in it.
    async fn type_line(
        &self,
        task_id: &TaskId,
        attempt: &AttemptId,
        line: (&str, Line),
        asked_ms: i64,
        wanted: impl Fn(&ContextEpisode) -> bool,
    ) -> bool {
        let mut held = false;
        let typed = self
            .type_line_or_hold(task_id, attempt, line, asked_ms, &wanted, &mut held)
            .await;
        // A hold that ended without the command (its person typed, the run ended, the terminal
        // never went quiet): the flag's "in ten minutes" is over, and the task waits for them.
        if held && !typed {
            self.end_compaction_hold(task_id, Activity::Waiting).await;
        }
        typed
    }

    /// [`Self::type_line`]; `held` says whether the hold's flag is up when it returns.
    async fn type_line_or_hold(
        &self,
        task_id: &TaskId,
        attempt: &AttemptId,
        (line, kind): (&str, Line),
        asked_ms: i64,
        wanted: &impl Fn(&ContextEpisode) -> bool,
        held: &mut bool,
    ) -> bool {
        let runtime = self.runtime(task_id);
        let typed_since = || runtime.typed_at_ms.load(Ordering::SeqCst) > asked_ms;
        let hold = Duration::from_millis(self.config.compact_hold_ms);
        // Since when the line waits for a possible draft; the person was told then.
        let mut held_since: Option<tokio::time::Instant> = None;
        // The hold ran out with the box still not known empty: the line goes behind it.
        let mut behind_draft = false;
        let mut started = tokio::time::Instant::now();
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
            if typed_since() {
                self.left_to_its_person(task_id, kind, wanted);
                return false;
            }
            if !behind_draft && self.has_draft(task_id) {
                if kind == Line::Wake {
                    // Typed while the session was compacted: the agent would act on it.
                    if self.context_is(task_id, wanted) {
                        runtime.hub.notice(WAKE_WITHHELD);
                    }
                    return false;
                }
                let since = match held_since {
                    Some(since) => since,
                    None => {
                        self.tell_of_held_compaction(task_id).await;
                        *held = true;
                        *held_since.insert(tokio::time::Instant::now())
                    }
                };
                if since.elapsed() < hold {
                    // The wait for a quiet terminal starts once the box is free.
                    started = tokio::time::Instant::now();
                    continue;
                }
                tracing::info!(task = %task_id, "nobody emptied the prompt box: the command for the full session is typed all the same");
                behind_draft = true;
            }
            ready = quiet_since.elapsed() >= TYPE_QUIET;
        }
        if !ready {
            tracing::warn!(task = %task_id, "the terminal never went quiet; nothing typed for a full session");
            return false;
        }
        crate::race::reached(
            crate::race::Point::LineReady,
            std::path::Path::new(task_id.as_str()),
        );
        // One typist at a time; what was asked above is asked again with the mark held, since
        // the wait for it gives a person time to start typing.
        let _typing = Typing::wait(&runtime.typing_message, TYPING_TURN_WAIT).await;
        if typed_since() {
            self.left_to_its_person(task_id, kind, wanted);
            return false;
        }
        if !self.context_is(task_id, wanted) || !self.attempt_live(task_id, attempt).await {
            return false;
        }
        if *held {
            // The hold is over, because it ran out or because the box was emptied without a
            // key (a held prompt went on): the task works again while Yulius goes on.
            self.end_compaction_hold(task_id, Activity::Working).await;
            *held = false;
        }
        if behind_draft {
            // Nobody emptied the box: a refusal of what is sent now is the compaction's
            // failure.
            if let Ok(mut episode) = runtime.context.lock()
                && let ContextEpisode::Compacting { behind_draft, .. } = &mut *episode
            {
                *behind_draft = true;
            }
        }
        if let Err(err) = self.send_input(task_id, line.as_bytes()).await {
            tracing::warn!(task = %task_id, error = %err, "could not type into a full session");
            return false;
        }
        crate::race::reached(
            crate::race::Point::LineTyped,
            std::path::Path::new(task_id.as_str()),
        );
        // A separate Enter, so the TUI sees typed text first and then a submit.
        tokio::time::sleep(Duration::from_millis(300)).await;
        if !self.attempt_live(task_id, attempt).await {
            return false;
        }
        if let Err(err) = self.send_input(task_id, b"\r").await {
            tracing::warn!(task = %task_id, error = %err, "could not submit into a full session");
            return false;
        }
        // The Enter sent whatever was in the box, so it is empty from here. Claude Code may
        // run no prompt hook for a command (2.1.292 ran none for `/compact`, SPEC-480), so
        // nothing else may say so, and a key from hours ago would keep the wake-up out after
        // the very compaction it was held for. An Enter the TUI dropped leaves the line in
        // the box all the same: not known here, and rare at an idle session.
        runtime
            .box_emptied_ms
            .fetch_max(self.now().0, Ordering::SeqCst);
        crate::race::reached(
            crate::race::Point::LineEntered,
            std::path::Path::new(task_id.as_str()),
        );
        true
    }

    /// A person typed since the line was asked for: nothing is typed, and for a compaction the
    /// terminal says that the full session is theirs (only while it is still wanted).
    fn left_to_its_person(
        &self,
        task_id: &TaskId,
        kind: Line,
        wanted: &impl Fn(&ContextEpisode) -> bool,
    ) {
        tracing::info!(task = %task_id, "a person types; Yulius does not type into the session");
        if kind == Line::Compact && self.context_is(task_id, wanted) {
            self.runtime(task_id).hub.notice(LEFT_TO_PERSON);
        }
    }

    /// Tells the session's person that its compaction waits for what they may have left in the
    /// prompt box (SPEC-483 TYP-001): the task is flagged with the reason and what to do, and
    /// its terminal says so, before the flag is raised. The flag ends with the hold, however
    /// it ends ([`Self::end_compaction_hold`]).
    async fn tell_of_held_compaction(&self, task_id: &TaskId) {
        self.runtime(task_id).hub.notice(COMPACT_HELD_NOTICE);
        if let Err(err) = self.flag_attention(task_id, held_for_a_draft()).await {
            tracing::warn!(task = %task_id, error = %err, "could not flag the task whose draft holds its compaction");
        }
        tracing::info!(task = %task_id, "the compaction of a full session waits for a person's possible draft");
    }

    /// The hold of a compaction is over: the task reads `next`, if it still reads the hold's
    /// flag.
    async fn end_compaction_hold(&self, task_id: &TaskId, next: Activity) {
        let runtime = self.runtime(task_id);
        let _held = runtime.attention_lock.lock().await;
        let store = &self.deps.store;
        let Ok(Some((reason, flagged))) = store.task_attention(task_id).await else {
            return;
        };
        if reason != held_for_a_draft() {
            return;
        }
        let from = (Some(Activity::NeedsAttention), flagged);
        match store
            .set_activity_from(task_id, from, Some(next), self.now())
            .await
        {
            Ok(true) => self.publish_task(task_id).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "could not end the flag of a held compaction");
            }
        }
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
