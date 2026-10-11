//! Chained tasks (SPEC-001 TSK-012; SPEC-002 §6.1 `chained`, ADR-16): a task that waits for
//! another task of its sandbox to be Done, then starts by itself.
//!
//! Rules, in full: a chained task starts when its predecessor is Done (the sidebar's rule: a
//! finished turn and a pull request merged, or open and tested). When the predecessor ends any
//! other way (stopped, failed, closed before it was done, deleted) the chained task needs a
//! person — Start now or Cancel — and never starts by itself; continuing the predecessor puts
//! it back to waiting. A chained task starts from its predecessor's branch until that branch is
//! merged, then from the integration branch.

use std::sync::Arc;

use yulius_contracts::api::tasks::{ChainLink, TaskSummary};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;
use yulius_domain::task::{TaskEvent, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::intake::ChainRecord;
use crate::model::{TaskOrigin, TaskRecord};
use crate::supervisor::RunStart;

/// Chained tasks waiting in one sandbox at most (the session queue's bound).
pub const MAX_CHAINED_PER_SANDBOX: usize = 32;

/// Why a waiting task needs a person, from its predecessor's state (`None` while that task may
/// still become Done).
fn blocked_reason(number: u32, status: TaskStatus) -> Option<String> {
    let what = match status {
        TaskStatus::Closed => "was closed",
        TaskStatus::Stopped(yulius_domain::task::StopReason::Cancelled) => "was stopped",
        TaskStatus::Stopped(_) => "stopped unexpectedly",
        TaskStatus::Failed(_) => "failed to start",
        _ => return None,
    };
    Some(format!(
        "#{number} {what} before it was done. Start this task now or cancel it."
    ))
}

impl App {
    /// The predecessor of a new chained task: a task of the same sandbox that has not ended.
    ///
    /// # Errors
    /// `Invalid` for an unknown, foreign, archived or ended task, or a full chain bound.
    pub(crate) async fn chain_predecessor(
        &self,
        sandbox: &SandboxId,
        after: &TaskId,
    ) -> AppResult<TaskRecord> {
        let store = &self.deps.store;
        let task = store
            .task(after)
            .await?
            .filter(|t| &t.sandbox_id == sandbox)
            .ok_or_else(|| AppError::Invalid("the task to run after does not exist".into()))?;
        if task.status.is_resting() || task.archived_at.is_some() {
            return Err(AppError::Invalid(format!(
                "#{} has ended; start the new task now instead",
                task.number
            )));
        }
        self.ensure_chain_room(sandbox).await?;
        Ok(task)
    }

    /// Refuses one more waiting task when the sandbox already has the most it may hold.
    pub(crate) async fn ensure_chain_room(&self, sandbox: &SandboxId) -> AppResult<()> {
        let waiting = self
            .deps
            .store
            .tasks(sandbox, false)
            .await?
            .iter()
            .filter(|t| t.status == TaskStatus::Chained)
            .count();
        if waiting >= MAX_CHAINED_PER_SANDBOX {
            return Err(AppError::Capacity(format!(
                "this sandbox already has {MAX_CHAINED_PER_SANDBOX} tasks waiting in chains"
            )));
        }
        Ok(())
    }

    /// Records the link of a new chained task.
    pub(crate) async fn link_chain(&self, task: &TaskRecord, after: &TaskRecord) -> AppResult<()> {
        self.deps
            .store
            .insert_chain(&ChainRecord {
                task_id: task.id.clone(),
                after_task_id: Some(after.id.clone()),
                after_number: after.number,
                blocked_reason: None,
                created_at: self.now(),
            })
            .await
    }

    /// The chain around a task as the API shows it: what it runs after, what runs after it.
    pub(crate) async fn chain_links(
        &self,
        task: &TaskRecord,
    ) -> (Option<Box<ChainLink>>, Vec<ChainLink>) {
        let store = &self.deps.store;
        let after = match store.chain(&task.id).await {
            Ok(Some(chain)) => {
                let predecessor = match &chain.after_task_id {
                    Some(id) => store.task(id).await.ok().flatten(),
                    None => None,
                };
                Some(Box::new(ChainLink {
                    task_id: predecessor.as_ref().map(|t| t.id.to_string()),
                    number: chain.after_number,
                    title: predecessor.as_ref().map(|t| t.title.clone()),
                    status: predecessor.as_ref().map(|t| t.status.as_str().to_owned()),
                    blocked: chain
                        .blocked_reason
                        .filter(|_| task.status == TaskStatus::Chained),
                }))
            }
            _ => None,
        };
        let mut next = Vec::new();
        for chain in store.chain_successors(&task.id).await.unwrap_or_default() {
            if let Ok(Some(successor)) = store.task(&chain.task_id).await {
                next.push(ChainLink {
                    task_id: Some(successor.id.to_string()),
                    number: successor.number,
                    title: Some(successor.title.clone()),
                    status: Some(successor.status.as_str().to_owned()),
                    blocked: chain
                        .blocked_reason
                        .filter(|_| successor.status == TaskStatus::Chained),
                });
            }
        }
        (after, next)
    }

    /// Re-evaluates the tasks waiting for `predecessor`: starts them when it is Done, marks
    /// them as needing a person when it ended otherwise, leaves them waiting meanwhile.
    pub async fn advance_chains(self: &Arc<Self>, predecessor: &TaskId) {
        let store = &self.deps.store;
        let Ok(chains) = store.chain_successors(predecessor).await else {
            return;
        };
        let mut waiting = Vec::new();
        for chain in chains.into_iter().filter(|c| c.blocked_reason.is_none()) {
            if let Ok(Some(task)) = store.task(&chain.task_id).await
                && task.status == TaskStatus::Chained
            {
                waiting.push(task);
            }
        }
        if waiting.is_empty() {
            return;
        }
        let Ok(Some(before)) = store.task(predecessor).await else {
            return;
        };
        // Tasks waiting for a sandbox's preparation start once the sandbox is ready or an admin
        // skips preparation (U-08), not when the preparation task's own pull request is green.
        if before.origin == TaskOrigin::Onboarding {
            let ready = self
                .refresh_onboarding(&before.sandbox_id)
                .await
                .is_ok_and(|state| {
                    !crate::onboarding::OnboardingState::parse(&state)
                        .is_some_and(crate::onboarding::OnboardingState::blocks_tasks)
                });
            if ready {
                for task in waiting {
                    if let Err(err) = self.release_chained(&task).await {
                        tracing::warn!(task = %task.id, error = %err, "waiting task did not start");
                    }
                }
            }
            return;
        }
        if self.task_is_done(&before).await {
            for task in waiting {
                if let Err(err) = self.release_chained(&task).await {
                    tracing::warn!(task = %task.id, error = %err, "chained task did not start");
                }
            }
            return;
        }
        if let Some(reason) = blocked_reason(before.number, before.status) {
            for task in waiting {
                self.block_chained(&task, &reason).await;
            }
        }
    }

    /// Marks every waiting successor of `predecessor` as needing a person (it is deleted).
    pub(crate) async fn block_successors_deleted(&self, predecessor: &TaskRecord) {
        let reason = format!(
            "#{} was deleted. Start this task now or cancel it.",
            predecessor.number
        );
        for chain in self
            .deps
            .store
            .chain_successors(&predecessor.id)
            .await
            .unwrap_or_default()
        {
            if let Ok(Some(task)) = self.deps.store.task(&chain.task_id).await
                && task.status == TaskStatus::Chained
            {
                self.block_chained(&task, &reason).await;
            }
        }
    }

    async fn block_chained(&self, task: &TaskRecord, reason: &str) {
        if self
            .deps
            .store
            .set_chain_blocked(&task.id, Some(reason))
            .await
            .is_ok()
        {
            self.runtime(&task.id)
                .hub
                .notice(format!("Yulius: {reason}"));
            self.publish_task(&task.id).await;
        }
    }

    /// Re-evaluates every waiting chained task (background tick).
    pub async fn advance_all_chains(self: &Arc<Self>) {
        let Ok(chained) = self
            .deps
            .store
            .tasks_with_status(&[TaskStatus::Chained])
            .await
        else {
            return;
        };
        let mut predecessors: Vec<TaskId> = Vec::new();
        for task in chained {
            if let Ok(Some(chain)) = self.deps.store.chain(&task.id).await
                && chain.blocked_reason.is_none()
                && let Some(after) = chain.after_task_id
                && !predecessors.contains(&after)
            {
                predecessors.push(after);
            }
        }
        for predecessor in predecessors {
            self.advance_chains(&predecessor).await;
        }
    }

    /// The predecessor was continued: its waiting successors wait for it again.
    pub(crate) async fn unblock_successors(&self, predecessor: &TaskId) {
        for chain in self
            .deps
            .store
            .chain_successors(predecessor)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.blocked_reason.is_some())
        {
            if let Ok(Some(task)) = self.deps.store.task(&chain.task_id).await
                && task.status == TaskStatus::Chained
                && self
                    .deps
                    .store
                    .set_chain_blocked(&task.id, None)
                    .await
                    .is_ok()
            {
                self.publish_task(&task.id).await;
            }
        }
    }

    /// `POST /api/tasks/{tid}/start-now`: a waiting chained task starts now, whatever its
    /// predecessor's state.
    ///
    /// # Errors
    /// Access errors, `Conflict` when the task is not waiting in a chain.
    pub async fn start_chained_now(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        if task.status != TaskStatus::Chained {
            return Err(AppError::Conflict(format!(
                "the task is {}, not waiting in a chain",
                task.status.as_str()
            )));
        }
        if !self.release_chained(&task).await? {
            return Err(AppError::Conflict("task state changed concurrently".into()));
        }
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Chained → queued (compare-and-set), then the normal start. `false` when it was no longer
    /// waiting.
    async fn release_chained(self: &Arc<Self>, task: &TaskRecord) -> AppResult<bool> {
        let next = task
            .status
            .apply(TaskEvent::ChainReleased)
            .map_err(|_| AppError::Conflict(format!("task is {}", task.status.as_str())))?;
        if !self
            .deps
            .store
            .transition_task(&task.id, Some(TaskStatus::Chained), next, None, self.now())
            .await?
        {
            return Ok(false);
        }
        self.deps.store.set_chain_blocked(&task.id, None).await?;
        self.publish_task(&task.id).await;
        let app = Arc::clone(self);
        let id = task.id.clone();
        tokio::spawn(async move {
            if let Err(err) = app
                .start_run(&id, RunStart::Fresh { prompt: None }, None)
                .await
            {
                tracing::warn!(error = %err, task = %id, "chained task failed to start");
            }
        });
        Ok(true)
    }

    /// The branch a chained task starts from: its predecessor's, until that is merged.
    pub(crate) async fn chain_base(&self, task: &TaskRecord) -> Option<String> {
        let store = &self.deps.store;
        let chain = store.chain(&task.id).await.ok()??;
        let before = store.task(&chain.after_task_id?).await.ok()??;
        let branch = before.branch?;
        if self.task_merged(&before.id).await {
            return None;
        }
        let repo = self.sandbox_repo(&task.sandbox_id);
        match self.deps.git.branch_exists(&repo, &branch).await {
            Ok(true) => Some(branch),
            _ => None,
        }
    }

    /// The briefing section of a chained task.
    pub(crate) async fn chain_briefing(&self, task: &TaskRecord) -> Option<String> {
        let store = &self.deps.store;
        let chain = store.chain(&task.id).await.ok()??;
        let before = match &chain.after_task_id {
            Some(id) => store.task(id).await.ok().flatten(),
            None => None,
        };
        let title = before.as_ref().map_or("(deleted)", |t| t.title.as_str());
        let branch = before
            .as_ref()
            .and_then(|t| t.branch.clone())
            .unwrap_or_default();
        Some(format!(
            "\n\n## Runs after #{n}\n\nThis task was queued to run after #{n} ({title}). It started from #{n}'s branch `{branch}` unless that was already merged; build on that work, and expect #{n}'s pull request to merge first.\n",
            n = chain.after_number
        ))
    }
}
