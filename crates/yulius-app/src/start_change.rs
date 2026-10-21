//! Changing how a waiting task starts (SPEC-126): a task that has not started yet (`chained` or
//! `prepared`) takes another of New task's start choices — after a task, on a rule, when a person
//! decides, at a time, now or as a supertask.
//!
//! Rules, in full: the choice is checked as at creation (one validator, `App::check_start_choice`).
//! The change replaces the status, the chain and the start time in one store transaction, only
//! while the task is still in the status it was read in; changes run one at a time, so two of them
//! never make tasks wait for each other. Moving into chains or prepared tasks from the other wait
//! counts against the sandbox's bound; changing within one does not. A wait may not wait for
//! itself, directly or through the predecessors and rules of other chained tasks. Start now and
//! Supertask wait after the sandbox's preparation while onboarding blocks tasks, as a new task
//! does; a chained task started now keeps its chain, as Start now does, so it builds on its
//! predecessor's branch. A task that has started keeps its start.

use std::collections::HashMap;
use std::sync::Arc;

use yulius_contracts::api::tasks::{ChangeStart, TaskSummary};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::rule::Rule;
use yulius_domain::task::{TaskEvent, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::intake::{ChainRecord, StartChange};
use crate::lifecycle::prepared_alone;
use crate::model::TaskRecord;
use crate::supervisor::RunStart;

/// New task's start choice as a request gives it (`CreateTask`, `ChangeStart`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StartFields<'a> {
    /// A task of the sandbox to run after.
    pub after_task_id: Option<&'a str>,
    /// A start rule.
    pub start_rule: Option<&'a str>,
    /// A supertask.
    pub supertask: bool,
    /// When a person decides.
    pub prepared: bool,
    /// At a time (RFC 3339).
    pub start_at: Option<&'a str>,
}

/// A start choice checked as New task checks it, without the sandbox's bounds.
#[derive(Debug)]
pub(crate) struct CheckedStart {
    /// A live task of the sandbox to run after.
    pub after: Option<TaskRecord>,
    /// A start rule (its tasks are checked where it is anchored).
    pub rule: Option<Rule>,
    /// Prepared: `Some(None)` when a person decides, `Some(Some(t))` at `t`.
    pub prepared: Option<Option<Timestamp>>,
    /// A supertask.
    pub supertask: bool,
}

/// A change's start choice.
enum Choice {
    /// After a task, or on a rule (anchored on the first task it names); `supertask` only for a
    /// supertask waiting for the sandbox's preparation.
    Chain {
        anchor: Box<TaskRecord>,
        rule: Option<Rule>,
        supertask: bool,
    },
    /// When a person decides, or at a time.
    Prepared(Option<Timestamp>),
    /// Now, optionally as a supertask.
    Now { supertask: bool },
}

fn trimmed(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|t| !t.is_empty())
}

impl App {
    /// Checks a start choice the way New task and a change share (SPEC-126): one predecessor or one
    /// rule; a supertask without either; a prepared task alone; a time in PRP-001's window.
    ///
    /// # Errors
    /// `Invalid` with the message New task gives.
    pub(crate) async fn check_start_choice(
        &self,
        sandbox: &SandboxId,
        fields: StartFields<'_>,
    ) -> AppResult<CheckedStart> {
        let after = match trimmed(fields.after_task_id) {
            None => None,
            Some(id) => {
                let id = TaskId::parse(id).map_err(|_| {
                    AppError::Invalid("the task to run after does not exist".into())
                })?;
                Some(self.live_predecessor(sandbox, &id).await?)
            }
        };
        let rule = match trimmed(fields.start_rule) {
            None => None,
            Some(text) => Some(Rule::parse(text).map_err(|e| AppError::Invalid(e.to_string()))?),
        };
        if after.is_some() && rule.is_some() {
            return Err(AppError::Invalid(
                "give a task to run after or a start rule, not both".into(),
            ));
        }
        // A supertask starts after the running work; waiting for a chain could wait for a task
        // its own hold keeps from starting (SPEC-071, ADR-0054).
        if fields.supertask && (after.is_some() || rule.is_some()) {
            return Err(AppError::Invalid(
                "a supertask starts after the running tasks: it cannot also run after a task or a rule"
                    .into(),
            ));
        }
        // Prepared (SPEC-105): when a person decides, or at a time.
        let prepared = match trimmed(fields.start_at) {
            Some(text) => Some(Some(crate::prepared::parse_start_at(text, self.now())?)),
            None => fields.prepared.then_some(None),
        };
        if prepared.is_some() && (after.is_some() || rule.is_some() || fields.supertask) {
            return Err(prepared_alone());
        }
        Ok(CheckedStart {
            after,
            rule,
            prepared,
            supertask: fields.supertask,
        })
    }

    /// `PUT /api/tasks/{tid}/start`: how a waiting task starts instead (SPEC-126).
    ///
    /// # Errors
    /// Access errors; `Conflict` when the task has started or changed concurrently; `Invalid` for
    /// a choice New task refuses or a wait on itself; `Capacity` beyond the sandbox's bound.
    pub async fn change_task_start(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ChangeStart,
    ) -> AppResult<TaskSummary> {
        // One change at a time: two could each pass the check below and wait for each other.
        let serial = self.start_change_lock.lock().await;
        let task = self.load_task(ctx, id, Action::Work).await?;
        let from = task.status;
        if !matches!(from, TaskStatus::Chained | TaskStatus::Prepared) {
            return Err(AppError::Conflict(format!(
                "the task has started (it is {}): only a task waiting to start changes how it starts",
                from.as_str()
            )));
        }
        let sandbox = task.sandbox_id.clone();
        let checked = self
            .check_start_choice(
                &sandbox,
                StartFields {
                    after_task_id: req.after_task_id.as_deref(),
                    start_rule: req.start_rule.as_deref(),
                    supertask: req.supertask,
                    prepared: req.prepared,
                    start_at: req.start_at.as_deref(),
                },
            )
            .await?;
        let mut choice = match checked {
            CheckedStart {
                after: Some(anchor),
                ..
            } => Choice::Chain {
                anchor: Box::new(anchor),
                rule: None,
                supertask: false,
            },
            CheckedStart {
                rule: Some(rule), ..
            } => Choice::Chain {
                anchor: Box::new(self.rule_named_anchor(&sandbox, &rule).await?),
                rule: Some(rule),
                supertask: false,
            },
            CheckedStart {
                prepared: Some(at), ..
            } => Choice::Prepared(at),
            CheckedStart { supertask, .. } => Choice::Now { supertask },
        };
        if let Choice::Chain { anchor, rule, .. } = &choice {
            self.refuse_waiting_for_itself(&task, anchor, rule.as_ref())
                .await?;
        }
        // A start that is not left to a person needs an agent (SPEC-246 CLA-002).
        if !matches!(choice, Choice::Prepared(_)) {
            self.ensure_agent_account(task.harness.provider()).await?;
        }
        // Start now waits after the sandbox's preparation while onboarding blocks tasks, a
        // supertask as one (U-08, as `create_task_held`).
        if let Choice::Now { supertask } = choice
            && let Some(preparation) = self.onboarding_wait(&sandbox, task.origin).await?
        {
            choice = Choice::Chain {
                anchor: Box::new(preparation),
                rule: None,
                supertask,
            };
        }
        let now = self.now();
        let store = &self.deps.store;
        let old = store.chain(&task.id).await?;
        let (event, chain, start_at, supertask, what) = match &choice {
            Choice::Chain {
                anchor,
                rule,
                supertask,
            } => {
                if from != TaskStatus::Chained {
                    self.ensure_chain_room(&sandbox).await?;
                }
                let chain = ChainRecord {
                    task_id: task.id.clone(),
                    after_task_id: Some(anchor.id.clone()),
                    after_number: anchor.number,
                    blocked_reason: None,
                    created_at: now,
                    rule: rule.as_ref().map(ToString::to_string),
                };
                let what = rule.as_ref().map_or_else(
                    || format!("after #{}", anchor.number),
                    |rule| format!("when {rule}"),
                );
                (TaskEvent::ChainChosen, Some(chain), None, *supertask, what)
            }
            Choice::Prepared(at) => {
                if from != TaskStatus::Prepared {
                    self.ensure_prepared_room(&sandbox).await?;
                }
                let what = at.map_or_else(|| "when I decide".into(), crate::auth::rfc3339);
                (TaskEvent::PreparedChosen, None, *at, false, what)
            }
            Choice::Now { supertask } => {
                // A chained task keeps its chain, as Start now does: it starts from its
                // predecessor's branch with its briefing (`chain_base`, `chain_briefing`).
                let (event, chain) = if from == TaskStatus::Chained {
                    let kept = old.clone().map(|c| ChainRecord {
                        blocked_reason: None,
                        ..c
                    });
                    (TaskEvent::ChainReleased, kept)
                } else {
                    (TaskEvent::PreparedReleased, None)
                };
                let what = if *supertask { "supertask" } else { "now" };
                (event, chain, None, *supertask, what.to_owned())
            }
        };
        let to = from
            .apply(event)
            .map_err(|_| AppError::Conflict(format!("the task is {}", from.as_str())))?;
        if !store
            .change_start(&StartChange {
                task_id: task.id.clone(),
                from,
                to,
                chain,
                start_at,
                supertask,
                at: now,
            })
            .await?
        {
            return Err(AppError::Conflict("task state changed concurrently".into()));
        }
        drop(serial);
        store
            .audit(
                now,
                &self.login_of(&ctx.user.id).await,
                "task.start_changed",
                Some(task.id.as_str()),
                Some(&what),
            )
            .await?;
        self.publish_task(&task.id).await;
        // The old predecessor's "runs after it" list lost this task.
        if let Some(before) = old.and_then(|c| c.after_task_id) {
            self.publish_task(&before).await;
        }
        match &choice {
            Choice::Chain { anchor, .. } => {
                // The predecessor may already be Done, or the rule true: start at once then.
                self.advance_chains(&anchor.id).await;
                self.publish_task(&anchor.id).await;
            }
            Choice::Prepared(Some(_)) => {
                if let Some(task) = store.task(&task.id).await? {
                    self.release_if_due(&task).await;
                }
            }
            Choice::Prepared(None) => {}
            Choice::Now { supertask } => {
                let app = Arc::clone(self);
                let id = task.id.clone();
                tokio::spawn(async move {
                    if let Err(err) = app
                        .start_run(&id, RunStart::Fresh { prompt: None }, None)
                        .await
                    {
                        tracing::warn!(error = %err, task = %id, "task failed to start");
                    }
                });
                // The project's queued tasks now wait for the supertask (SPEC-071 SUP-006).
                if *supertask {
                    self.drain_queue().await;
                }
            }
        }
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Refuses a wait that would wait for `task` itself: the predecessor or a task the rule
    /// names is `task`, or waits for it through chained tasks' predecessors and rules (CHS-004).
    /// A chained task's rule names only tasks that existed when it was written (CHS-006).
    async fn refuse_waiting_for_itself(
        &self,
        task: &TaskRecord,
        anchor: &TaskRecord,
        rule: Option<&Rule>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let tasks = store.tasks(&task.sandbox_id, false).await?;
        let by_number: HashMap<u32, &TaskRecord> = tasks.iter().map(|t| (t.number, t)).collect();
        let named = |numbers: Vec<u32>, written: Option<Timestamp>| -> Vec<TaskId> {
            numbers
                .into_iter()
                .filter_map(|n| by_number.get(&n))
                .filter(|t| written.is_none_or(|at| t.created_at <= at))
                .map(|t| t.id.clone())
                .collect()
        };
        let mut pending = match rule {
            Some(rule) => named(rule.numbers(), None),
            None => vec![anchor.id.clone()],
        };
        let mut seen: Vec<TaskId> = Vec::new();
        while let Some(id) = pending.pop() {
            if id == task.id {
                return Err(AppError::Invalid(format!(
                    "#{} cannot wait for itself: what it would wait for already waits for it",
                    task.number
                )));
            }
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let waiting = tasks
                .iter()
                .any(|t| t.id == id && t.status == TaskStatus::Chained);
            if !waiting {
                continue;
            }
            if let Some(chain) = store.chain(&id).await? {
                match chain.rule.as_deref().map(Rule::parse) {
                    Some(Ok(rule)) => pending.extend(named(rule.numbers(), Some(chain.created_at))),
                    Some(Err(_)) => {}
                    None => pending.extend(chain.after_task_id),
                }
            }
        }
        Ok(())
    }
}
