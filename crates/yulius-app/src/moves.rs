//! Moving a task off its subscription (SPEC-447): a person's choice, a removed subscription, and
//! one Claude refused. The move continues the same session where the task stands; a task with
//! nowhere to go waits, and when no subscription is usable at all it needs a person. A task moves
//! only between subscriptions of its own harness's provider, and what it says names them (SPEC-503
//! CDX-013).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::ModelFamily;

use crate::account_gate::{AgentAccounts, account_kind};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{PauseKind, TaskRecord};
use crate::needs_you::NeedsYouRecord;
use crate::supervisor::{CONTINUE_TASK_PROMPT, RunStart, SwitchCondition};

/// What a task of `provider` with nothing usable left is asked for (MOV-006). The request ends by
/// itself when the task gets a subscription again.
pub(crate) fn no_subscription_action(provider: Provider) -> String {
    format!(
        "Sign {} subscription in again or add one in Subscriptions",
        crate::account_gate::a_kind(provider)
    )
}

/// Whether `action` is the "no subscription left" request of any provider.
pub(crate) fn is_no_subscription_action(action: &str) -> bool {
    Harness::ALL
        .iter()
        .any(|harness| action == no_subscription_action(harness.provider()))
}

/// What a task is asked for when Claude refused it at a limit nothing says the end of, again
/// right after a retry, and no subscription can take it (SPEC-464 UCL-010). The request ends by
/// itself when Yulius takes the task on at a limit after all (a move, or a wait it resumes
/// from), or its agent works again.
pub(crate) const LIMIT_WITHOUT_END_ACTION: &str = "Run the task on another subscription or model, or end the limit (for example with \
     usage credits), then send the task a message";

/// The audit action of such a request.
const LIMIT_WITHOUT_END_AUDIT: &str = "task.limit_without_end";

/// How often a task is tried again by itself at a model's limit nothing says the end of, when
/// no subscription can take it: once (a provider's reading ends such a limit, and a session
/// resumed on a newer Claude Code names the reset). Refused the same way again, it is not
/// started on that subscription at every reading (SPEC-464 UCL-010).
pub(crate) const BLIND_LIMIT_RETRIES: u32 = 1;

/// What a task remembers of the limits nothing says the end of (SPEC-464 UCL-010). Kept in
/// memory: a restart of Yulius allows one more retry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct BlindLimits {
    /// Waits with nowhere to go in a row, the model not having answered in between.
    waits: u32,
    /// What refused the task that way since the model last answered: each subscription with
    /// the family of the model it refused. A task of a Fable model is refused its fallback
    /// there too (SPEC-482), and each model is remembered by itself.
    met: Vec<(AccountId, ModelFamily)>,
    /// The retries are spent: the pool does not put the task back on what refused it by
    /// itself while a reset is known to wait for.
    leaving_out: bool,
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

    /// A refresh actually ran this model on this subscription.
    pub(crate) fn available(&mut self, account: &AccountId, family: ModelFamily) {
        self.met
            .retain(|(id, model)| id != account || *model != family);
    }

    /// `account` refused the task's run of a model of `family` at a limit nothing says the end
    /// of.
    pub(crate) fn met(&mut self, account: &AccountId, family: ModelFamily) {
        let refusal = (account.clone(), family);
        if !self.met.contains(&refusal) {
            self.met.push(refusal);
        }
    }

    /// A limit nothing says the end of left the task with nowhere to go. `until` is the
    /// earliest reset known of a subscription that could take it; `status` is the task's (one
    /// that waits for capacity already has its pause, and its session is not at the refusal
    /// for a person to act on).
    pub(crate) fn nowhere_to_go(
        &mut self,
        until: Option<Timestamp>,
        status: TaskStatus,
    ) -> BlindWait {
        self.waits = self.waits.saturating_add(1);
        if self.waits <= BLIND_LIMIT_RETRIES {
            return BlindWait::Pause;
        }
        // Refused the same way right after its retry: not started there again by itself.
        self.leaving_out = true;
        if until.is_some() || status != TaskStatus::Running {
            BlindWait::Pause
        } else {
            BlindWait::AskAPerson
        }
    }

    /// What the pool leaves out for this task (`rotation::decide_leaving_out`): each
    /// subscription that refused it, for the model it refused. Nothing while it has a retry.
    pub(crate) fn left_out(&self) -> &[(AccountId, ModelFamily)] {
        if self.leaving_out { &self.met } else { &[] }
    }
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
        let observed = store.open_pause_with_process(task_id).await?;
        let task = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        let idle_codex = if let Some(activity) = task.activity.filter(|activity| {
            task.harness == Harness::Codex
                && task.status == TaskStatus::Running
                && activity.is_idle()
        }) {
            let attempt = store.live_attempt(task_id).await?.ok_or_else(|| {
                AppError::Conflict("the idle session changed during the move".into())
            })?;
            Some(SwitchCondition::Idle {
                from: (activity, task.updated_at),
                attempt: attempt.id,
            })
        } else {
            None
        };
        crate::race::reached(
            crate::race::Point::MovePauseRead,
            std::path::Path::new(task_id.as_str()),
        );
        // An admitted start owns its saved instruction and its pause row. Read a legacy
        // goal turn's process with that row so a later run cannot turn it into a live move.
        if let Some((pause, has_process)) = &observed
            && match pause.kind {
                PauseKind::GoalFreshStart | PauseKind::GoalResumeStart => true,
                PauseKind::FreshStart | PauseKind::Start => task.harness == Harness::Codex,
                PauseKind::Turn => task.mode == crate::model::TaskMode::Goal && !has_process,
                PauseKind::Limit | PauseKind::Prompt => false,
            }
        {
            let start = if pause.kind == PauseKind::Start {
                RunStart::Resume {
                    prompt: pause.continuation.clone(),
                }
            } else {
                self.start_for_capacity_pause(&task, pause).await?
            };
            return self
                .switch_from(
                    task_id,
                    account,
                    reason,
                    start,
                    SwitchCondition::Paused(pause),
                )
                .await;
        }
        let pause = observed.map(|(pause, _)| pause);
        let at_limit = pause.as_ref().is_some_and(|p| p.kind == PauseKind::Limit)
            || self.runtime(task_id).at_limit.load(Ordering::SeqCst);
        if at_limit {
            return self.switch_after_limit(task_id, account, reason).await;
        }
        if self.held_opening(task_id).await {
            let start = RunStart::Fresh { prompt: None };
            return self.switch_with(task_id, account, reason, start).await;
        }
        let goal_stop = task.mode == crate::model::TaskMode::Goal
            && pause
                .as_ref()
                .is_some_and(|pause| pause.kind == PauseKind::Turn);
        let prompt = match pause {
            Some(pause) if pause.kind == PauseKind::Prompt => pause.continuation,
            Some(pause) if pause.kind == PauseKind::Turn => self.stop_continuation(&task).await,
            _ if task.status == TaskStatus::Running && task.activity == Some(Activity::Working) => {
                Some(CONTINUE_TASK_PROMPT.into())
            }
            _ => None,
        };
        let start = RunStart::Resume { prompt }.with_goal_continuation(goal_stop);
        let condition = if matches!(start, RunStart::Resume { prompt: None }) {
            idle_codex.unwrap_or(SwitchCondition::Any)
        } else {
            SwitchCondition::Any
        };
        self.switch_from(task_id, account, reason, start, condition)
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
            cause: None,
            named: Default::default(),
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
                cause: None,
                named: Default::default(),
            };
            if let Err(err) = store
                .set_task_needs_you(&task.id, Some(&request), now)
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "recording a Needs you failed");
            }
        }
        // The agent's work after this shows the session went on: it ends the request and
        // answers the flag, as for a goal Claude Code paused (ADR-0304).
        if let Err(err) = self.flag_stalled(task).await {
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
            && is_no_subscription_action(&request.action)
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
        const FABLE: ModelFamily = ModelFamily::Fable;
        let first = AccountId::parse("a-first")?;
        let refused = [(first.clone(), FABLE)];
        let known = Some(Timestamp(9_000));
        // Refused there, nowhere to go.
        let nowhere = |blind: &mut BlindLimits, until, status| {
            blind.met(&first, FABLE);
            blind.nowhere_to_go(until, status)
        };
        // Nothing known: the task waits once, then a person is asked.
        let mut blind = BlindLimits::default();
        assert_eq!(nowhere(&mut blind, None, RUNNING), BlindWait::Pause);
        assert!(
            blind.left_out().is_empty(),
            "tried there again after a reading"
        );
        assert_eq!(nowhere(&mut blind, None, RUNNING), BlindWait::AskAPerson);
        assert_eq!(blind.left_out(), refused);
        assert_eq!(
            nowhere(&mut blind, None, RUNNING),
            BlindWait::AskAPerson,
            "and at every later refusal there"
        );
        // The model answered in between: a later limit has its own retry.
        blind.answered();
        assert_eq!(blind, BlindLimits::default());
        assert_eq!(nowhere(&mut blind, None, RUNNING), BlindWait::Pause);
        // Another subscription says when it resets: the task waits for that, off the
        // subscription that refused it, and nobody is asked.
        let mut blind = BlindLimits::default();
        assert_eq!(nowhere(&mut blind, known, RUNNING), BlindWait::Pause);
        assert!(blind.left_out().is_empty());
        assert_eq!(nowhere(&mut blind, known, RUNNING), BlindWait::Pause);
        assert_eq!(blind.left_out(), refused, "left out from now on");
        // That reset is no longer known and the pool tried the subscription once more
        // (`rotation::decide_leaving_out`): refused there again, the task asks a person.
        assert_eq!(nowhere(&mut blind, None, RUNNING), BlindWait::AskAPerson);
        // A task that waits for capacity already keeps its pause: the pool goes on from there.
        let mut blind = BlindLimits::default();
        let waiting = TaskStatus::WaitingCapacity;
        assert_eq!(nowhere(&mut blind, None, waiting), BlindWait::Pause);
        assert_eq!(nowhere(&mut blind, None, waiting), BlindWait::Pause);
        assert_eq!(blind.left_out(), refused);
        Ok(())
    }

    /// SPEC-482 FBK-007: a task of a Fable model is refused its fallback on the same
    /// subscription. Both models are remembered, each once, and left out together once the
    /// retry is spent; a limit that moved the task on (it had somewhere to go) is remembered
    /// without counting as a wait.
    #[test]
    fn each_model_a_subscription_refused_is_left_out() -> Result<(), yulius_domain::ids::IdError> {
        let first = AccountId::parse("a-first")?;
        let second = AccountId::parse("b-second")?;
        let known = Some(Timestamp(9_000));
        let mut blind = BlindLimits::default();
        // Fable refused: the task went on with the fallback, which is refused too.
        blind.met(&first, ModelFamily::Fable);
        blind.met(&first, ModelFamily::Opus);
        assert_eq!(
            blind.nowhere_to_go(known, TaskStatus::Running),
            BlindWait::Pause
        );
        assert!(blind.left_out().is_empty(), "one retry of each");
        blind.met(&first, ModelFamily::Fable);
        blind.met(&first, ModelFamily::Opus);
        blind.met(&second, ModelFamily::Opus);
        assert_eq!(
            blind.nowhere_to_go(known, TaskStatus::Running),
            BlindWait::Pause
        );
        assert_eq!(
            blind.left_out(),
            [
                (first.clone(), ModelFamily::Fable),
                (first, ModelFamily::Opus),
                (second, ModelFamily::Opus)
            ]
        );
        Ok(())
    }
}
