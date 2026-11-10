//! Moving a task off its subscription (SPEC-447): a person's choice, a removed subscription, and
//! one Claude refused. The move continues the same session where the task stands; a task with
//! nowhere to go waits, and when no subscription is usable at all it needs a person. A task moves
//! only between subscriptions of its own harness's provider, and what it says names them (SPEC-451
//! CDX-013).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};

use crate::account_gate::{AgentAccounts, account_kind};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::PauseKind;
use crate::needs_you::NeedsYouRecord;
use crate::supervisor::RunStart;

/// What a task of `provider` with nothing usable left is asked for (MOV-006). The request ends by
/// itself when the task gets a subscription again.
pub(crate) fn no_subscription_action(provider: Provider) -> String {
    format!(
        "Sign a {} subscription in again or add one in Subscriptions",
        account_kind(provider)
    )
}

/// Whether `action` is the "no subscription left" request of any provider.
fn is_no_subscription_action(action: &str) -> bool {
    Harness::ALL
        .iter()
        .any(|harness| action == no_subscription_action(harness.provider()))
}

/// Why a task left its subscription, as its terminal says it.
pub(crate) enum Left {
    /// The provider refused the subscription (#1138's `AccessLost`), with the provider's text.
    Refused {
        provider: Provider,
        label: String,
        message: String,
    },
    /// A person removed it.
    Removed { label: String },
}

impl Left {
    pub(crate) fn said(&self) -> String {
        match self {
            Self::Refused {
                provider,
                label,
                message,
            } => {
                let message: String = message.chars().take(200).collect();
                format!(
                    "{} refused {label} ({})",
                    account_kind(*provider),
                    message.trim()
                )
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
        let provider = task.harness.provider();
        let kind = account_kind(provider);
        let usable = self
            .agent_accounts(provider)
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
            "⏸ {said}. No {kind} subscription is left to continue on: sign one in again or add \
             one in Subscriptions, and the task continues by itself."
        ));
        let action = no_subscription_action(provider);
        // A request already open (the agent's question) stays: the task is under Needs you anyway.
        if let Ok(Some(open)) = store.task_needs_you(task_id).await
            && !is_no_subscription_action(&open.action)
        {
            return;
        }
        let request = NeedsYouRecord {
            action,
            happened: Some(format!("{said}.")),
            problem: Some(format!(
                "No {kind} subscription is usable, so the task waits."
            )),
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

    /// Ends the "no subscription left" request once the task runs again (MOV-006); any other
    /// request stays.
    pub(crate) async fn end_no_subscription_request(&self, task_id: &TaskId) {
        if let Ok(Some(request)) = self.deps.store.task_needs_you(task_id).await
            && is_no_subscription_action(&request.action)
        {
            self.end_needs_you(task_id).await;
        }
    }
}
