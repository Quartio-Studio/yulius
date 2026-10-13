//! Task intake (SPEC-001 TSK-011, TSK-013; SPEC-002 ADR-16): the split check and proposal for a
//! new task, the scope check of prompts typed into a running task (the `UserPromptSubmit`
//! hook), and the decisions on a held prompt.
//!
//! Without a Jev key nothing is checked and nothing is held; every Jev failure means "no
//! answer". A held prompt never reaches the agent: Claude Code shows the person the reason and
//! keeps the text in its input box (verified with Claude Code 2.1.295), and the task view shows
//! a card with the choices.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use yulius_contracts::api::intake::{
    HeldPromptOutcome, IntakeRequest, IntakeVerdict, ProposedTaskView, ResolveHeldPrompt,
    SplitProposal,
};
use yulius_contracts::api::tasks::HeldPrompt;
use yulius_contracts::hooks::{HookInput, HookOutput};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::intake::{
    self as policy, ChoiceAnswer, PromptClass, Proposal, SCOPE_QUESTION, SPLIT_QUESTION,
    SplitVerdict,
};
use yulius_domain::permission::Action;
use yulius_domain::steps::StepStatus;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::lifecycle::{MAX_PROMPT_BYTES, NewTask};
use crate::model::{TaskOrigin, TaskRecord};
use crate::ports::intake::{ChoiceEvaluator, SplitProposer};
use crate::ports::store::TaskFieldUpdate;

/// How long the composer's split check may take (it runs after Enter, before the task exists).
pub const SPLIT_CHECK_DEADLINE: Duration = Duration::from_millis(2_500);
/// How long a scope check may take: inside the hook client's 5 s budget, so a slow Jev lets the
/// prompt through instead of failing the hook.
pub const SCOPE_CHECK_DEADLINE: Duration = Duration::from_millis(3_000);
/// Split proposals running at once (each is one headless Claude run).
pub const MAX_PROPOSALS: usize = 2;
/// How long "cancel and revert" waits for the task's process to be reaped.
pub const REVERT_STOP_WAIT: Duration = Duration::from_secs(30);
/// Longest prompt excerpt in a task summary.
const EXCERPT_CHARS: usize = 2_000;

/// States of a held prompt.
pub mod hold_state {
    /// Waiting for a decision (shown as a card).
    pub const PENDING: &str = "pending";
    /// "Send anyway": the identical prompt passes once.
    pub const SENT: &str = "sent";
    /// The sent prompt reached the agent.
    pub const CONSUMED: &str = "consumed";
    /// Started as a parallel task.
    pub const PARALLEL: &str = "parallel";
    /// Chained after this task.
    pub const AFTER: &str = "after";
    /// Steps skipped and the new direction chained.
    pub const SKIPPED_AFTER: &str = "skipped_after";
    /// This task cancelled and reverted, a new task started.
    pub const REVERTED: &str = "reverted";
    /// Closed without action.
    pub const DISMISSED: &str = "dismissed";
    /// A newer held prompt of the same task replaced it.
    pub const SUPERSEDED: &str = "superseded";
}

/// A chained task's link to its predecessor (SPEC-001 TSK-012).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainRecord {
    /// The chained task.
    pub task_id: TaskId,
    /// Its predecessor; `None` once the predecessor was deleted.
    pub after_task_id: Option<TaskId>,
    /// The predecessor's number (kept after deletion).
    pub after_number: u32,
    /// Why the waiting task needs a person (it will not start by itself).
    pub blocked_reason: Option<String>,
    /// When the link was made.
    pub created_at: Timestamp,
    /// The start rule text (SPEC-017); `None` means the predecessor is Done.
    pub rule: Option<String>,
}

/// A prompt held by the scope check (SPEC-001 TSK-013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldRecord {
    /// Id.
    pub id: String,
    /// The task it was typed into.
    pub task_id: TaskId,
    /// Its class (never `Context`).
    pub class: PromptClass,
    /// The full prompt.
    pub prompt: String,
    /// SHA-256 hex of the trimmed prompt (matches the identical prompt on "send anyway").
    pub prompt_sha256: String,
    /// Jev's probability of the class, in percent.
    pub percent: u32,
    /// Who typed into the task's terminal last before it was submitted.
    pub typed_by: Option<UserId>,
    /// One of [`hold_state`].
    pub state: String,
    /// When it was held.
    pub created_at: Timestamp,
    /// Who decided (`user:<login>`).
    pub resolved_by: Option<String>,
    /// When.
    pub resolved_at: Option<Timestamp>,
    /// The task created for it.
    pub new_task_id: Option<TaskId>,
}

/// The intake services an instance runs with. Without a Jev evaluator nothing is checked.
pub struct IntakeServices {
    /// TypeSafe's Jev in use; replaced at once when a superadmin sets or removes the key
    /// (SPEC-001 ADM-002, `jev_key.rs`).
    pub(crate) jev: std::sync::RwLock<Option<crate::jev_key::ActiveJev>>,
    /// Builds evaluators from keys (none: the key cannot be managed, e.g. in tests).
    pub(crate) connector: Option<Arc<dyn crate::ports::intake::JevConnector>>,
    /// The key of `YULIUS_JEV_API_KEY_FILE`, or why it is not usable.
    pub(crate) config_key: Result<Option<crate::model::Secret>, String>,
    /// Headless Claude for split proposals.
    pub proposer: Option<Arc<dyn SplitProposer>>,
    /// Probability of "several" needed to propose a split.
    pub split_threshold: f64,
    /// Probability needed to hold a typed prompt.
    pub scope_threshold: f64,
    permits: Arc<tokio::sync::Semaphore>,
}

impl IntakeServices {
    /// Services with the default thresholds and a fixed evaluator (`None`: nothing is checked).
    #[must_use]
    pub fn new(
        jev: Option<Arc<dyn ChoiceEvaluator>>,
        proposer: Option<Arc<dyn SplitProposer>>,
    ) -> Self {
        Self {
            jev: std::sync::RwLock::new(jev.map(crate::jev_key::ActiveJev::fixed)),
            connector: None,
            config_key: Ok(None),
            proposer,
            split_threshold: policy::DEFAULT_SPLIT_THRESHOLD,
            scope_threshold: policy::DEFAULT_SCOPE_THRESHOLD,
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_PROPOSALS)),
        }
    }

    /// The same services with a key manager: `connector` builds evaluators from keys a
    /// superadmin saves; `config_key` is the key of `YULIUS_JEV_API_KEY_FILE` (or why it is not
    /// usable), used while no key is saved. [`App::load_jev_key`] picks the key in use.
    #[must_use]
    pub fn with_key_management(
        mut self,
        connector: Arc<dyn crate::ports::intake::JevConnector>,
        config_key: Result<Option<crate::model::Secret>, String>,
    ) -> Self {
        self.connector = Some(connector);
        self.config_key = config_key;
        self
    }

    /// The same services with these thresholds (each clamped to 0.5–1).
    #[must_use]
    pub fn with_thresholds(mut self, split: f64, scope: f64) -> Self {
        self.split_threshold = split.clamp(0.5, 1.0);
        self.scope_threshold = scope.clamp(0.5, 1.0);
        self
    }
}

fn excerpt(text: &str) -> String {
    let mut out: String = text.chars().take(EXCERPT_CHARS).collect();
    if text.chars().count() > EXCERPT_CHARS {
        out.push('…');
    }
    out
}

fn short(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    out
}

fn percent(answer: &ChoiceAnswer, key: &str) -> u32 {
    // 0..=100 after the clamp, so the cast is exact.
    (answer.probability(key) * 100.0).round().clamp(0.0, 100.0) as u32
}

impl App {
    /// Installs the intake services (once; later calls are ignored and return `false`).
    pub fn set_intake(&self, services: IntakeServices) -> bool {
        self.intake.set(services).is_ok()
    }

    pub(crate) fn jev(&self) -> Option<(Arc<dyn ChoiceEvaluator>, &IntakeServices)> {
        let services = self.intake.get()?;
        let jev = services.jev.read().ok()?.as_ref()?.evaluator.clone();
        Some((jev, services))
    }

    /// Records that `user` typed into `task`'s terminal now (a prompt submitted soon after is
    /// theirs, TSK-013).
    pub fn record_typing(&self, task: &TaskId, user: &UserId) {
        let runtime = self.runtime(task);
        runtime.typed_at_ms.store(self.now().0, Ordering::SeqCst);
        if let Ok(mut who) = runtime.typed_by.lock() {
            *who = Some(user.clone());
        }
    }

    /// `Some(who)` when a person typed into the task's terminal within
    /// [`policy::TYPED_WINDOW_MS`].
    fn typed_recently(&self, task: &TaskId) -> Option<Option<UserId>> {
        let runtime = self.runtime(task);
        let at = runtime.typed_at_ms.load(Ordering::SeqCst);
        if at <= 0 || self.now().0.saturating_sub(at) > policy::TYPED_WINDOW_MS {
            return None;
        }
        Some(runtime.typed_by.lock().ok().and_then(|who| who.clone()))
    }

    /// `POST /api/sandboxes/{sid}/intake/check` (TSK-011): whether a new task's prompt mixes
    /// separate workstreams. Short prompts, no Jev key and every Jev failure answer "one".
    ///
    /// # Errors
    /// Access errors; `Invalid` for an oversized prompt.
    pub async fn intake_check(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &IntakeRequest,
    ) -> AppResult<IntakeVerdict> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Work).await?;
        let prompt = req.prompt.trim();
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(
                "the prompt must be 1 byte to 64 KiB".into(),
            ));
        }
        let one = |checked| IntakeVerdict {
            verdict: SplitVerdict::One.as_str().to_owned(),
            checked,
        };
        if !policy::needs_split_check(prompt) {
            return Ok(one(false));
        }
        let Some((jev, services)) = self.jev() else {
            return Ok(one(false));
        };
        match jev
            .evaluate(
                &policy::split_state(prompt),
                &SPLIT_QUESTION,
                SPLIT_CHECK_DEADLINE,
            )
            .await
        {
            Ok(answer) => Ok(IntakeVerdict {
                verdict: policy::split_verdict(&answer, services.split_threshold)
                    .as_str()
                    .to_owned(),
                checked: true,
            }),
            Err(failure) => {
                tracing::info!(%failure, "split check skipped");
                Ok(one(false))
            }
        }
    }

    /// `POST /api/sandboxes/{sid}/intake/proposal` (TSK-011): Claude's smallest split of the
    /// request, checked (2–4 tasks grouped by area, every listed item kept word for word);
    /// no tasks means "keep it as one task".
    ///
    /// # Errors
    /// `Conflict` without a proposer or account, `Capacity` while other proposals run,
    /// `Upstream` when Claude fails or its proposal is unusable.
    pub async fn intake_proposal(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &IntakeRequest,
    ) -> AppResult<SplitProposal> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Work).await?;
        let prompt = req.prompt.trim();
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(AppError::Invalid(
                "the prompt must be 1 byte to 64 KiB".into(),
            ));
        }
        let (_, services) = self
            .jev()
            .ok_or_else(|| AppError::Conflict("split proposals are not configured".into()))?;
        let proposer = services
            .proposer
            .as_ref()
            .ok_or_else(|| AppError::Conflict("split proposals are not configured".into()))?;
        let _permit = Arc::clone(&services.permits)
            .try_acquire_owned()
            .map_err(|_| {
                AppError::Capacity("other split proposals are being prepared; try again".into())
            })?;
        // A split proposal runs a small model: only the account-wide windows bind.
        let account = self
            .pool_account(Provider::Anthropic, yulius_domain::usage::ModelFamily::Any)
            .await?;
        let credential = self.open_credential(&account)?;
        let tasks = proposer
            .propose(&credential, prompt)
            .await
            .map_err(|e| AppError::Upstream(format!("Claude could not propose a split: {e}")))?;
        match policy::check_proposal(prompt, tasks) {
            Ok(Proposal::One) => Ok(SplitProposal { tasks: Vec::new() }),
            Ok(Proposal::Split(tasks)) => Ok(SplitProposal {
                tasks: tasks
                    .into_iter()
                    .map(|t| ProposedTaskView {
                        title: t.title,
                        prompt: t.prompt,
                        after_previous: t.after_previous,
                    })
                    .collect(),
            }),
            Err(err) => {
                tracing::info!(error = %err, "split proposal rejected");
                Err(AppError::Upstream(format!(
                    "the split proposal was not usable: {err}"
                )))
            }
        }
    }

    /// The scope check of `UserPromptSubmit` (TSK-013): `Some(block)` holds the prompt. Passes
    /// (`None`) without Jev, when the task is not running, for slash commands, short prompts,
    /// Claude Code's own turns, the task's own prompt, prompts nobody typed in the last minute
    /// (Claude Code's or Yulius's own turns), a "send anyway" of the identical prompt, any Jev
    /// failure and every answer below the threshold.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn screen_prompt(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<Option<HookOutput>> {
        let Some((jev, services)) = self.jev() else {
            return Ok(None);
        };
        let task = &ctx.task;
        let Some(text) = input.prompt.as_deref().map(str::trim) else {
            return Ok(None);
        };
        // A task paused at its turn end for capacity has a live session too: what a person types
        // there is checked like in a running one.
        if !matches!(
            task.status,
            TaskStatus::Running | TaskStatus::WaitingCapacity
        ) || policy::bypass(text).is_some()
            || text == task.prompt.trim()
        {
            return Ok(None);
        }
        let store = &self.deps.store;
        let sha = self.deps.crypto.sha256_hex(text.as_bytes());
        let now = self.now();
        // "Send anyway" first, so the override is consumed even when it arrives late.
        let since = Timestamp(now.0.saturating_sub(policy::SENT_HOLD_TTL_MS));
        if store.take_sent_hold(&task.id, &sha, since).await? {
            return Ok(None);
        }
        let Some(typed_by) = self.typed_recently(&task.id) else {
            return Ok(None);
        };
        let steps: Vec<(String, String)> = store
            .steps(&task.id)
            .await?
            .into_iter()
            .map(|s| (s.title, s.status))
            .collect();
        let state = policy::scope_state(&task.title, &task.prompt, &steps, text);
        let answer = match jev
            .evaluate(&state, &SCOPE_QUESTION, SCOPE_CHECK_DEADLINE)
            .await
        {
            Ok(answer) => answer,
            Err(failure) => {
                tracing::info!(%failure, task = %task.id, "scope check skipped; the prompt passes");
                return Ok(None);
            }
        };
        let class = policy::scope_class(&answer, services.scope_threshold);
        // One line per check, without the prompt (it is private): whether intake caught a typed
        // prompt can be read from the server log.
        tracing::info!(
            task = %task.id,
            verdict = class.as_str(),
            percent = percent(&answer, class.as_str()),
            "scope check of a typed prompt"
        );
        if class == PromptClass::Context {
            return Ok(None);
        }
        let hold = HoldRecord {
            id: self.deps.crypto.new_id(),
            task_id: task.id.clone(),
            class,
            prompt: text.to_owned(),
            prompt_sha256: sha,
            percent: percent(&answer, class.as_str()),
            typed_by: typed_by.clone(),
            state: hold_state::PENDING.to_owned(),
            created_at: now,
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        };
        store.insert_hold(&hold).await?;
        let actor = match &typed_by {
            Some(user) => self.login_of(user).await,
            None => "unknown".into(),
        };
        store
            .audit(
                now,
                &actor,
                "prompt.held",
                Some(task.id.as_str()),
                Some(class.as_str()),
            )
            .await?;
        self.publish_task(&task.id).await;
        Ok(Some(HookOutput::block(class.held_reason())))
    }

    /// The task's pending held prompt as the API shows it.
    pub(crate) async fn held_prompt_view(&self, task: &TaskId) -> Option<Box<HeldPrompt>> {
        let hold = self.deps.store.pending_hold(task).await.ok()??;
        let typed_by = match &hold.typed_by {
            Some(user) => Some(self.login_of(user).await),
            None => None,
        };
        Some(Box::new(HeldPrompt {
            id: hold.id,
            class: hold.class.as_str().to_owned(),
            excerpt: excerpt(&hold.prompt),
            percent: hold.percent,
            typed_by,
            created_at: rfc3339(hold.created_at),
        }))
    }

    /// `POST /api/tasks/{tid}/held-prompts/{hid}` (TSK-013): one decision per held prompt;
    /// the first decision wins, a second gets `Conflict`.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Conflict` (already decided, not running for `send`, the
    /// task did not stop for `revert`), `Invalid` (unknown action, steps, missing confirmation).
    pub async fn resolve_held_prompt(
        self: &Arc<Self>,
        ctx: &AuthContext,
        task_id: &str,
        hold_id: &str,
        req: &ResolveHeldPrompt,
    ) -> AppResult<HeldPromptOutcome> {
        let task = self.load_task(ctx, task_id, Action::Work).await?;
        let hold = self
            .deps
            .store
            .hold(hold_id)
            .await?
            .filter(|h| h.task_id == task.id)
            .ok_or(AppError::NotFound)?;
        if hold.state != hold_state::PENDING {
            return Err(AppError::Conflict("this prompt was already handled".into()));
        }
        let login = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into());
        let decision = Decision {
            task: &task,
            hold: &hold,
            ctx,
            actor: format!("user:{login}"),
        };
        let (created, note) = match req.action.as_str() {
            "dismiss" => {
                decision.claim(self, hold_state::DISMISSED, None).await?;
                (
                    None,
                    "Dismissed. The prompt is still in Claude Code's input box.".to_owned(),
                )
            }
            "send" => (None, self.held_send(&decision).await?),
            "parallel" => {
                let created = self
                    .held_new_task(&decision, hold_state::PARALLEL, None)
                    .await?;
                let note = if created.status == TaskStatus::Chained {
                    started_later(created.number)
                } else {
                    format!("Started #{} in parallel.", created.number)
                };
                (Some(created), note)
            }
            "after" => {
                let created = self
                    .held_new_task(&decision, hold_state::AFTER, Some(&task))
                    .await?;
                let note = format!("#{} runs after this task.", created.number);
                (Some(created), note)
            }
            "skip_after" => self.held_skip_after(&decision, &req.skip_step_ids).await?,
            "revert" => self.held_revert(&decision, req.confirm).await?,
            _ => {
                return Err(AppError::Invalid(
                    "action must be parallel, after, send, skip_after, revert or dismiss".into(),
                ));
            }
        };
        self.publish_task(&task.id).await;
        let current = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let created = match created {
            Some(t) => Some(self.task_summary(&t).await?),
            None => None,
        };
        Ok(HeldPromptOutcome {
            task: self.task_summary(&current).await?,
            created,
            note,
        })
    }

    /// "Send anyway": the identical prompt passes once; Yulius presses Enter in the session,
    /// where Claude Code kept the held text in its input box.
    async fn held_send(&self, d: &Decision<'_>) -> AppResult<String> {
        if d.task.status != TaskStatus::Running {
            return Err(AppError::Conflict(
                "the task is not running; continue it first".into(),
            ));
        }
        d.claim(self, hold_state::SENT, None).await?;
        self.deps
            .store
            .audit(
                self.now(),
                &d.actor,
                "prompt.sent_anyway",
                Some(d.task.id.as_str()),
                Some(d.hold.class.as_str()),
            )
            .await?;
        Ok(match self.send_input(&d.task.id, b"\r").await {
            Ok(()) => "Sent to the agent unchanged.".to_owned(),
            Err(_) => "Recorded: press Enter in the terminal to send it unchanged.".to_owned(),
        })
    }

    /// A new task with the held prompt: started now, or chained after `after`. Settings
    /// (harness, model, effort, mode) follow the task the prompt was typed into.
    async fn held_new_task(
        self: &Arc<Self>,
        d: &Decision<'_>,
        state: &str,
        after: Option<&TaskRecord>,
    ) -> AppResult<TaskRecord> {
        d.claim(self, state, None).await?;
        let req = NewTask {
            sandbox: d.task.sandbox_id.clone(),
            created_by: d.ctx.user.id.clone(),
            prompt: d.hold.prompt.clone(),
            harness: d.task.harness,
            title: None,
            workflow_id: None,
            steps: Vec::new(),
            model: d.task.model.clone(),
            effort: d.task.effort.clone(),
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode: d.task.mode,
            supertask: false,
        };
        match self.create_task_with(req, after).await {
            Ok(created) => {
                d.link(self, state, &created.id).await;
                Ok(created)
            }
            Err(err) => {
                d.unclaim(self, state).await;
                Err(err)
            }
        }
    }

    /// Drops scope: the chosen open steps become `skipped` (with the user's note), and the new
    /// direction is chained after this task.
    async fn held_skip_after(
        self: &Arc<Self>,
        d: &Decision<'_>,
        step_ids: &[String],
    ) -> AppResult<(Option<TaskRecord>, String)> {
        let store = &self.deps.store;
        let steps = store.steps(&d.task.id).await?;
        let mut chosen = Vec::new();
        for id in step_ids {
            let step = steps
                .iter()
                .find(|s| s.id.as_str() == id)
                .ok_or_else(|| AppError::Invalid("unknown step".into()))?;
            if StepStatus::parse(&step.status).is_some_and(StepStatus::is_resolved) {
                return Err(AppError::Invalid(format!(
                    "the step \"{}\" is already {}",
                    step.title, step.status
                )));
            }
            if !chosen
                .iter()
                .any(|c: &&crate::model::StepRecord| c.id == step.id)
            {
                chosen.push(step);
            }
        }
        let created = self
            .held_new_task(d, hold_state::SKIPPED_AFTER, Some(d.task))
            .await?;
        let now = self.now();
        let note_text = format!("Ruled out by the user: {}", short(&d.hold.prompt, 300));
        for step in &chosen {
            store
                .update_step(&step.id, "skipped", Some(&note_text), &d.actor, now)
                .await?;
        }
        if !chosen.is_empty() {
            self.publish_steps(d.task).await;
        }
        // The skipped steps carry the change to the agent (their note says the user ruled them
        // out); no message is stored (SPEC-028 MSR-002).
        let note = format!(
            "{} skipped; #{} runs after this task.",
            match chosen.len() {
                1 => "1 step".to_owned(),
                n => format!("{n} steps"),
            },
            created.number
        );
        Ok((Some(created), note))
    }

    /// Redirect: cancel this task, keep its work on a new branch and revert its worktree, then
    /// start a new task with the prompt. Destructive, so it needs `confirm`.
    async fn held_revert(
        self: &Arc<Self>,
        d: &Decision<'_>,
        confirm: bool,
    ) -> AppResult<(Option<TaskRecord>, String)> {
        if !confirm {
            return Err(AppError::Invalid(
                "reverting cancels this task and resets its branch; confirm it explicitly".into(),
            ));
        }
        d.claim(self, hold_state::REVERTED, None).await?;
        match self.cancel_and_revert(d).await {
            Ok(kept) => {
                let note = match &kept {
                    Some(branch) => format!(
                        "#{} was cancelled and its changes reverted. Its work is kept on branch {branch}; nothing was deleted.",
                        d.task.number
                    ),
                    None => format!(
                        "#{} was cancelled; it had no changes to revert.",
                        d.task.number
                    ),
                };
                let req = NewTask {
                    sandbox: d.task.sandbox_id.clone(),
                    created_by: d.ctx.user.id.clone(),
                    prompt: d.hold.prompt.clone(),
                    harness: d.task.harness,
                    title: None,
                    workflow_id: None,
                    steps: Vec::new(),
                    model: d.task.model.clone(),
                    effort: d.task.effort.clone(),
                    origin: TaskOrigin::Ui,
                    automation_run: None,
                    start: true,
                    mode: d.task.mode,
                    supertask: false,
                };
                let created = self.create_task_with(req, None).await?;
                d.link(self, hold_state::REVERTED, &created.id).await;
                let note = if created.status == TaskStatus::Chained {
                    format!("{note} {}", started_later(created.number))
                } else {
                    format!("{note} Started #{}.", created.number)
                };
                Ok((Some(created), note))
            }
            Err(err) => {
                d.unclaim(self, hold_state::REVERTED).await;
                Err(err)
            }
        }
    }

    /// Stops the task (waiting for its process to be reaped), keeps and reverts its worktree,
    /// archives it. Returns the branch holding the work, when there was a worktree.
    async fn cancel_and_revert(self: &Arc<Self>, d: &Decision<'_>) -> AppResult<Option<String>> {
        let store = &self.deps.store;
        let task_id = &d.task.id;
        let current = store.task(task_id).await?.ok_or(AppError::NotFound)?;
        if !current.status.is_resting() {
            self.stop_task_now(task_id).await?;
            let deadline = tokio::time::Instant::now() + REVERT_STOP_WAIT;
            loop {
                let status = store
                    .task(task_id)
                    .await?
                    .map(|t| t.status)
                    .ok_or(AppError::NotFound)?;
                if status.is_resting() && store.live_attempt(task_id).await?.is_none() {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppError::Conflict(
                        "the task did not stop in time; nothing was reverted, try again".into(),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let now = self.now();
        let kept = match &current.worktree_path {
            Some(worktree) if std::path::Path::new(worktree).join(".git").exists() => {
                let sandbox = store
                    .sandbox(&current.sandbox_id)
                    .await?
                    .ok_or(AppError::NotFound)?;
                let base = crate::delivery::delivery_profile(&sandbox).integration_branch;
                let keep = format!("yulius/reverted/{}-{}", current.number, now.0);
                let user = store.user(&d.ctx.user.id).await?;
                let (name, email) = crate::projects::commit_identity(user.as_ref());
                self.deps
                    .git
                    .keep_and_revert(std::path::Path::new(worktree), &keep, &base, &name, &email)
                    .await?;
                Some(keep)
            }
            _ => None,
        };
        store
            .update_task(
                task_id,
                &TaskFieldUpdate {
                    archived_at: Some(Some(now)),
                    ..TaskFieldUpdate::default()
                },
                now,
            )
            .await?;
        store
            .audit(
                now,
                &d.actor,
                "task.reverted",
                Some(task_id.as_str()),
                kept.as_deref(),
            )
            .await?;
        self.publish_task(task_id).await;
        Ok(kept)
    }
}

/// One decision on a held prompt.
struct Decision<'a> {
    task: &'a TaskRecord,
    hold: &'a HoldRecord,
    ctx: &'a AuthContext,
    actor: String,
}

impl Decision<'_> {
    /// Moves the hold from pending to `state`; the first decision wins.
    async fn claim(&self, app: &App, state: &str, new_task: Option<&TaskId>) -> AppResult<()> {
        let won = app
            .deps
            .store
            .resolve_hold(
                &self.hold.id,
                hold_state::PENDING,
                state,
                &self.actor,
                app.now(),
                new_task,
            )
            .await?;
        if won {
            Ok(())
        } else {
            Err(AppError::Conflict("this prompt was already handled".into()))
        }
    }

    /// Records the task created for the decision.
    async fn link(&self, app: &App, state: &str, task: &TaskId) {
        let _ = app
            .deps
            .store
            .resolve_hold(
                &self.hold.id,
                state,
                state,
                &self.actor,
                app.now(),
                Some(task),
            )
            .await;
    }

    /// Puts the hold back to pending after a failed decision, so the card stays.
    async fn unclaim(&self, app: &App, state: &str) {
        let _ = app
            .deps
            .store
            .resolve_hold(
                &self.hold.id,
                state,
                hold_state::PENDING,
                &self.actor,
                app.now(),
                None,
            )
            .await;
    }
}

/// The note for a new task that waits for the sandbox's preparation (U-08).
fn started_later(number: u32) -> String {
    format!("#{number} starts by itself once this project's preparation is done.")
}
