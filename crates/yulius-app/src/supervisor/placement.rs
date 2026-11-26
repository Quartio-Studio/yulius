//! Placing runs on subscriptions: the account views, the choice of a subscription, model
//! availability, usage records, and the limit and rotation decisions of a live run.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_domain::harness::claude_model_name;
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::rotation::{self, AccountView, Decision, Trigger};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, FULL_PERCENT, ModelFamily, ReportedLimit};

use super::ObservationSource;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AccountRecord, AccountStatus, AttemptRecord, PauseKind, TaskRecord};
use crate::ports::harness::Observation;
use crate::usage_json;

/// The model family `limit` names when that is not `family`, the one the run was launched with:
/// the session's person switched the model inside it (`/model`), which no launch follows, and
/// Claude Code's record of the refused request named it (SPEC-493 MDL-002). The limit is that
/// model's alone.
pub(crate) fn switched_model(limit: &ReportedLimit, family: ModelFamily) -> Option<ModelFamily> {
    match limit.scope {
        yulius_domain::usage::LimitScope::Model(Some(named)) if named != family => Some(named),
        _ => None,
    }
}

/// What a session's terminal says when the model its person switched it to (`ran`) is at its
/// limit and Yulius continues the session with `next`, the model the relaunch runs (the
/// session's own, or its fallback).
pub(crate) fn switched_back_notice(ran: ModelFamily, next: &str) -> String {
    let ran = match ran {
        ModelFamily::Fable => "Fable",
        ModelFamily::Opus => "Opus",
        ModelFamily::Sonnet => "Sonnet",
        ModelFamily::Any => "That model",
    };
    format!(
        "{ran} is used up on this subscription: this session goes on with {}.",
        claude_model_name(next)
    )
}

/// Where the pool places a run (SPEC-482).
pub(crate) struct Placed {
    /// The subscription.
    pub account: AccountRecord,
    /// The model the run is launched with instead of the task's own, while no subscription can
    /// run that; `None`: the task's own.
    pub fallback: Option<&'static str>,
}

/// What a session's terminal says when a launch changes the model it runs (SPEC-482 FBK-008):
/// `model` is not the task's `own` (the launch falls back), or it is again after a run on
/// another model (`before`, the model of the task's last run). `None` when nothing changes.
pub(crate) fn model_change_notice(own: &str, before: Option<&str>, model: &str) -> Option<String> {
    let name = claude_model_name;
    if model != own {
        return (before != Some(model)).then(|| {
            format!(
                "{} is used up on every subscription: this session runs on {}.",
                name(own),
                name(model)
            )
        });
    }
    // Only a fallback is undone: a task set to another model since its last run says nothing.
    let fell_back = before.is_some_and(|before| {
        before != own && yulius_domain::harness::fallback_model(own) == Some(before)
    });
    fell_back.then(|| format!("{} can run again: this session runs on it.", name(own)))
}

/// Why a person's choice of subscription cannot take the task, in one plain sentence naming it
/// (SPEC-447 MOV-002).
pub(crate) fn refused_choice(
    label: Option<&str>,
    provider: yulius_domain::harness::Provider,
    reason: &rotation::DecisionReason,
) -> String {
    use rotation::{DecisionReason, Refusal};
    let name = label.unwrap_or("That subscription");
    match reason {
        DecisionReason::ManualRefused(Refusal::UnknownAccount) => {
            "That subscription no longer exists.".into()
        }
        DecisionReason::ManualRefused(Refusal::Unavailable) => {
            format!("{name} needs a new sign-in, so it cannot take the task.")
        }
        DecisionReason::ManualRefused(Refusal::Exhausted) => {
            format!("{name} is at its usage limit, so it cannot take the task until it resets.")
        }
        DecisionReason::ManualRefused(Refusal::ProviderMismatch) => {
            format!(
                "{name} is not {} subscription, so it cannot take the task.",
                crate::account_gate::a_kind(provider)
            )
        }
        _ => "No other subscription can take the task right now.".into(),
    }
}

impl App {
    /// The model families waiting tasks have excluded from this account. A window reading
    /// can already have cleared the stored limit by the time a person asks for a refresh.
    pub(crate) async fn excluded_families(&self, account: &AccountId) -> Vec<ModelFamily> {
        let mut families = Vec::new();
        let Ok(pauses) = self.deps.store.open_pauses().await else {
            return families;
        };
        for pause in pauses {
            if let Some(runtime) = self.existing_runtime(&pause.task_id)
                && let Ok(limits) = runtime.blind_limits.lock()
            {
                for (id, family) in limits.left_out() {
                    if id == account && !families.contains(family) {
                        families.push(*family);
                    }
                }
            }
        }
        families
    }

    /// A successful model probe allows this account/model pair in each task's next placement.
    pub(crate) fn model_available(&self, account: &AccountId, family: ModelFamily) {
        if let Ok(runtimes) = self.supervisor.runtimes.lock() {
            for runtime in runtimes.values() {
                if let Ok(mut limits) = runtime.blind_limits.lock() {
                    limits.available(account, family);
                }
            }
        }
        self.capacity_changed();
    }

    /// Account views for the rotation policy; Done tasks' idle sessions are no load (SPEC-057).
    pub(crate) async fn account_views(&self) -> AppResult<Vec<AccountView>> {
        let accounts = self.deps.store.accounts().await?;
        let live = self.working_attempts().await?;
        Ok(accounts
            .iter()
            .filter(|a| a.credential.is_some())
            .map(|account| {
                crate::capacity::rotation_view(
                    account,
                    crate::accounts::in_use_count(&account.id, &live),
                )
            })
            .collect())
    }

    /// When a paused task resumes: the earliest known time a subscription has capacity for it
    /// again, as the pool policy decides now (ROT-010). `None` while the task is not paused,
    /// once capacity is back, or when no reset time is known.
    pub(crate) async fn capacity_returns_at(&self, task: &TaskRecord) -> Option<Timestamp> {
        if task.status != TaskStatus::WaitingCapacity {
            return None;
        }
        let input = self.rotation_input(task, None).await.ok()?;
        // As the resume loop decides: off what refused the task with no end (SPEC-464 UCL-010).
        match self.decide_for_waiting(&task.id, &input, Trigger::Start).0 {
            Decision::Wait { until } => until,
            Decision::Switch { .. } | Decision::Stay => None,
        }
    }

    /// Where a new attempt of `task` runs (placement, ROT-004): its account, and the fallback
    /// model it is launched with while no subscription can run the task's own (SPEC-482). `None`
    /// when no subscription has capacity and the task must pause (ROT-010). A requested account
    /// is honoured when it is usable and refused otherwise (ROT-007).
    pub(crate) async fn choose_account(
        &self,
        task: &TaskRecord,
        requested: Option<&AccountId>,
    ) -> AppResult<Option<Placed>> {
        let trigger = match requested {
            Some(id) => Trigger::Manual {
                account: id.clone(),
            },
            None => Trigger::Start,
        };
        let input = self.rotation_input(task, None).await?;
        match rotation::decide(&input, trigger) {
            (Decision::Switch { account, .. }, _) => Ok(Some(Placed {
                account: self
                    .deps
                    .store
                    .account(&account)
                    .await?
                    .ok_or(AppError::NotFound)?,
                // The same snapshot decided the account and says the model.
                fallback: crate::capacity::fallback_of(task.model.as_deref())
                    .filter(|_| rotation::placing_family(&input) != input.family),
            })),
            (Decision::Wait { .. }, _) => Ok(None),
            (Decision::Stay, reason) => {
                let label = match requested {
                    Some(id) => self.deps.store.account(id).await?.map(|a| a.label),
                    None => None,
                };
                Err(AppError::Capacity(refused_choice(
                    label.as_deref(),
                    task.harness.provider(),
                    &reason,
                )))
            }
        }
    }

    /// Account usage update from transcripts or providers. The subscription reads `limited`
    /// while the default model cannot run on it: a limit Claude reported is in effect, or a
    /// window that binds that model is used up (SPEC-464).
    pub(crate) async fn record_account_usage(&self, account: &AccountId, usage: &AccountUsage) {
        let now = self.now();
        // A per-model window whose reset passed says nothing about the one that began.
        let usage = &usage.without_reset_model_windows(now);
        let family = crate::capacity::default_family();
        // A reading never makes a subscription that is not healthy usable again: a cancelled
        // one can still report usage (or a late transcript line can), and only a sign-in proves
        // it works (SPEC-483 SGN-002).
        let needs_sign_in = match self.deps.store.account(account).await {
            Ok(Some(record)) if !record.status.healthy() => {
                Some((record.status, record.last_error))
            }
            _ => None,
        };
        crate::race::reached(
            crate::race::Point::UsageStatusRead,
            Path::new(account.as_str()),
        );
        // Nor does it erase why it needs one (SPEC-447 MOV-005).
        let kept_error = needs_sign_in.as_ref().and_then(|(_, error)| error.clone());
        let status = if let Some((status, _)) = needs_sign_in {
            status
        } else if usage.exhausted_for(now, FULL_PERCENT, family) {
            AccountStatus::Limited
        } else {
            AccountStatus::Ok
        };
        let json = usage_json::to_json(usage);
        let _ = self
            .deps
            .store
            .set_account_state(account, status, Some(&json), kept_error.as_deref(), now)
            .await;
        // New usage may let paused tasks resume, or pause the pool (ROT-010).
        self.capacity_changed();
        // At or above the threshold: tasks on this account rotate at their next turn boundary
        // (ROT-003); with hooks the Stop hook decides this itself.
        if usage.exhausted(now, rotation::THRESHOLD)
            && let Ok(live) = self.deps.store.live_attempts().await
        {
            for attempt in live
                .iter()
                .filter(|a| a.account_id.as_ref() == Some(account))
            {
                self.runtime(&attempt.task_id)
                    .proactive_switch
                    .store(true, Ordering::SeqCst);
            }
        }
    }

    /// A usage limit that cut off a subagent of `task_id`'s thread (SPEC-493 RLM-001, RLM-002):
    /// the subscription's reading keeps it, for the model family it binds. One that binds the
    /// session's own model (`family`, or the subscription) moves or pauses the session as any
    /// limit does; one of another model leaves the session where it is, unless the subagent is
    /// a review round's reviewer, whose model the session then moves for. Either way a relaunch
    /// tells the session to resume that reviewer.
    pub(crate) async fn subagent_limit(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        family: ModelFamily,
        observation: &Observation,
    ) {
        let Observation::SubagentCutOff {
            agent_id,
            at,
            resets_at,
            message,
            window_minutes,
            scope,
        } = observation
        else {
            return;
        };
        use yulius_domain::usage::LimitScope;
        // A limit of "the model that ran" of a review round's reviewer is of its round's model,
        // also when no answer of it was read yet (refused at its first request, or resumed in a
        // new process); one of a subagent whose model cannot be told is no one's (review round
        // 1 of fix 7): it is never taken for the session's.
        // Pair the eligibility read and reservation with the hooks that clear recovery.
        // Release this lock before writing account usage so a reviewer can report meanwhile.
        let runtime = self.runtime(task_id);
        let mut pending = runtime.cut_off_reviewer.lock().await;
        let reviewer = self.open_round_reviewer(task_id, agent_id, *at).await;
        let scope = match (*scope, &reviewer) {
            (LimitScope::Model(None), Some(reviewer)) => {
                match ModelFamily::of(Some(&reviewer.model)) {
                    ModelFamily::Any => LimitScope::Model(None),
                    named => LimitScope::Model(Some(named)),
                }
            }
            (scope, _) => scope,
        };
        if scope == LimitScope::Model(None) {
            tracing::info!(task = %task_id, %message, "a usage limit of a subagent whose model is not known: not the session's");
            return;
        }
        if let Some(reviewer) = &reviewer {
            *pending = Some(reviewer.clone());
        }
        drop(pending);
        let limit = ReportedLimit {
            scope,
            window_minutes: *window_minutes,
            resets_at: *resets_at,
        };
        let binds_session = !matches!(
            scope,
            LimitScope::Model(Some(other)) if other != family
        );
        tracing::info!(task = %task_id, %message, ?scope, binds_session, "usage limit reached by a subagent");
        if binds_session {
            self.limit_met(task_id, attempt, limit, *at, ObservationSource::Live)
                .await;
            return;
        }
        // Recorded before the move is decided: where the session is reads as unable to run the
        // reviewer's model only then (review round 1 of fix 7).
        if let Some(account) = &attempt.account_id {
            self.record_limit(account, limit, family).await;
        }
        if let Some(reviewer) = reviewer {
            let pending = runtime.cut_off_reviewer.lock().await;
            if pending.as_ref() == Some(&reviewer) {
                runtime.reviewer_move.store(true, Ordering::SeqCst);
            }
        }
    }

    /// The session of `task_id` is at the usage limit `limit`, recorded at `at`: its turn is
    /// over, the subscription's reading keeps the limit, and the pool decides where the task
    /// goes on (SPEC-034, SPEC-464).
    pub(super) async fn limit_met(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptRecord,
        limit: ReportedLimit,
        at: Option<Timestamp>,
        source: ObservationSource,
    ) {
        let runtime = self.runtime(task_id);
        runtime.end_turn();
        let blind = limit.without_an_end(self.now());
        // The limit of the model this run was launched with (SPEC-482 FBK-006), unless it names
        // another one: the model the session was switched to inside it (SPEC-493 MDL-002).
        let family = self.run_family_of(attempt).await;
        let ran = switched_model(&limit, family);
        runtime.limit_cut_the_turn(&attempt.id, at, ran);
        if let Some(account) = &attempt.account_id {
            // A hand-over may place the replacement itself. The reporting account and
            // model still count if the pool later exhausts its retry without an answer.
            if blind && let Ok(mut met) = runtime.blind_limits.lock() {
                met.met(account, ran.unwrap_or(family));
            }
            self.record_limit(account, limit, family).await;
        }
        let app = self.clone();
        let tid = task_id.clone();
        let account = attempt.account_id.clone();
        let attempt_id = attempt.id.clone();
        tokio::spawn(async move {
            crate::race::reached(crate::race::Point::LimitMoveReady, Path::new(tid.as_str()));
            app.rotate_limited_attempt(&tid, &attempt_id, account, blind, ran, source)
                .await;
            crate::race::reached(
                crate::race::Point::LimitMoveFinished,
                Path::new(tid.as_str()),
            );
        });
    }

    /// Reconciliation can replace a run holding the previous owner's servers after reading
    /// this limit. Apply the pool decision only if the reporting attempt still owns the run.
    pub(super) async fn rotate_limited_attempt(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt_id: &AttemptId,
        account: Option<AccountId>,
        blind: bool,
        ran: Option<ModelFamily>,
        source: ObservationSource,
    ) {
        let Some(runtime) = self.existing_runtime(task_id) else {
            return;
        };
        let recovery = runtime.reattach_move.lock().await;
        // All reports wait for a pending hand-over. Only a replayed move holds the lock
        // through its decision. A new live pause can resume while its notices finish.
        let _recovery = matches!(source, ObservationSource::Replay).then_some(recovery);
        let current = runtime
            .run
            .lock()
            .await
            .as_ref()
            .is_some_and(|run| &run.attempt_id == attempt_id);
        if current {
            self.rotate_past(task_id, account, Trigger::LimitReached, blind, ran)
                .await;
        }
    }

    /// Applies the pool policy for a running task and acts on the decision (SPEC-001 §7.4):
    /// stay, move to another subscription by native resume, or pause (ROT-010).
    pub async fn rotate(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
    ) {
        self.rotate_at(task_id, current, trigger, false).await;
    }

    /// [`Self::rotate`]; `blind` says the limit that asks for it is a model's that nothing says
    /// the end of (SPEC-464 UCL-010).
    async fn rotate_at(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
        blind: bool,
    ) {
        self.rotate_past(task_id, current, trigger, blind, None)
            .await;
    }

    /// [`Self::rotate_at`]; `ran` is the model family the limit binds when that is not the one
    /// the run was launched with ([`switched_model`]). The run placed past the limit is the
    /// task's own model again, so the subscription it sits on still counts, and the session's
    /// terminal says why it goes on with another model than its person chose inside it.
    async fn rotate_past(
        self: &Arc<Self>,
        task_id: &TaskId,
        current: Option<AccountId>,
        trigger: Trigger,
        blind: bool,
        ran: Option<ModelFamily>,
    ) {
        let Ok(Some(task)) = self.deps.store.task(task_id).await else {
            return;
        };
        let limit = matches!(trigger, Trigger::LimitReached);
        // A limit can also end a turn that is already held at its boundary.
        if task.status != TaskStatus::Running
            && !(limit && task.status == TaskStatus::WaitingCapacity)
        {
            return;
        }
        let Ok(mut input) = self.rotation_input(&task, current.clone()).await else {
            return;
        };
        if ran.is_some() {
            input.running = ran;
        }
        let (decision, reason) = rotation::decide(&input, trigger);
        self.runtime(task_id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        match decision {
            Decision::Stay => {}
            Decision::Switch { account } => {
                // A turn the limit cut goes on past it, with nothing typed (SPEC-034).
                let reason = format!("{reason:?}");
                let switched = if limit {
                    self.switch_after_limit(task_id, Some(account), &reason)
                        .await
                } else {
                    self.switch_task_now(task_id, Some(account), &reason, None)
                        .await
                };
                if let Err(err) = switched {
                    tracing::warn!(error = %err, task = %task_id, "automatic switch failed");
                }
            }
            Decision::Wait { until } => {
                // A model's limit nothing says the end of, with nowhere to go: the task is
                // tried again by itself only so often (SPEC-464 UCL-010). After that it waits
                // for a reset that is known, off this subscription, or asks a person.
                if blind && limit {
                    let next = self
                        .runtime(task_id)
                        .blind_limits
                        .lock()
                        .map_or(crate::moves::BlindWait::Pause, |mut blind| {
                            blind.nowhere_to_go(until, task.status)
                        });
                    if next == crate::moves::BlindWait::AskAPerson {
                        self.limit_without_end(&task).await;
                        return;
                    }
                }
                let paused = if limit {
                    let paused = self
                        .pause_task(task_id, PauseKind::Limit, current, None)
                        .await;
                    // The pool resumes a task that waits at its limit by itself: a request
                    // that says it does not go on by itself is answered, as at a move past
                    // the limit (SPEC-464 UCL-007).
                    if paused.is_ok() {
                        crate::race::reached(
                            crate::race::Point::LimitPauseStored,
                            Path::new(task_id.as_str()),
                        );
                        self.end_limit_requests(task_id, crate::goal_retry::TakenOn::Paused)
                            .await;
                    }
                    paused
                } else {
                    self.pause_task(task_id, PauseKind::Turn, current, None)
                        .await
                };
                if let Err(err) = paused {
                    tracing::warn!(error = %err, task = %task_id, "could not pause the task");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// SPEC-482 FBK-008: the terminal says a change of the model once, at the launch that
    /// makes it: onto the fallback, and back to the task's own model after a run on it.
    #[test]
    fn a_launch_says_when_the_model_changes() {
        use super::model_change_notice as said;
        let (fable, opus) = ("claude-fable-5-2", "claude-opus-5-5");
        let fell_back =
            "Fable 5.2 is used up on every subscription: this session runs on Opus 5.5.";
        let back = "Fable 5.2 can run again: this session runs on it.";
        // The first run, a run after one on Fable, and one after a run that names no model.
        for before in [None, Some(fable)] {
            assert_eq!(said(fable, before, opus).as_deref(), Some(fell_back));
            assert_eq!(said(fable, before, fable), None);
        }
        assert_eq!(said(fable, Some(opus), opus), None, "said at the first");
        assert_eq!(said(fable, Some(opus), fable).as_deref(), Some(back));
        // An older Fable model names itself.
        assert_eq!(
            said("claude-fable-5-1", Some(opus), "claude-fable-5-1").as_deref(),
            Some("Fable 5.1 can run again: this session runs on it.")
        );
        // A task a person set to another model since its last run did not fall back.
        assert_eq!(said(opus, Some(fable), opus), None);
        assert_eq!(
            said("claude-sonnet-5-5", Some(opus), "claude-sonnet-5-5"),
            None
        );
    }
}
