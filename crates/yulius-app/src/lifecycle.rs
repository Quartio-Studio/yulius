//! Task creation and lifecycle use cases exposed to the API and to other services
//! (SPEC-001 TSK-001..008, HRN-002, ROT-007).

use std::sync::Arc;

use yulius_contracts::api::tasks::{ContinueTask, CreateTask, TaskSummary};
use yulius_domain::auto_choice::AUTO;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AccountId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::rule::Rule;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::auto_choice::WorkflowPick;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{StepRecord, TaskMode, TaskOrigin, TaskRecord, UserRecord};
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
    /// Goal or prompt mode (TSK-010).
    pub mode: TaskMode,
}

fn parse_harness(text: &str) -> AppResult<Harness> {
    Harness::parse(text).map_err(|_| {
        AppError::Invalid("harness must be claude (Yulius runs Claude Code only)".into())
    })
}

impl App {
    /// Creates a task record (and optionally starts it). Enforces the onboarding gate (U-08).
    ///
    /// # Errors
    /// `Invalid` input, `Conflict` while the sandbox is being prepared.
    pub async fn create_task_internal(self: &Arc<Self>, req: NewTask) -> AppResult<TaskRecord> {
        self.create_task_with(req, None).await
    }

    /// [`App::create_task_internal`], optionally chained after `after` (SPEC-001 TSK-012): the
    /// task then waits as `chained` instead of starting, and starts by itself once `after` is
    /// Done (at once when it already is).
    ///
    /// # Errors
    /// As [`App::create_task_internal`]; `Invalid`/`Capacity` for an unusable predecessor.
    pub async fn create_task_with(
        self: &Arc<Self>,
        req: NewTask,
        after: Option<&TaskRecord>,
    ) -> AppResult<TaskRecord> {
        self.create_task_ruled(req, after, None).await
    }

    /// [`App::create_task_with`], or waiting for a start `rule` over other tasks of the sandbox
    /// instead of one predecessor (SPEC-017): the task starts by itself once the rule is true.
    ///
    /// # Errors
    /// As [`App::create_task_with`]; `Invalid` for a rule naming an unknown task, or a rule
    /// together with `after`.
    pub async fn create_task_ruled(
        self: &Arc<Self>,
        req: NewTask,
        after: Option<&TaskRecord>,
        rule: Option<&Rule>,
    ) -> AppResult<TaskRecord> {
        let store = &self.deps.store;
        let after = match (after, rule) {
            (Some(_), Some(_)) => {
                return Err(AppError::Invalid(
                    "give a task to run after or a start rule, not both".into(),
                ));
            }
            (None, Some(rule)) => Some(self.rule_anchor(&req.sandbox, rule).await?),
            (Some(task), None) => Some(self.chain_predecessor(&req.sandbox, &task.id).await?),
            (None, None) => None,
        };
        let sandbox = store
            .sandbox(&req.sandbox)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        // While the sandbox is being prepared a new task is not refused: it waits after the
        // preparation task and starts by itself once the sandbox is ready (U-08, IMP-010).
        let after = match after {
            Some(task) => Some(task),
            None if req.start => match self.onboarding_wait(&sandbox.id, req.origin).await? {
                Some(preparation) => {
                    self.ensure_chain_room(&sandbox.id).await?;
                    Some(preparation)
                }
                None => None,
            },
            None => None,
        };
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
            status: if after.is_some() {
                TaskStatus::Chained
            } else if req.start {
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
            queued_at: (req.start && after.is_none()).then_some(now),
            gate_blocks: 0,
            wake_enabled: true,
            mode: req.mode,
        };
        let task = store.insert_task(&record).await?;
        if let Some(before) = &after {
            self.link_chain(&task, before, rule).await?;
        }
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
                    effort: 1,
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
        if let Some(before) = &after {
            // The predecessor may already be Done: start at once then.
            self.advance_chains(&before.id).await;
            self.publish_task(&before.id).await;
        } else if req.start {
            let app = self.clone();
            let id = task.id.clone();
            tokio::spawn(async move {
                if let Err(err) = app
                    .start_run(&id, RunStart::Fresh { prompt: None }, None)
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
        let task = self
            .create_task_as(&ctx.user, &sid, req, TaskOrigin::Ui)
            .await?;
        self.task_summary(&task).await
    }

    /// Creates a task on `user`'s authority the way New task does (access, workflow or Jev's
    /// choice, mode, a predecessor or a start rule, the onboarding gate), from the UI or from an
    /// agent tool (SPEC-037 MGT-001).
    ///
    /// # Errors
    /// Access, validation and onboarding-gate errors.
    pub async fn create_task_as(
        self: &Arc<Self>,
        user: &UserRecord,
        sid: &SandboxId,
        req: &CreateTask,
        origin: TaskOrigin,
    ) -> AppResult<TaskRecord> {
        let sid = sid.clone();
        self.require(user, &sid, Action::Work).await?;
        let harness = parse_harness(&req.harness)?;
        // New tasks run in goal mode unless the user picks prompt mode (TSK-010); `auto` lets Jev
        // choose (TSK-015).
        let mode = match req.mode.as_deref().map(str::trim) {
            None | Some("") => Some(TaskMode::Goal),
            Some(AUTO) => None,
            Some(text) => Some(
                TaskMode::parse(text)
                    .ok_or_else(|| AppError::Invalid("mode must be goal, prompt or auto".into()))?,
            ),
        };
        let workflow = match req.workflow_id.as_deref().map(str::trim) {
            None | Some("") => WorkflowPick::None,
            Some(AUTO) => WorkflowPick::Auto,
            Some(id) => WorkflowPick::Given(Box::new(self.visible_workflow(&sid, id).await?)),
        };
        let after = match req.after_task_id.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(id) => {
                let id = TaskId::parse(id).map_err(|_| {
                    AppError::Invalid("the task to run after does not exist".into())
                })?;
                Some(self.chain_predecessor(&sid, &id).await?)
            }
        };
        let rule = match req.start_rule.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => Some(Rule::parse(text).map_err(|e| AppError::Invalid(e.to_string()))?),
        };
        // Jev is asked only about a prompt the task accepts.
        let prompt = req.prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(
                "the prompt must be 1 byte to 64 KiB".into(),
            ));
        }
        let choice = self.auto_choose(&sid, prompt, mode, workflow).await?;
        let task = self
            .create_task_ruled(
                NewTask {
                    sandbox: sid,
                    created_by: user.id.clone(),
                    prompt: req.prompt.clone(),
                    harness,
                    title: req.title.clone(),
                    workflow_id: choice.workflow.as_ref().map(|w| w.id.clone()),
                    steps: choice
                        .workflow
                        .as_ref()
                        .map(|w| w.steps.clone())
                        .unwrap_or_default(),
                    model: req.model.clone().filter(|m| !m.trim().is_empty()),
                    effort: req.effort.clone().filter(|e| !e.trim().is_empty()),
                    origin,
                    automation_run: None,
                    start: true,
                    mode: choice.mode,
                },
                after.as_ref(),
                rule.as_ref(),
            )
            .await?;
        if choice.is_auto() {
            // The task exists and runs: a failed record only loses the "Auto →" label.
            let actor = self.login_of(&user.id).await;
            if let Err(err) = self.record_auto_choice(&task.id, &actor, &choice).await {
                tracing::warn!(error = %err, task = %task.id, "auto choice not recorded");
            }
            self.publish_task(&task.id).await;
        }
        Ok(task)
    }

    /// Initial steps of a workflow usable in `sandbox` (no workflow → no steps; AUT-004).
    ///
    /// # Errors
    /// `Invalid` for an unknown workflow or one of another sandbox; store failures.
    pub async fn workflow_steps(
        &self,
        sandbox: &SandboxId,
        workflow: Option<&str>,
    ) -> AppResult<Vec<String>> {
        match workflow.map(str::trim).filter(|w| !w.is_empty()) {
            None => Ok(Vec::new()),
            Some(id) => Ok(self.visible_workflow(sandbox, id).await?.steps),
        }
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
    /// account) — TSK-007, IMP-004.
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
        // Continuing a pending sandbox's onboarding task starts onboarding: browser only.
        self.ensure_onboarding_continue_allowed(ctx, &task).await?;
        // Continuing is starting a task: refused while the sandbox is being prepared (U-08).
        self.ensure_tasks_may_start(&task.sandbox_id, task.origin)
            .await?;
        let account = parse_account(req)?;
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
        self.start_run(&task.id, RunStart::Resume { prompt: None }, account)
            .await?;
        // Its waiting successors wait for it again (TSK-012).
        self.unblock_successors(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// `POST /api/tasks/{tid}/switch`: manual account switch of a running task (ROT-007).
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
        let account = parse_account(req)?;
        // A turn a usage limit cut goes on past the limit, with nothing typed (SPEC-034).
        let at_limit = matches!(
            self.deps.store.open_pause(&task.id).await?,
            Some(pause) if pause.kind == crate::model::PauseKind::Limit
        );
        if at_limit {
            self.switch_after_limit(&task.id, account, "manual").await?;
        } else if self.held_opening(&task.id).await {
            // The session never took its first message: a fresh one types it with its images
            // (SPEC-068 OPN-003).
            let start = crate::supervisor::RunStart::Fresh { prompt: None };
            self.switch_with(&task.id, account, "manual", start).await?;
        } else {
            self.switch_task_now(
                &task.id,
                account,
                "manual",
                Some("Continue the task exactly where you left off.".into()),
            )
            .await?;
        }
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }
}

/// The requested account of a Continue or Switch, if any.
fn parse_account(req: &ContinueTask) -> AppResult<Option<AccountId>> {
    req.account_id
        .as_deref()
        .map(AccountId::parse)
        .transpose()
        .map_err(|_| AppError::Invalid("invalid account".into()))
}
