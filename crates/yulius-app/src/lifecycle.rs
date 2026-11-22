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
use yulius_domain::time::Timestamp;

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
    /// Runs alone in its project once the running work is finished (SPEC-071).
    pub supertask: bool,
}

/// Refuse another provider's model before creating a task or updating its settings.
pub(crate) fn model_of_its_harness(harness: Harness, model: Option<&str>) -> AppResult<()> {
    let Some(model) = model else {
        return Ok(());
    };
    match (harness, Harness::for_model_prefix(model)) {
        (Harness::Antigravity, owner) if owner != Some(Harness::Antigravity) => {
            Err(AppError::Invalid(format!(
                "an Antigravity task takes a Gemini model (such as {})",
                yulius_domain::harness::DEFAULT_ANTIGRAVITY_MODEL
            )))
        }
        (Harness::Codex, Some(Harness::ClaudeCode)) => Err(AppError::Invalid(
            "a Codex task takes an OpenAI model. Choose Claude Code for a Claude model.".into(),
        )),
        (_, Some(owner)) if owner != harness => Err(AppError::Invalid(format!(
            "Choose {} for this model. This task uses {}.",
            owner.display_name(),
            harness.display_name()
        ))),
        _ => Ok(()),
    }
}

pub(crate) fn parse_harness(text: &str) -> AppResult<Harness> {
    Harness::parse(text)
        .map_err(|_| AppError::Invalid("harness must be claude, codex or antigravity".into()))
}

/// Whether `account` has non-exhausted capacity right now for a default-model task of `harness`
/// (SPEC-520 RHM-001/002). A bare `Limited` status without usage data is treated as exhausted;
/// with usage data, the rotation policy's own exhaustion check decides.
pub(crate) fn harness_account_available(
    account: &crate::model::AccountRecord,
    harness: Harness,
    now: Timestamp,
) -> bool {
    if account.provider != harness.provider()
        || account.credential.is_none()
        || !account.status.healthy()
        || (account.status == crate::model::AccountStatus::Limited && account.usage_json.is_none())
    {
        return false;
    }
    let default_model = match harness {
        Harness::Codex => Some(yulius_domain::harness::DEFAULT_CODEX_MODEL),
        Harness::Antigravity => Some(yulius_domain::harness::DEFAULT_ANTIGRAVITY_MODEL),
        Harness::ClaudeCode => None,
    };
    let family = crate::capacity::model_family(default_model);
    let fallback = crate::capacity::fallback_of(default_model)
        .map(|fb| crate::capacity::model_family(Some(fb)));
    let usage = crate::accounts::account_usage(account);
    let threshold = yulius_domain::rotation::THRESHOLD;
    !usage.exhausted_for(now, threshold, family)
        || fallback.is_some_and(|fb| !usage.exhausted_for(now, threshold, fb))
}

/// Harnesses eligible for a random draw at `now` (SPEC-520 RHM-001/002): every harness with at
/// least one non-exhausted subscription gets one equal entry regardless of how many subscriptions
/// it has; when every connected pool is currently exhausted, harnesses with healthy signed-in
/// accounts waiting to reset are returned so the task queues instead of failing with no-account.
pub(crate) fn eligible_harnesses(
    accounts: &[crate::model::AccountRecord],
    now: Timestamp,
) -> Vec<Harness> {
    let available: Vec<Harness> = Harness::ALL
        .into_iter()
        .filter(|&h| {
            accounts
                .iter()
                .any(|a| harness_account_available(a, h, now))
        })
        .collect();
    if !available.is_empty() {
        return available;
    }
    Harness::ALL
        .into_iter()
        .filter(|&h| {
            crate::account_gate::agent_accounts(accounts, h.provider())
                == crate::account_gate::AgentAccounts::Usable
        })
        .collect()
}

impl App {
    /// Draws a harness uniformly across [`eligible_harnesses`] (`ClaudeCode` when no harness has
    /// any healthy subscription; SPEC-520 RHM-001/002, ADR-0377).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn draw_harness(&self) -> AppResult<Harness> {
        let accounts = self.deps.store.accounts().await?;
        let candidates = eligible_harnesses(&accounts, self.now());
        let queued = self
            .harness_rolls
            .lock()
            .ok()
            .and_then(|mut rolls| rolls.pop_front());
        if let Some(roll) = queued {
            return Ok(Harness::drawn(&candidates, roll));
        }
        if candidates.is_empty() {
            return Ok(Harness::ClaudeCode);
        }
        let mut last = 0_u8;
        for _ in 0..8 {
            let bytes = self.deps.crypto.random_bytes();
            last = bytes[0];
            if let Some(idx) = Harness::unbiased_index(candidates.len(), &bytes) {
                return Ok(candidates[idx]);
            }
        }
        Ok(Harness::drawn(&candidates, last))
    }

    /// Queues the rolls of the next harness draws, in order (tests: a draw is otherwise random).
    pub fn queue_harness_rolls(&self, rolls: &[u8]) {
        if let Ok(mut queued) = self.harness_rolls.lock() {
            queued.extend(rolls);
        }
    }
}

/// A prepared task has one way to start (SPEC-105 PRP-001).
pub(crate) fn prepared_alone() -> AppError {
    AppError::Invalid(
        "a prepared task starts when you decide or at its time: it cannot also run after a task, \
         by a rule or as a supertask"
            .into(),
    )
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
        self.create_task_held(req, after, rule, None).await
    }

    /// [`App::create_task_ruled`], or `prepared` (SPEC-105): `Some(None)` waits until a person
    /// starts it, `Some(Some(time))` until that time. A prepared task has no predecessor or rule.
    ///
    /// # Errors
    /// As [`App::create_task_ruled`]; `Invalid` for a prepared task with a predecessor or rule,
    /// `Capacity` beyond the sandbox's prepared tasks.
    pub async fn create_task_held(
        self: &Arc<Self>,
        req: NewTask,
        after: Option<&TaskRecord>,
        rule: Option<&Rule>,
        prepared: Option<Option<Timestamp>>,
    ) -> AppResult<TaskRecord> {
        self.create_task_drawn(req, after, rule, prepared, false)
            .await
    }

    pub(crate) async fn create_task_drawn(
        self: &Arc<Self>,
        req: NewTask,
        after: Option<&TaskRecord>,
        rule: Option<&Rule>,
        prepared: Option<Option<Timestamp>>,
        harness_drawn: bool,
    ) -> AppResult<TaskRecord> {
        let store = &self.deps.store;
        if prepared.is_some() && (after.is_some() || rule.is_some() || req.supertask) {
            return Err(prepared_alone());
        }
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
        // A prepared task waits for onboarding when it is released instead (PRP-003).
        let after = match after {
            Some(task) => Some(task),
            None if prepared.is_some() => {
                self.ensure_prepared_room(&sandbox.id).await?;
                None
            }
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
        let status = if after.is_some() {
            TaskStatus::Chained
        } else if prepared.is_some() {
            TaskStatus::Prepared
        } else if req.start {
            TaskStatus::Queued
        } else {
            TaskStatus::Closed
        };
        let record = TaskRecord {
            resume_attempt_id: None,
            id: TaskId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            sandbox_id: sandbox.id.clone(),
            number: 0,
            title,
            prompt,
            harness: req.harness,
            model: req.model.clone(),
            effort: req.effort.clone(),
            workflow_id: req.workflow_id.clone(),
            status,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: req.origin,
            automation_run: req.automation_run.clone(),
            created_by: req.created_by.clone(),
            owner: req.created_by.clone(),
            created_at: now,
            updated_at: now,
            // The time of the close the task is in (SPEC-483 CLT-001).
            closed_at: (status == TaskStatus::Closed).then_some(now),
            archived_at: (!req.start).then_some(now),
            queued_at: (req.start && after.is_none() && prepared.is_none()).then_some(now),
            gate_blocks: 0,
            wake_enabled: true,
            mode: req.mode,
            supertask: req.supertask,
        };
        let task = store.insert_task(&record).await?;
        if let Some(before) = &after {
            self.link_chain(&task, before, rule).await?;
        }
        if let Some(Some(start_at)) = prepared {
            store
                .set_task_schedule(&task.id, Some(start_at), now)
                .await?;
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
        if harness_drawn {
            store.record_harness_draw(&task.id, now).await?;
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
        } else if let Some(start_at) = prepared {
            // A prepared task never starts here; a time within the grace minute starts it now
            // rather than on the next tick.
            if start_at.is_some() {
                self.release_if_due(&task).await;
            }
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
            // The project's queued tasks now wait for the new supertask: their labels say so,
            // also at the session limit (SPEC-071 SUP-006).
            if req.supertask {
                self.drain_queue().await;
            }
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
        // No agent given (or `auto`): the one `model` implies when given, else drawn uniformly
        // across harnesses with non-exhausted subscriptions once inputs validate (SPEC-520
        // RHM-001..003).
        let model = req
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        let explicit_harness = match req.harness.trim() {
            "" | AUTO => model.map(Harness::implied_by),
            text => Some(parse_harness(text)?),
        };
        if let Some(harness) = explicit_harness {
            model_of_its_harness(harness, model)?;
        }
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
        // The start choice, checked as a change of a waiting task checks it (SPEC-126).
        let crate::start_change::CheckedStart {
            after,
            rule,
            prepared,
            ..
        } = self
            .check_start_choice(
                &sid,
                crate::start_change::StartFields {
                    after_task_id: req.after_task_id.as_deref(),
                    start_rule: req.start_rule.as_deref(),
                    supertask: req.supertask,
                    prepared: req.prepared,
                    start_at: req.start_at.as_deref(),
                },
            )
            .await?;
        if after.is_some() {
            self.ensure_chain_room(&sid).await?;
        }
        // Jev is asked only about a prompt the task accepts.
        let prompt = req.prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(
                "the prompt must be 1 byte to 64 KiB".into(),
            ));
        }
        let (harness, drawn) = match explicit_harness {
            Some(harness) => (harness, false),
            None => (self.draw_harness().await?, true),
        };
        // A task that starts (now, after a task, by a rule) needs an agent: refused without a
        // usable account before anything is created (SPEC-246 CLA-002); a prepared one starts
        // only when a person says so.
        if prepared.is_none() {
            self.ensure_agent_account(harness.provider()).await?;
        }
        let choice = self.auto_choose(&sid, prompt, mode, workflow).await?;
        let task = self
            .create_task_drawn(
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
                    model: model.map(str::to_owned),
                    effort: req.effort.clone().filter(|e| !e.trim().is_empty()),
                    origin,
                    automation_run: None,
                    start: true,
                    mode: choice.mode,
                    supertask: req.supertask,
                },
                after.as_ref(),
                rule.as_ref(),
                prepared,
                drawn,
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

    /// `POST /api/tasks/{tid}/stop`: anyone who may work in the project stops a task, or
    /// cancels one that waits to start (SPEC-490 OWN-002).
    ///
    /// # Errors
    /// Access errors, `Conflict` when not stoppable.
    pub async fn stop_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        self.stop_task_as(ctx, id, None).await
    }

    /// [`App::stop_task`] for a person on a page, or for a `tool` acting for them, which its
    /// audit then names. A stop by someone who does not own the task is audited with the owner
    /// and said in the task's terminal (SPEC-490 OWN-005).
    ///
    /// # Errors
    /// As [`App::stop_task`].
    pub(crate) async fn stop_task_as(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        tool: Option<crate::tasks::ToolAudit<'_>>,
    ) -> AppResult<TaskSummary> {
        let task = self
            .load_task_to(ctx, id, yulius_domain::task_access::TaskAction::Tidy)
            .await?;
        self.stop_task_now(&task.id).await?;
        self.record_tidy(ctx, &task, "task.stopped", tool).await;
        self.say_ended_by_another(ctx, &task, "Stopped").await;
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
        self.continue_task_with_prompt(ctx, id, req, req.prompt.as_deref())
            .await
    }

    /// [`App::continue_task`] with an optional prompt delivered on wake for a parked task
    /// (SPEC-553 PRK-102). A prompt for a non-parked task is ignored (normal continue).
    ///
    /// # Errors
    /// As [`App::continue_task`], plus `Invalid` when `prompt` exceeds [`crate::task_message::MAX_MESSAGE_BYTES`].
    pub async fn continue_task_with_prompt(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ContinueTask,
        prompt: Option<&str>,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        self.continue_loaded_task(ctx, task, req, prompt).await
    }

    /// Continue the task returned by the caller's work access check.
    pub(crate) async fn continue_loaded_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        task: TaskRecord,
        req: &ContinueTask,
        prompt: Option<&str>,
    ) -> AppResult<TaskSummary> {
        // A parked task resumes in its session (SPEC-512 PRK-005): with nothing typed, its
        // request stays until the person writes; with a prompt, it wakes with that prompt and
        // ends any open `needs_you` request (SPEC-553 PRK-102).
        if task.status == TaskStatus::Parked {
            if parse_account(req)?.is_some() {
                return Err(AppError::Invalid(
                    "a parked task resumes where it ran; move it after it resumed".into(),
                ));
            }
            if let Some(raw) = prompt
                && raw.len() > crate::task_message::MAX_MESSAGE_BYTES
            {
                return Err(AppError::Invalid(format!(
                    "prompt exceeds {} bytes",
                    crate::task_message::MAX_MESSAGE_BYTES
                )));
            }
            let cleaned = prompt
                .map(crate::task_message::clean_message)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            if let Some(text) = cleaned.as_deref()
                && text.len() > crate::task_message::MAX_MESSAGE_BYTES
            {
                return Err(AppError::Invalid(format!(
                    "prompt exceeds {} bytes",
                    crate::task_message::MAX_MESSAGE_BYTES
                )));
            }
            self.ensure_agent_account(task.harness.provider()).await?;
            if !self.resume_parked(&task, cleaned.clone()).await? {
                return Err(AppError::Conflict(
                    "the task changed while it was resumed; try again".into(),
                ));
            }
            if cleaned.is_some() {
                self.end_needs_you(&task.id).await;
            }
            let task = self
                .deps
                .store
                .task(&task.id)
                .await?
                .ok_or(AppError::NotFound)?;
            return self.task_summary(&task).await;
        }
        if !task.status.is_resting() {
            return Err(AppError::Conflict(format!(
                "the task is {}",
                task.status.as_str()
            )));
        }
        // Continuing a pending sandbox's onboarding task starts onboarding: a person only, in the
        // browser or through their operate token (ADR-0188).
        self.ensure_onboarding_continue_allowed(ctx, &task).await?;
        // Continuing is starting a task: refused while the sandbox is being prepared (U-08).
        self.ensure_tasks_may_start(&task.sandbox_id, task.origin)
            .await?;
        self.ensure_agent_account(task.harness.provider()).await?;
        let account = parse_account(req)?;
        // A subscription asked for by name must still exist: "Continue on…" never lands elsewhere
        // unsaid (SPEC-447 MOV-002). It must be of the task's provider too, refused before the task
        // starts (SPEC-503 CDX-013): a task never changes harness.
        if let Some(id) = &account {
            let provider = task.harness.provider();
            match self.deps.store.account(id).await? {
                None => {
                    return Err(AppError::Capacity(
                        "That subscription no longer exists.".into(),
                    ));
                }
                Some(record) if record.provider != provider => {
                    return Err(AppError::Capacity(format!(
                        "{} is not {} subscription, so it cannot take the task.",
                        record.label,
                        crate::account_gate::a_kind(provider)
                    )));
                }
                Some(_) => {}
            }
        }
        crate::race::reached(
            crate::race::Point::OwnerContinueRequested,
            std::path::Path::new(task.id.as_str()),
        );
        self.start_ended_run(&task, account).await?;
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

    /// `POST /api/tasks/{tid}/switch`: a person moves a running or waiting task to the
    /// subscription they choose, or the pool's best other one (SPEC-447 MOV-001, ROT-007).
    ///
    /// # Errors
    /// `Conflict` when not running or waiting, `Capacity` when the subscription cannot take it.
    pub async fn switch_task(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &ContinueTask,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let account = parse_account(req)?;
        // A paused run would be ended by the switch (SPEC-191 PER-005).
        if self.is_paused_by_person(&task.id).await {
            return Err(AppError::Conflict("Resume the task first.".into()));
        }
        // Where the task stands decides how it goes on (SPEC-447 MOV-001): past a limit, its first
        // message again, a held prompt, or the cut turn told to go on.
        self.move_task(&task.id, account, "manual").await?;
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
