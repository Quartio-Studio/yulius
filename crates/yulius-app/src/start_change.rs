//! Changing how a waiting task starts (SPEC-126): a task that has not started yet (`chained` or
//! `prepared`) takes another of New task's start choices — after a task, on a rule, when a person
//! decides, at a time, now or as a supertask.
//!
//! Rules, in full: the choice is checked as at creation. The change replaces the status, the
//! chain and the start time in one store transaction, only while the task is still in the status
//! it was read in. Moving into chains or prepared tasks from the other wait counts against the
//! sandbox's bound; changing within one does not. A wait may not wait for itself, directly or
//! through the predecessors and rules of other chained tasks. Start now and Supertask wait after
//! the sandbox's preparation while onboarding blocks tasks, as a new task does. A task that has
//! started keeps its start.

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

/// A checked start choice.
enum Choice {
    /// After a task, or on a rule (anchored on the first task it names).
    Chain {
        anchor: Box<TaskRecord>,
        rule: Option<Rule>,
    },
    /// When a person decides, or at a time.
    Prepared(Option<Timestamp>),
    /// Now, optionally as a supertask.
    Now { supertask: bool },
}

impl App {
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
        let task = self.load_task(ctx, id, Action::Work).await?;
        let from = task.status;
        if !matches!(from, TaskStatus::Chained | TaskStatus::Prepared) {
            return Err(AppError::Conflict(format!(
                "the task has started (it is {}): only a task waiting to start changes how it starts",
                from.as_str()
            )));
        }
        let sandbox = task.sandbox_id.clone();
        let mut choice = self.start_choice(&sandbox, req).await?;
        if let Choice::Chain { anchor, rule } = &choice {
            self.refuse_waiting_for_itself(&task, anchor, rule.as_ref())
                .await?;
        }
        // Start now waits after the sandbox's preparation while onboarding blocks tasks (U-08).
        if let Choice::Now { .. } = choice
            && let Some(preparation) = self.onboarding_wait(&sandbox, task.origin).await?
        {
            choice = Choice::Chain {
                anchor: Box::new(preparation),
                rule: None,
            };
        }
        let now = self.now();
        let (event, chain, start_at, supertask, what) = match &choice {
            Choice::Chain { anchor, rule } => {
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
                (TaskEvent::ChainChosen, Some(chain), None, false, what)
            }
            Choice::Prepared(at) => {
                if from != TaskStatus::Prepared {
                    self.ensure_prepared_room(&sandbox).await?;
                }
                let what = at.map_or_else(|| "when I decide".into(), crate::auth::rfc3339);
                (TaskEvent::PreparedChosen, None, *at, false, what)
            }
            Choice::Now { supertask } => {
                let event = if from == TaskStatus::Chained {
                    TaskEvent::ChainReleased
                } else {
                    TaskEvent::PreparedReleased
                };
                let what = if *supertask { "supertask" } else { "now" };
                (event, None, None, *supertask, what.to_owned())
            }
        };
        let to = from
            .apply(event)
            .map_err(|_| AppError::Conflict(format!("the task is {}", from.as_str())))?;
        let store = &self.deps.store;
        let before = store.chain(&task.id).await?.and_then(|c| c.after_task_id);
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
        if let Some(before) = before {
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

    /// Checks a start choice as New task does (`create_task_as`), without the sandbox's bounds.
    async fn start_choice(&self, sandbox: &SandboxId, req: &ChangeStart) -> AppResult<Choice> {
        let after = match req.after_task_id.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(id) => {
                let id = TaskId::parse(id).map_err(|_| {
                    AppError::Invalid("the task to run after does not exist".into())
                })?;
                Some(self.live_predecessor(sandbox, &id).await?)
            }
        };
        let rule = match req.start_rule.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => Some(Rule::parse(text).map_err(|e| AppError::Invalid(e.to_string()))?),
        };
        if after.is_some() && rule.is_some() {
            return Err(AppError::Invalid(
                "give a task to run after or a start rule, not both".into(),
            ));
        }
        if req.supertask && (after.is_some() || rule.is_some()) {
            return Err(AppError::Invalid(
                "a supertask starts after the running tasks: it cannot also run after a task or a rule"
                    .into(),
            ));
        }
        let prepared = match req.start_at.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => {
                Some(Some(crate::prepared::parse_start_at(text, self.now())?))
            }
            _ => req.prepared.then_some(None),
        };
        if prepared.is_some() && (after.is_some() || rule.is_some() || req.supertask) {
            return Err(prepared_alone());
        }
        Ok(match (after, rule, prepared) {
            (Some(anchor), _, _) => Choice::Chain {
                anchor: Box::new(anchor),
                rule: None,
            },
            (None, Some(rule), _) => Choice::Chain {
                anchor: Box::new(self.rule_named_anchor(sandbox, &rule).await?),
                rule: Some(rule),
            },
            (None, None, Some(at)) => Choice::Prepared(at),
            (None, None, None) => Choice::Now {
                supertask: req.supertask,
            },
        })
    }

    /// Refuses a wait that would wait for `task` itself: the predecessor or a task the rule
    /// names is `task`, or waits for it through chained tasks' predecessors and rules (CHS-004).
    async fn refuse_waiting_for_itself(
        &self,
        task: &TaskRecord,
        anchor: &TaskRecord,
        rule: Option<&Rule>,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let tasks = store.tasks(&task.sandbox_id, false).await?;
        let by_number: HashMap<u32, &TaskRecord> = tasks.iter().map(|t| (t.number, t)).collect();
        let named = |numbers: Vec<u32>| -> Vec<TaskId> {
            numbers
                .into_iter()
                .filter_map(|n| by_number.get(&n).map(|t| t.id.clone()))
                .collect()
        };
        let mut pending = match rule {
            Some(rule) => named(rule.numbers()),
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
                    Some(Ok(rule)) => pending.extend(named(rule.numbers())),
                    Some(Err(_)) => {}
                    None => pending.extend(chain.after_task_id),
                }
            }
        }
        Ok(())
    }
}
