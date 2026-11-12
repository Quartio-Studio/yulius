//! Moving a task off its subscription (SPEC-447): a person's choice, a removed subscription, and
//! one Claude refused. The move continues the same session where the task stands; a task with
//! nowhere to go waits, and when no subscription is usable at all it needs a person.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::account_gate::AgentAccounts;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{PauseKind, TaskRecord};
use crate::needs_you::NeedsYouRecord;
use crate::supervisor::RunStart;

/// What a task with nothing usable left is asked for (MOV-006). The request ends by itself when
/// the task gets a subscription again.
pub(crate) const NO_SUBSCRIPTION_ACTION: &str =
    "Sign a Claude subscription in again or add one in Subscriptions";

/// What a task is asked for when Claude refused it at a limit nothing says the end of, again
/// right after a retry, and no subscription can take it (SPEC-464 UCL-010). The request ends by
/// itself when the task goes on past the limit.
pub(crate) const LIMIT_WITHOUT_END_ACTION: &str = "Run the task on another subscription or model, or end the limit (for example with \
     usage credits), then send the task a message";

/// The audit action of such a request.
const LIMIT_WITHOUT_END_AUDIT: &str = "task.limit_without_end";

/// How often a task is tried again by itself at a model's limit nothing says the end of, when
/// no subscription can take it: once (a provider's reading ends such a limit, and a session
/// resumed on a newer Claude Code names the reset). Refused the same way again, it is not
/// started on that subscription at every reading (SPEC-464 UCL-010).
pub(crate) const BLIND_LIMIT_RETRIES: u32 = 1;

/// What a task remembers of the limits nothing says the end of that left it with nowhere to go
/// (SPEC-464 UCL-010). Kept in memory: a restart of Yulius allows one more retry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BlindLimits {
    /// Such waits in a row, the model not having answered in between.
    waits: u32,
    /// The subscription that refused the task that way again after its retries: the pool does
    /// not put the task back on it by itself.
    refused_by: Option<AccountId>,
}

/// What follows a limit nothing says the end of when no subscription can take the task now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlindWait {
    /// The task waits and the pool takes it on again: on any subscription while it has a
    /// retry, afterwards on any but the one that refused it.
    Pause,
    /// Nothing says when the task can go on: its session stays at the refusal, and a person
    /// is asked.
    AskAPerson,
}

impl BlindLimits {
    /// The model answered in the task's session: a later limit is no repeat of an earlier one.
    pub(crate) fn answered(&mut self) {
        *self = Self::default();
    }

    /// A limit nothing says the end of left the task with nowhere to go on `account`. `until`
    /// is the earliest reset known of a subscription that could take it; `status` is the
    /// task's (one that waits for capacity already has its pause, and its session is not at
    /// the refusal for a person to act on).
    pub(crate) fn nowhere_to_go(
        &mut self,
        account: Option<&AccountId>,
        until: Option<Timestamp>,
        status: TaskStatus,
    ) -> BlindWait {
        self.waits = self.waits.saturating_add(1);
        if self.waits <= BLIND_LIMIT_RETRIES {
            return BlindWait::Pause;
        }
        // Refused the same way right after its retry: not started there again by itself.
        self.refused_by = account.cloned();
        if until.is_some() || status != TaskStatus::Running {
            BlindWait::Pause
        } else {
            BlindWait::AskAPerson
        }
    }

    /// The subscription the pool leaves out for this task.
    pub(crate) fn refused_by(&self) -> Option<&AccountId> {
        self.refused_by.as_ref()
    }

    /// No reset is known to wait for any more: the subscription is a candidate again. The task
    /// has no retry left, so it asks a person when it is refused there the same way.
    pub(crate) fn nothing_to_wait_for(&mut self) {
        self.refused_by = None;
    }
}

/// Why a task left its subscription, as its terminal says it.
pub(crate) enum Left {
    /// Claude refused the subscription (#1138's `AccessLost`), with the provider's text.
    Refused { label: String, message: String },
    /// A person removed it.
    Removed { label: String },
}

impl Left {
    pub(crate) fn said(&self) -> String {
        match self {
            Self::Refused { label, message } => {
                let message: String = message.chars().take(200).collect();
                format!("Claude refused {label} ({})", message.trim())
            }
            Self::Removed { label } => format!("{label} was removed"),
        }
    }
}

impl App {
    /// Moves a running or waiting task to `account`, or to the pool's choice when `None`, in the
    /// same session (MOV-001, MOV-003): past the limit for a turn a limit cut (SPEC-034), the first
    /// message again in a fresh session when it never arrived (SPEC-068), a held prompt typed
    /// again, and a turn cut mid-way told to go on. An idle task gets nothing typed. With nowhere
    /// to go the task waits without a process.
    ///
    /// # Errors
    /// A refused switch (ROT-014), or `Conflict` when the task cannot switch now.
    pub(crate) async fn move_task(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        reason: &str,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let pause = store.open_pause(task_id).await?;
        let at_limit = pause.as_ref().is_some_and(|p| p.kind == PauseKind::Limit)
            || self.runtime(task_id).at_limit.load(Ordering::SeqCst);
        if at_limit {
            return self.switch_after_limit(task_id, account, reason).await;
        }
        if self.held_opening(task_id).await {
            let start = RunStart::Fresh { prompt: None };
            return self.switch_with(task_id, account, reason, start).await;
        }
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let prompt = match pause {
            Some(pause) if pause.kind == PauseKind::Prompt => pause.continuation,
            Some(pause) if pause.kind == PauseKind::Turn => self.stop_continuation(&task).await,
            _ if task.status == TaskStatus::Running && task.activity == Some(Activity::Working) => {
                Some("Continue the task exactly where you left off.".into())
            }
            _ => None,
        };
        self.switch_with(task_id, account, reason, RunStart::Resume { prompt })
            .await
    }

    /// Says in the task's terminal, in one line, why it left its subscription and where it went
    /// (MOV-006). With nothing usable left the task needs a person until it runs again.
    pub(crate) async fn tell_left(&self, task_id: &TaskId, left: &Left) {
        let store = &self.deps.store;
        let Ok(Some(task)) = store.task(task_id).await else {
            return;
        };
        let said = left.said();
        let on = match store.live_attempt(task_id).await {
            Ok(Some(attempt)) if task.status == TaskStatus::Running => match attempt.account_id {
                Some(id) => store.account(&id).await.ok().flatten().map(|a| a.label),
                None => None,
            },
            _ => None,
        };
        let hub = &self.runtime(task_id).hub;
        if let Some(on) = on {
            hub.notice(format!("↻ {said}: this session continues on {on}."));
            return;
        }
        if task.status != TaskStatus::WaitingCapacity {
            return;
        }
        let usable = self
            .agent_accounts(task.harness.provider())
            .await
            .is_ok_and(|a| a == AgentAccounts::Usable);
        if usable {
            hub.notice(format!(
                "⏸ {said}. No other subscription has room yet: the task continues by itself \
                 when one does."
            ));
            return;
        }
        hub.notice(format!(
            "⏸ {said}. No Claude subscription is left to continue on: sign one in again or add \
             one in Subscriptions, and the task continues by itself."
        ));
        // A request already open (the agent's question) stays: the task is under Needs you anyway.
        if let Ok(Some(open)) = store.task_needs_you(task_id).await
            && open.action != NO_SUBSCRIPTION_ACTION
        {
            return;
        }
        let request = NeedsYouRecord {
            action: NO_SUBSCRIPTION_ACTION.into(),
            happened: Some(format!("{said}.")),
            problem: Some("No Claude subscription is usable, so the task waits.".into()),
            decision: false,
            link: Some(crate::account_gate::CONNECT_PATH.into()),
            at: self.now().0,
        };
        match store
            .set_task_needs_you(task_id, Some(&request), self.now())
            .await
        {
            Ok(true) => self.publish_task(task_id).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "recording a Needs you failed")
            }
        }
        // A subscription that came meanwhile may have started it already: then nobody is needed.
        if !matches!(store.task(task_id).await, Ok(Some(t)) if t.status == TaskStatus::WaitingCapacity)
        {
            self.end_no_subscription_request(task_id).await;
        }
    }

    /// Claude refused `task` at a model's limit nothing says the end of, again right after a
    /// retry, and no subscription can take it (SPEC-464 UCL-010). It is not paused to be tried
    /// again: its session stays at the refusal, and a person is asked (unless a request is open
    /// already), in the terminal and under Needs you. A move to a subscription that can run it,
    /// or a message once the limit is over, takes it on.
    pub(crate) async fn limit_without_end(&self, task: &TaskRecord) {
        let runtime = self.runtime(&task.id);
        // The goal pause Claude Code wrote there says less than this request: not asked too,
        // whether its notice was read already or comes after.
        if let Ok(mut pause) = runtime.limit_goal_pause.lock() {
            pause.asked();
        }
        tracing::warn!(task = %task.id, "refused again at a limit nothing says the end of: the task needs a person");
        runtime.hub.notice(
            "⏸ Claude refused this session at a usage limit again and does not say when the limit \
             ends. No subscription can take the task now, so it waits for you: run it on another \
             subscription or model, or send it a message once the limit is over.",
        );
        let store = &self.deps.store;
        let now = self.now();
        if matches!(store.task_needs_you(&task.id).await, Ok(None)) {
            let request = NeedsYouRecord {
                action: LIMIT_WITHOUT_END_ACTION.into(),
                happened: Some(
                    "Claude refused the task at a usage limit again and did not say when the \
                     limit ends."
                        .into(),
                ),
                problem: Some(
                    "No subscription can run the task now, and nothing says when one can: the \
                     task does not go on by itself."
                        .into(),
                ),
                decision: false,
                link: Some(crate::account_gate::CONNECT_PATH.into()),
                at: now.0,
            };
            if let Err(err) = store
                .set_task_needs_you(&task.id, Some(&request), now)
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "recording a Needs you failed");
            }
        }
        if let Err(err) = self.set_task_activity(task, Activity::NeedsAttention).await {
            tracing::warn!(task = %task.id, error = %err, "could not flag a limit nothing says the end of");
        }
        self.publish_task(&task.id).await;
        if let Err(err) = store
            .audit(
                now,
                "system",
                LIMIT_WITHOUT_END_AUDIT,
                Some(task.id.as_str()),
                Some("refused again without a reset time; not retried"),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not log a limit nothing says the end of");
        }
    }

    /// Ends the "no subscription left" request once the task runs again (MOV-006); any other
    /// request stays.
    pub(crate) async fn end_no_subscription_request(&self, task_id: &TaskId) {
        if let Ok(Some(request)) = self.deps.store.task_needs_you(task_id).await
            && request.action == NO_SUBSCRIPTION_ACTION
        {
            self.end_needs_you(task_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-464 UCL-010: one retry at a limit nothing says the end of, then no more by itself.
    /// What follows depends on whether anything is known to wait for.
    #[test]
    fn a_limit_nothing_says_the_end_of_is_waited_for_once()
    -> Result<(), yulius_domain::ids::IdError> {
        const RUNNING: TaskStatus = TaskStatus::Running;
        let first = AccountId::parse("a-first")?;
        let known = Some(Timestamp(9_000));
        // Nowhere to go and nothing known: the task waits once, then a person is asked.
        let mut blind = BlindLimits::default();
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, RUNNING),
            BlindWait::Pause
        );
        assert_eq!(
            blind.refused_by(),
            None,
            "tried there again after a reading"
        );
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, RUNNING),
            BlindWait::AskAPerson
        );
        assert_eq!(blind.refused_by(), Some(&first));
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, RUNNING),
            BlindWait::AskAPerson,
            "and at every later refusal there"
        );
        // The model answered in between: a later limit has its own retry.
        blind.answered();
        assert_eq!(blind, BlindLimits::default());
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, RUNNING),
            BlindWait::Pause
        );
        // Another subscription says when it resets: the task waits for that, off the
        // subscription that refused it, and nobody is asked.
        let mut blind = BlindLimits::default();
        assert_eq!(
            blind.nowhere_to_go(Some(&first), known, RUNNING),
            BlindWait::Pause
        );
        assert_eq!(blind.refused_by(), None);
        assert_eq!(
            blind.nowhere_to_go(Some(&first), known, RUNNING),
            BlindWait::Pause
        );
        assert_eq!(blind.refused_by(), Some(&first), "left out from now on");
        // That reset is no longer known: the subscription is tried once more, and the task
        // asks a person when it is refused there again.
        blind.nothing_to_wait_for();
        assert_eq!(blind.refused_by(), None);
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, RUNNING),
            BlindWait::AskAPerson
        );
        // A task that waits for capacity already keeps its pause: the pool goes on from there.
        let mut blind = BlindLimits::default();
        let waiting = TaskStatus::WaitingCapacity;
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, waiting),
            BlindWait::Pause
        );
        assert_eq!(
            blind.nowhere_to_go(Some(&first), None, waiting),
            BlindWait::Pause
        );
        assert_eq!(blind.refused_by(), Some(&first));
        Ok(())
    }
}
