//! Task creation and lifecycle use cases exposed to the API and to other services
//! (SPEC-001 TSK-001..008, HRN-002, ROT-007).

use std::sync::Arc;

use yulius_contracts::api::tasks::{ContinueTask, CreateTask, TaskSummary};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{StepRecord, TaskOrigin, TaskRecord};
use crate::ports::store::TaskFieldUpdate;
use crate::supervisor::{RunStart, branch_for};
use crate::tasks::title_from_prompt;

/// Maximum prompt size.
pub const MAX_PROMPT_BYTES: usize = 64 * 1024;

/// Internal task creation request (API, automations, imports, onboarding, agents).
#[derive(Debug, Clone)]
pub struct NewTask {
    /// Sandbox.
    pub sandbox: SandboxId,
    /// Creator.
    pub created_by: UserId,
    /// Prompt.
    pub prompt: String,
    /// Harness.
    pub harness: Harness,
    /// Title (derived when absent).
    pub title: Option<String>,
    /// Workflow id.
    pub workflow_id: Option<String>,
    /// Initial steps.
    pub steps: Vec<String>,
    /// Model.
    pub model: Option<String>,
    /// Effort.
    pub effort: Option<String>,
    /// Origin.
    pub origin: TaskOrigin,
    /// Automation run key.
    pub automation_run: Option<String>,
    /// Start a run immediately (false for imported/archived tasks).
    pub start: bool,
}

fn parse_harness(text: &str) -> AppResult<Harness> {
    Harness::parse(text).map_err(|_| AppError::Invalid("harness must be claude or codex".into()))
}

impl App {
    /// Creates a task record (and optionally starts it). Enforces the onboarding gate (U-08).
    ///
    /// # Errors
    /// `Invalid` input, `Conflict` while the sandbox is being prepared.
    pub async fn create_task_internal(self: &Arc<Self>, req: NewTask) -> AppResult<TaskRecord> {
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(&req.sandbox)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let onboarding = self.refresh_onboarding(&sandbox.id).await?;
        if matches!(onboarding.as_str(), "pending" | "preparing")
            && req.origin != TaskOrigin::Onboarding
            && req.origin != TaskOrigin::Import
        {
            return Err(AppError::Conflict("this project is being prepared for Yulius; wait for the preparation task or ask an admin to skip it".into()));
        }
        let prompt = req.prompt.trim().to_owned();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(
                "the prompt must be 1 byte to 64 KiB".into(),
            ));
        }
        let now = self.now();
        let title = req
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .map_or_else(
                || title_from_prompt(&prompt),
                |t| t.chars().take(120).collect(),
            );
        let record = TaskRecord {
            id: TaskId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            sandbox_id: sandbox.id.clone(),
            number: 0,
            title,
            prompt,
            harness: req.harness,
            model: req.model.clone(),
            effort: req.effort.clone(),
            workflow_id: req.workflow_id.clone(),
            status: if req.start {
                TaskStatus::Queued
            } else {
                TaskStatus::Closed
            },
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: req.origin,
            automation_run: req.automation_run.clone(),
            created_by: req.created_by.clone(),
            created_at: now,
            updated_at: now,
            closed_at: (!req.start).then_some(now),
            archived_at: (!req.start).then_some(now),
            queued_at: req.start.then_some(now),
            gate_blocks: 0,
            wake_enabled: true,
        };
        let task = store.insert_task(&record).await?;
        let branch = branch_for(task.number, &task.title);
        store
            .update_task(
                &task.id,
                &TaskFieldUpdate {
                    branch: Some(branch),
                    ..TaskFieldUpdate::default()
                },
                now,
            )
            .await?;
        if !req.steps.is_empty() {
            let mut steps = Vec::new();
            for (i, title) in req
                .steps
                .iter()
                .take(crate::coordination::MAX_STEPS)
                .enumerate()
            {
                steps.push(StepRecord {
                    id: StepId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
                    task_id: task.id.clone(),
                    ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                    title: title.clone(),
                    status: "pending".into(),
                    note: None,
                    updated_by: Some("system".into()),
                    updated_at: Some(now),
                });
            }
            store.replace_steps(&task.id, &steps, "system", now).await?;
        }
        store
            .audit(
                now,
                &self.login_of(&req.created_by).await,
                "task.created",
                Some(task.id.as_str()),
                Some(req.origin.as_str()),
            )
            .await?;
        self.publish_task(&task.id).await;
        if req.start {
            let app = self.clone();
            let id = task.id.clone();
            tokio::spawn(async move {
                if let Err(err) = app
                    .start_run(&id, RunStart::Fresh { prompt: None }, None, None)
                    .await
                {
                    tracing::warn!(error = %err, task = %id, "task failed to start");
                }
            });
        }
        store.task(&task.id).await?.ok_or(AppError::NotFound)
    }

    /// `POST /api/sandboxes/{sid}/tasks`.
    ///
    /// # Errors
    /// Access, validation and onboarding-gate errors.
    pub async fn create_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
        req: &CreateTask,
    ) -> AppResult<TaskSummary> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Work).await?;
        let harness = parse_harness(&req.harness)?;
        let steps = self
            .workflow_steps(&sid, req.workflow_id.as_deref())
            .await?;
        let task = self
            .create_task_internal(NewTask {
                sandbox: sid,
                created_by: ctx.user.id.clone(),
                prompt: req.prompt.clone(),
                harness,
                title: req.title.clone(),
                workflow_id: req.workflow_id.clone(),
                steps,
                model: req.model.clone().filter(|m| !m.trim().is_empty()),
                effort: req.effort.clone().filter(|e| !e.trim().is_empty()),
                origin: TaskOrigin::Ui,
                automation_run: None,
                start: true,
            })
            .await?;
        self.task_summary(&task).await
    }

    /// Initial steps of a workflow (CP7 provides workflows; none yet → no steps).
    ///
    /// # Errors
    /// Store failures.
    pub async fn workflow_steps(
        &self,
        _sandbox: &SandboxId,
        _workflow: Option<&str>,
    ) -> AppResult<Vec<String>> {
        Ok(Vec::new())
    }

    /// `POST /api/tasks/{tid}/stop`.
    ///
    /// # Errors
    /// Access errors, `Conflict` when not stoppable.
    pub async fn stop_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        self.stop_task_now(&task.id).await?;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// `POST /api/tasks/{tid}/continue`: resumes the latest session (optionally on another
    /// harness or account) — TSK-007, IMP-004.
    ///
    /// # Errors
    /// `Conflict` when the task is live.
    pub async fn continue_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ContinueTask,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        if !task.status.is_resting() {
            return Err(AppError::Conflict(format!(
                "the task is {}",
                task.status.as_str()
            )));
        }
        let harness = req.harness.as_deref().map(parse_harness).transpose()?;
        let account = req
            .account_id
            .as_deref()
            .map(AccountId::parse)
            .transpose()
            .map_err(|_| AppError::Invalid("invalid account".into()))?;
        if task.archived_at.is_some() {
            let now = self.now();
            self.deps
                .store
                .update_task(
                    &task.id,
                    &TaskFieldUpdate {
                        archived_at: Some(None),
                        ..TaskFieldUpdate::default()
                    },
                    now,
                )
                .await?;
        }
        self.start_run(
            &task.id,
            RunStart::Resume { prompt: None },
            harness,
            account,
        )
        .await?;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// `POST /api/tasks/{tid}/switch`: manual account/harness switch of a running task (ROT-007).
    ///
    /// # Errors
    /// `Conflict` when not running.
    pub async fn switch_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ContinueTask,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let harness = req.harness.as_deref().map(parse_harness).transpose()?;
        let account = req
            .account_id
            .as_deref()
            .map(AccountId::parse)
            .transpose()
            .map_err(|_| AppError::Invalid("invalid account".into()))?;
        self.switch_task_now(
            &task.id,
            harness,
            account,
            "manual",
            Some("Continue the task exactly where you left off.".into()),
        )
        .await?;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }
}
